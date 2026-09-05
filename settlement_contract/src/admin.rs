use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env, Symbol, Vec};

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS},
    events::{self, AdminTransferred, PendingRecovery},
    storage::{self, CommonDataKey},
    upgrade::probe_supports_interface,
};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_and_bump_ttl, read_admin, read_admins,
    read_governance, read_pending_recovery, read_recovery_address, read_rule_or_default,
    read_threshold, validate_admins_and_threshold, validate_fee_against_governance,
    validate_governance, validate_nonzero_address, verify_admin_auth, write_admins,
    read_fallback_rule, read_governance, read_optional_primary_admin, read_pending_recovery,
    read_recovery_address, read_rule_or_default, read_schema_version, read_threshold,
    validate_admins_and_threshold, validate_governance, validate_nonzero_address,
    verify_admin_auth, write_admins,
};
use crate::types::{DataKey, Operation, ScheduledOp, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, BOOTSTRAP_DEFAULT_RULE, CURRENT_SCHEMA_VERSION,
    DEFAULT_TIMELOCK_DELAY_SECONDS, MAX_SETTLEMENT_DELAY_LEDGER, MERCHANT_TTL_BUMP,
    MERCHANT_TTL_THRESHOLD, RULE_TTL_BUMP, RULE_TTL_THRESHOLD, SCHEDULED_OP_TTL_BUMP,
    SCHEDULED_OP_TTL_THRESHOLD,
};

#[contractimpl]
impl SettlementContract {
    pub fn supports_interface(_env: Env, version: u32) -> bool {
        version == crate::SUPPORTED_INTERFACE_VERSION
    }

    /// Initialize the contract with the given admin address.
    ///
    /// # Panics
    ///
    /// * [`AlreadyInitialized`](SettlementError::AlreadyInitialized) — if the contract has already been initialized.
    pub fn init(
        env: Env,
        deployer: Address,
        admins: Vec<Address>,
        threshold: u32,
        governance: Address,
        recovery_address: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::AlreadyInitialized);
        }
        // Gate initialization to the deployer to prevent front-running (issue #684).
        deployer.require_auth();
        validate_admins_and_threshold(&env, &admins, threshold);
        validate_governance(&env, &governance);
        validate_nonzero_address(
            &env,
            &recovery_address,
            SettlementError::InvalidRecoveryAddress,
        );
        for i in 0..threshold {
            admins.get(i).unwrap().require_auth();
        }
        env.storage().instance().set(&DataKey::Deployer, &deployer);
        write_admins(&env, &admins, threshold);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &recovery_address);
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
    }

    pub fn is_initialized(env: Env) -> bool {
        env.storage().instance().has(&DataKey::Admin)
    }

    pub fn get_admin(env: Env) -> Vec<Address> {
        read_admins(&env)
    }

    pub fn get_threshold(env: Env) -> u32 {
        read_threshold(&env)
    }

    pub fn get_governance(env: Env) -> Address {
        read_governance(&env)
    }

    pub fn get_recovery_address(env: Env) -> Address {
        read_recovery_address(&env)
    }

    /// Updates the governance address after validating it.
    ///
    /// # Validation policy (issue #562)
    ///
    /// `new_governance` is validated by [`validate_governance`] before it is
    /// stored. The timelocked path (`_update_governance`, reached through
    /// [`Operation::UpdateGovernance`]) routes through the **same** function,
    /// so a governance address cannot bypass validation by taking the
    /// scheduled route instead of the direct one. Any change to governance
    /// validation must be made inside `validate_governance` — never inlined in
    /// just one path, or the two paths will drift apart again.
    pub fn update_governance(env: Env, signers: Vec<Address>, new_governance: Address) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        assert_not_paused(&env);
        validate_governance(&env, &new_governance);
        let admin = signers.get(0).unwrap();
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(&env, events::GOVERNANCE_UPDATED_EVENT),),
            (admin, new_governance),
        );
    }

    pub fn initiate_recovery(env: Env, new_admin: Address) {
        let recovery_address = read_recovery_address(&env);
        recovery_address.require_auth();
        validate_nonzero_address(
            &env,
            &new_admin,
            SettlementError::InvalidAdmin,
        );

        if env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(&env, SettlementError::RecoveryAlreadyPending);
        }

        let pending = PendingRecovery {
            new_admin: new_admin.clone(),
            execute_after: env.ledger().timestamp() + RECOVERY_DELAY_SECONDS,
            initiated_by: recovery_address.clone(),
        };
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
        events::emit_recovery_initiated(&env, &recovery_address, &new_admin, pending.execute_after);
    }

    pub fn cancel_recovery(env: Env, signers: Vec<Address>) {
        // Cancellation policy (issue #560): the pending recovery may be
        // cancelled by the address recorded as `initiated_by` (the recovery
        // address that started it) or by the current admin set meeting the
        // multisig threshold. The admin path is unchanged; the initiator
        // path lets an initiation be undone by the address that made it.
        // Any other caller is refused with `Unauthorized`.
        let pending = read_pending_recovery(&env);
        let cancelled_by_initiator =
            signers.len() == 1 && signers.get(0).unwrap() == pending.initiated_by;
        if cancelled_by_initiator {
            pending.initiated_by.require_auth();
        } else {
            verify_admin_auth(&env, &signers, read_threshold(&env));
        }
        let canceller = signers.get(0).unwrap();
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_cancelled(&env, &canceller);
    }

    pub fn update_recovery_address(env: Env, signers: Vec<Address>, new_recovery: Address) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();
        validate_nonzero_address(
            &env,
            &new_recovery,
            SettlementError::InvalidRecoveryAddress,
            SettlementError::InvalidRecoveryAddress,
        );
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &new_recovery);
        env.events().publish(
            (
                Symbol::new(&env, events::RECOVERY_ADDRESS_UPDATED_EVENT),
                new_recovery.clone(),
            ),
            admin,
        );
    }

    pub fn execute_recovery(env: Env) {
        let pending = read_pending_recovery(&env);
        if env.ledger().timestamp() < pending.execute_after {
            panic_with_error!(&env, SettlementError::RecoveryDelayActive);
        }

        let old_admin = read_optional_primary_admin(&env);
        let new_admins = soroban_sdk::vec![&env, pending.new_admin.clone()];
        // Finalize the new admin configuration before consuming the recovery.
        // If validation or writing fails, the pending target remains available.
        write_admins(&env, &new_admins, 1);
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_executed(
            &env,
            &AdminTransferred {
                old_admin,
                new_admin: pending.new_admin.clone(),
            },
        );
    }

    pub fn transfer_admin(
        env: Env,
        signers: Vec<Address>,
        new_admins: Vec<Address>,
        new_threshold: u32,
    ) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        validate_admins_and_threshold(&env, &new_admins, new_threshold);

        // Enforce admin/merchant exclusivity in both directions (issue #692).
        for i in 0..new_admins.len() {
            if is_merchant_registered_and_bump_ttl(&env, new_admins.get(i).unwrap()) {
                panic_with_error!(&env, SettlementError::InvalidAdmin);
            }
        }

        let old_admin = read_admin(&env);
        write_admins(&env, &new_admins, new_threshold);
        let primary_new_admin = new_admins.get(0).unwrap();
        events::emit_admin_transferred(
            &env,
            &AdminTransferred {
                old_admin,
                new_admin: primary_new_admin,
            },
        );
    }

    pub fn change_threshold(env: Env, signers: Vec<Address>, new_threshold: u32) {
        let admins = read_admins(&env);
        if new_threshold == 0 || new_threshold > admins.len() {
            panic_with_error!(&env, SettlementError::InvalidThreshold);
        }

        let current_threshold = read_threshold(&env);
        verify_admin_auth(&env, &signers, current_threshold + 1);

        env.storage()
            .instance()
            .set(&CommonDataKey::Threshold, &new_threshold);
        env.events().publish(
            (Symbol::new(&env, events::THRESHOLD_CHANGED_EVENT),),
            (current_threshold, new_threshold),
        );
    }

    pub fn upgrade(env: Env, signers: Vec<Address>, new_wasm_hash: BytesN<32>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        // Verify the new Wasm supports the required BettaPay interface
        // (version 1) before overwriting the running code. See
        // `bettapay_common::upgrade::probe_supports_interface`.
        if !probe_supports_interface(&env, &new_wasm_hash, 1) {
            panic_with_error!(&env, SettlementError::InvalidWasmInterface);
        }

        let event_wasm_hash = new_wasm_hash.clone();
        env.events().publish(
            (
                Symbol::new(&env, events::CONTRACT_UPGRADED_EVENT),
                event_wasm_hash,
            ),
            admin,
        );

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    pub fn pause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if storage::is_paused(&env) {
            panic_with_error!(&env, SettlementError::AlreadyPaused);
        }
        let admin = signers.get(0).unwrap();
        storage::apply_pause(&env, &admin);
    }

    pub fn unpause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if !storage::is_paused(&env) {
            panic_with_error!(&env, SettlementError::AlreadyUnpaused);
        }
        let admin = signers.get(0).unwrap();
        storage::apply_unpause(&env, &admin);
    }

    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// Idempotent schema migration entry point.
    ///
    /// Issue #704: ships the schema-version marker (written at `init`) and a
    /// migration entry point so the first real storage migration has a
    /// defined baseline, mirroring governance_contract's `migrate` (issue
    /// #507). There is no existing storage-format difference to convert yet,
    /// so calling `migrate` simply confirms the `SchemaVersion` marker is at
    /// `CURRENT_SCHEMA_VERSION`. It is admin-gated and idempotent: a
    /// contract already at `CURRENT_SCHEMA_VERSION` is a no-op.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is paused.
    /// * Auth failure — if the signers do not satisfy the admin threshold.
    pub fn migrate(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        assert_not_paused(&env);
        let admin = signers.get(0).unwrap();

        if read_schema_version(&env) < CURRENT_SCHEMA_VERSION {
            env.storage()
                .instance()
                .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
        }
        env.events().publish(
            (Symbol::new(&env, events::MIGRATED_EVENT),),
            (admin, CURRENT_SCHEMA_VERSION),
        );
    }

    /// Schedules an administrative operation to be executed after a timelock.
    ///
    /// # Authorization
    ///
    /// Requires authentication from the configured admin set. The caller must
    /// pass enough valid signers to meet the current multisig threshold, so a
    /// single (possibly compromised) admin cannot enqueue an operation —
    /// including `Upgrade` — without the required consensus (Issue #463).
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if signers lack admin authority.
    /// * [`ExecutionNotReady`](SettlementError::ExecutionNotReady) — if `execute_in` is less than `DEFAULT_TIMELOCK_DELAY_SECONDS`.
    /// * [`OperationAlreadyScheduled`](SettlementError::OperationAlreadyScheduled) — if the operation is already in the queue.
    pub fn schedule(env: Env, signers: Vec<Address>, operation: Operation, execute_in: u64) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        assert_not_paused(&env);
        let caller = signers.get(0).unwrap();

        if execute_in < DEFAULT_TIMELOCK_DELAY_SECONDS {
            panic_with_error!(&env, SettlementError::ExecutionNotReady);
        }

        let operation_xdr = operation.clone().to_xdr(&env);
        let op_hash: BytesN<32> = env.crypto().sha256(&operation_xdr).into();
        let key = DataKey::ScheduledOperation(op_hash.clone());

        // The hash only indexes storage; a match on `key` alone does not
        // prove `operation` is what was scheduled. Compare the stored XDR
        // bytes to tell a genuine re-schedule of the same operation (still
        // rejected as a duplicate) apart from two *different* operations
        // whose hashes happen to collide (issue #570).
        if let Some(existing) = env.storage().persistent().get::<_, ScheduledOp>(&key) {
            if existing.operation_xdr == operation_xdr {
                panic_with_error!(&env, SettlementError::OperationAlreadyScheduled);
            }
            panic_with_error!(&env, SettlementError::OperationHashCollision);
        }

        let execute_at = env.ledger().timestamp() + execute_in;
        env.storage().persistent().set(
            &key,
            &ScheduledOp {
                operation_xdr,
                execute_at,
            },
        );
        env.storage()
            .persistent()
            .extend_ttl(&key, SCHEDULED_OP_TTL_THRESHOLD, SCHEDULED_OP_TTL_BUMP);

        env.events().publish(
            (Symbol::new(&env, events::OP_SCHEDULED_EVENT), op_hash),
            (caller, execute_at),
        );
    }

    /// Executes a previously scheduled administrative operation.
    ///
    /// # Authorization
    ///
    /// Requires authentication from the configured admin set. The caller must
    /// pass enough valid signers to meet the current multisig threshold.
    /// This prevents any external actor from front-running the timelock expiry
    /// and executing an operation the admins intended to cancel (Issue #462).
    pub fn execute(env: Env, signers: Vec<Address>, operation: Operation) {
        verify_admin_auth(&env, &signers, read_threshold(&env));

        let op_hash: BytesN<32> = env.crypto().sha256(&operation.clone().to_xdr(&env)).into();
    /// # Execution auth policy (uniform)
    ///
    /// `execute` deliberately performs **no caller authentication** for any
    /// [`Operation`] variant handled below. Authorization is enforced at the
    /// timelock boundary instead: [`schedule`](Self::schedule) (and
    /// [`cancel`](Self::cancel)) require admin multisig auth via
    /// `verify_admin_auth` against the stored threshold. Once an operation
    /// has been scheduled by the admins and its timelock delay has elapsed,
    /// execution is intentionally permissionless so any caller can trigger it
    /// (issue #693). This is the single uniform policy for **every** variant
    /// in the `match` below — including `CancelRecovery`, which historically
    /// required primary-admin auth and was normalized to match the rest
    /// (issue #561 / #693). No variant may add its own `require_auth` here;
    /// if the policy ever changes, it must change for all variants at once
    /// and be re-documented on this function.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`OperationNotScheduled`](SettlementError::OperationNotScheduled) — if the operation was not scheduled.
    /// * [`ExecutionNotReady`](SettlementError::ExecutionNotReady) — if the timelock delay has not elapsed.
    pub fn execute(env: Env, executor: Address, operation: Operation) {
        assert_not_paused(&env);
        executor.require_auth();

        let operation_xdr = operation.clone().to_xdr(&env);
        let op_hash: BytesN<32> = env.crypto().sha256(&operation_xdr).into();
        let key = DataKey::ScheduledOperation(op_hash.clone());

        let scheduled: ScheduledOp = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::OperationNotScheduled));

        // Guard against a hash collision letting an operation that was never
        // scheduled ride the timelock slot of a different, already-pending
        // one (issue #570).
        if scheduled.operation_xdr != operation_xdr {
            panic_with_error!(&env, SettlementError::OperationNotScheduled);
        }

        if env.ledger().timestamp() < scheduled.execute_at {
            panic_with_error!(&env, SettlementError::ExecutionNotReady);
        }

        env.storage().persistent().remove(&key);

        match operation {
            Operation::UpdateGovernance(new_gov) => {
                Self::_update_governance(&env, &executor, new_gov)
            }
            Operation::CancelRecovery => Self::_cancel_recovery(&env, &executor),
            Operation::TransferAdmin(new_admins, new_threshold) => {
                Self::_transfer_admin(&env, &executor, new_admins, new_threshold)
            }
            Operation::Upgrade(wasm_hash) => Self::_upgrade(&env, &executor, wasm_hash),
            Operation::RegisterMerchant(merchant) => {
                Self::_register_merchant(&env, &executor, merchant)
            }
            Operation::UnregisterMerchant(merchant) => {
                Self::_unregister_merchant(&env, &executor, merchant)
            }
            Operation::SetSettlementRule(merchant, rule) => {
                Self::_set_settlement_rule(&env, &executor, merchant, rule)
            }
            Operation::ClearSettlementRule(merchant) => {
                Self::_clear_settlement_rule(&env, &executor, merchant)
            }
            Operation::SetDefaultRule(rule) => Self::_set_default_rule(&env, &executor, rule),
        }

        env.events()
            .publish((Symbol::new(&env, events::OP_EXECUTED_EVENT), op_hash), ());
    }

    /// Cancels a scheduled administrative operation.
    ///
    /// # Authorization
    ///
    /// Requires the same multisig threshold as [`Self::schedule`], so a single
    /// admin cannot unilaterally remove an operation the full admin set agreed
    /// to schedule (Issue #463).
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if signers lack admin authority.
    /// * [`OperationNotScheduled`](SettlementError::OperationNotScheduled) — if the operation was not scheduled.
    pub fn cancel(env: Env, signers: Vec<Address>, operation: Operation) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        assert_not_paused(&env);
        let caller = signers.get(0).unwrap();

        let operation_xdr = operation.clone().to_xdr(&env);
        let op_hash: BytesN<32> = env.crypto().sha256(&operation_xdr).into();
        let key = DataKey::ScheduledOperation(op_hash.clone());

        let scheduled: ScheduledOp = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::OperationNotScheduled));

        // A hash match alone doesn't prove this is the operation that was
        // scheduled — see the equivalent check in `execute()` (issue #570).
        if scheduled.operation_xdr != operation_xdr {
            panic_with_error!(&env, SettlementError::OperationNotScheduled);
        }

        env.storage().persistent().remove(&key);

        env.events().publish(
            (Symbol::new(&env, events::OP_CANCELLED_EVENT), op_hash),
            caller,
        );
    }

    // --- Internal Admin Functions ---

    /// Timelocked governance update, reached via [`Operation::UpdateGovernance`]
    /// from [`execute`](Self::execute).
    ///
    /// # Validation policy (issue #562)
    ///
    /// Mirrors the direct `update_governance` entry point exactly: the new
    /// address must pass [`validate_governance`] before it is stored. Both
    /// paths intentionally share this single function so the scheduled path
    /// can never enforce a weaker (or different) check than the direct path.
    fn _update_governance(env: &Env, executor: &Address, new_governance: Address) {
        assert_not_paused(env);
        validate_governance(env, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(env, events::GOVERNANCE_UPDATED_EVENT),),
            (executor, new_governance),
        );
    }

    fn _cancel_recovery(env: &Env, executor: &Address) {
        if !env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(env, SettlementError::RecoveryNotPending);
        }
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_cancelled(env, executor);
    }

    fn _transfer_admin(env: &Env, _executor: &Address, new_admins: Vec<Address>, new_threshold: u32) {
        let old_admin = read_admin(env);
        validate_admins_and_threshold(env, &new_admins, new_threshold);
        // Enforce admin/merchant exclusivity in both directions (issue #692).
        for i in 0..new_admins.len() {
            if is_merchant_registered_and_bump_ttl(env, new_admins.get(i).unwrap()) {
                panic_with_error!(env, SettlementError::InvalidAdmin);
            }
        }
        write_admins(env, &new_admins, new_threshold);
        let primary_new_admin = new_admins.get(0).unwrap();
        events::emit_admin_transferred(
            env,
            &AdminTransferred {
                old_admin,
                new_admin: primary_new_admin,
            },
        );
    }

    fn _upgrade(env: &Env, new_wasm_hash: BytesN<32>) {
        // The timelocked path must enforce the same interface check as the
        // direct `upgrade` entry point: a scheduled upgrade to Wasm that does
        // not implement the BettaPay interface must raise
        // `InvalidWasmInterface` instead of swapping in broken code.
        if !probe_supports_interface(env, &new_wasm_hash, 1) {
            panic_with_error!(env, SettlementError::InvalidWasmInterface);
        }

        let admin = read_admin(env);
    fn _upgrade(env: &Env, executor: &Address, new_wasm_hash: BytesN<32>) {
        env.events().publish(
            (
                Symbol::new(env, events::CONTRACT_UPGRADED_EVENT),
                new_wasm_hash.clone(),
            ),
            executor,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Internal method to register a merchant.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`ZeroAddress`](SettlementError::ZeroAddress) — if the provided merchant address is the zero address.
    /// * [`InvalidAdmin`](SettlementError::InvalidAdmin) — if attempting to register an admin as a merchant.
    /// * [`MerchantExists`](SettlementError::MerchantExists) — if the merchant is already registered.
    fn _register_merchant(env: &Env, executor: &Address, merchant: Address) {
        assert_not_paused(env);
        validate_nonzero_address(
            env,
            &merchant,
        );
        let admin = read_admin(env);

        
        // Prevent an admin from being registered as a merchant
        let admins = read_admins(env);
        for i in 0..admins.len() {
            if admins.get(i).unwrap() == merchant {
                panic_with_error!(env, SettlementError::InvalidAdmin);
            }
        }

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &());
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);

        // Remove any ArchivedMerchant tombstone from a prior registration so
        // the re-registered merchant can read new payment records (issue #685).
        let archived_key = DataKey::ArchivedMerchant(merchant.clone());
        env.storage().persistent().remove(&archived_key);

        env.events().publish(
            (
                Symbol::new(env, events::MERCHANT_REGISTERED_EVENT),
                merchant,
            ),
            executor,
        );
    }

    /// Internal method to unregister a merchant.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not currently registered.
    fn _unregister_merchant(env: &Env, executor: &Address, merchant: Address) {
        assert_not_paused(env);

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        // Orphan the merchant's payment history, matching the direct
        // unregister_merchant path (issue #490).
        let archived_key = DataKey::ArchivedMerchant(merchant.clone());
        env.storage().persistent().set(&archived_key, &());
        env.storage().persistent().extend_ttl(
            &archived_key,
            MERCHANT_TTL_THRESHOLD,
            MERCHANT_TTL_BUMP,
        );

        let rule_key = DataKey::Rule(merchant.clone());
        let old_rule: Option<SettlementRule> = env.storage().persistent().get(&rule_key);
        if let Some(old_rule) = old_rule {
            env.storage().persistent().remove(&rule_key);
            // Same canonical event shape as clear_settlement_rule (issue #491).
            // Use the shared fallback chain (default → governance → bootstrap)
            // so the event matches the rule that will actually govern the next
            // payment (issue #689).
            let fallback = read_fallback_rule(env);
            events::emit_settlement_rule_cleared(env, &merchant, executor, &old_rule, &fallback);
        }

        env.events().publish(
            (
                Symbol::new(env, events::MERCHANT_UNREGISTERED_EVENT),
                merchant,
            ),
            executor,
        );
    }

    fn _set_settlement_rule(
        env: &Env,
        executor: &Address,
        merchant: Address,
        rule: SettlementRule,
    ) {
        assert_not_paused(env);

        validate_fee_against_governance(env, &rule);

        if !is_merchant_registered_and_bump_ttl(env, merchant.clone()) {
            panic_with_error!(env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps > MAX_FEE_BPS || rule.network_fee_bps > MAX_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(env, SettlementError::InvalidSettlementDelay);
        }
        if !is_merchant_registered_and_bump_ttl(env, merchant.clone()) {
            panic_with_error!(env, SettlementError::MerchantMissing);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);
        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (
                Symbol::new(env, events::SETTLEMENT_RULE_UPDATED_EVENT),
                merchant,
            ),
            (executor, prev, rule),
        );
    }

    fn _clear_settlement_rule(env: &Env, executor: &Address, merchant: Address) {
        assert_not_paused(env);

        let key = DataKey::Rule(merchant.clone());
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(env, SettlementError::MerchantRuleNotSet));

        env.storage().persistent().remove(&key);

        let fallback = read_rule_or_default(env, merchant.clone());

        env.events().publish(
            (
                Symbol::new(env, events::SETTLEMENT_RULE_CLEARED_EVENT),
                merchant,
            ),
            (executor, removed, fallback),
        );
    }

    fn _set_default_rule(env: &Env, executor: &Address, new_rule: SettlementRule) {
        assert_not_paused(env);

        validate_fee_against_governance(env, &new_rule);

        if new_rule.platform_fee_bps > BPS_DENOMINATOR || new_rule.network_fee_bps > BPS_DENOMINATOR
        {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps < MIN_FEE_BPS || new_rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps > MAX_FEE_BPS || new_rule.network_fee_bps > MAX_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .instance()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .instance()
            .set(&DataKey::DefaultRule, &new_rule);

        env.events().publish(
            (Symbol::new(env, events::DEFAULT_RULE_UPDATED_EVENT),),
            (executor, prev, new_rule),
        );
    }
}
