use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env, IntoVal, Symbol, Vec};

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS},
    events::{self, AdminTransferred, PendingRecovery},
    storage::{self, CommonDataKey},
};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_and_bump_ttl, read_admin, read_admins,
    read_governance, read_pending_recovery, read_recovery_address, read_rule_or_default,
    read_threshold, validate_admins_and_threshold, validate_governance, validate_nonzero_address,
    read_fallback_rule, read_governance, read_optional_primary_admin, read_pending_recovery,
    read_recovery_address, read_rule_or_default, read_threshold,
    validate_admins_and_threshold, validate_governance, validate_nonzero_address,
    verify_admin_auth, write_admins,
};
use crate::types::{DataKey, Operation, ScheduledOp, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, BOOTSTRAP_DEFAULT_RULE,
    DEFAULT_TIMELOCK_DELAY_SECONDS, MAX_SETTLEMENT_DELAY_LEDGER, MERCHANT_TTL_BUMP,
    MERCHANT_TTL_THRESHOLD, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
};

#[contractimpl]
impl SettlementContract {
    pub fn supports_interface(_env: Env, version: u32) -> bool {
        version == 1
    }

    /// Initialize the contract with the given admin address.
    ///
    /// # Panics
    ///
    /// * [`AlreadyInitialized`](SettlementError::AlreadyInitialized) — if the contract has already been initialized.
    pub fn init(
        env: Env,
        admins: Vec<Address>,
        threshold: u32,
        governance: Address,
        recovery_address: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::AlreadyInitialized);
        }
        validate_admins_and_threshold(&env, &admins, threshold);
        validate_governance(&env, &governance);
        validate_nonzero_address(
            &env,
            &recovery_address,
            SettlementError::InvalidRecoveryAddress,
            SettlementError::InvalidRecoveryAddress,
        );
        for i in 0..threshold {
            admins.get(i).unwrap().require_auth();
        }
        write_admins(&env, &admins, threshold);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &recovery_address);
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
        };
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
        events::emit_recovery_initiated(&env, &recovery_address, &new_admin, pending.execute_after);
    }

    pub fn cancel_recovery(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();
        if !env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(&env, SettlementError::RecoveryNotPending);
        }
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_cancelled(&env, &admin);
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

        // Deploy a probe instance of the new Wasm and verify it supports
        // the required BettaPay interface (version 1) before overwriting the
        // running code.  The wasm hash is reused as the salt so the probe
        // address is deterministic and collision-free.
        let probe = env
            .deployer()
            .with_current_contract(new_wasm_hash.clone())
            .deploy(new_wasm_hash.clone());

        let version_args: Vec<u32> = soroban_sdk::vec![&env, 1u32];
        let supports: bool = match env.try_invoke_contract::<bool, SettlementError>(
            &probe,
            &Symbol::new(&env, "supports_interface"),
            version_args.into_val(&env),
        ) {
            Ok(Ok(v)) => v,
            _ => panic_with_error!(&env, SettlementError::InvalidWasmInterface),
        };
        if !supports {
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
        let admin = signers.get(0).unwrap();
        storage::set_paused(&env, true);
        events::emit_paused(&env, &admin);
    }

    pub fn unpause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();
        storage::set_paused(&env, false);
        events::emit_unpaused(&env, &admin);
    }

    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// Schedules an administrative operation to be executed after a timelock.
    pub fn schedule(env: Env, signers: Vec<Address>, operation: Operation, execute_in: u64) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
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
            .extend_ttl(&key, 17280 * 14, 17280 * 30);

        env.events().publish(
            (Symbol::new(&env, events::OP_SCHEDULED_EVENT), op_hash),
            (caller, execute_at),
        );
    }

    /// Executes a previously scheduled administrative operation.
    pub fn execute(env: Env, operation: Operation) {
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
            Operation::UpdateGovernance(new_gov) => Self::_update_governance(&env, new_gov),
            Operation::CancelRecovery => {
                let admin = read_admin(&env);
                admin.require_auth();
                Self::_cancel_recovery(&env)
            }
            Operation::TransferAdmin(new_admins, new_threshold) => {
                Self::_transfer_admin(&env, new_admins, new_threshold)
            }
            Operation::Upgrade(wasm_hash) => Self::_upgrade(&env, wasm_hash),
            Operation::RegisterMerchant(merchant) => Self::_register_merchant(&env, merchant),
            Operation::UnregisterMerchant(merchant) => Self::_unregister_merchant(&env, merchant),
            Operation::SetSettlementRule(merchant, rule) => {
                Self::_set_settlement_rule(&env, merchant, rule)
            }
            Operation::ClearSettlementRule(merchant) => {
                Self::_clear_settlement_rule(&env, merchant)
            }
            Operation::SetDefaultRule(rule) => Self::_set_default_rule(&env, rule),
        }

        env.events()
            .publish((Symbol::new(&env, events::OP_EXECUTED_EVENT), op_hash), ());
    }

    /// Cancels a scheduled administrative operation.
    pub fn cancel(env: Env, signers: Vec<Address>, operation: Operation) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
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

    fn _update_governance(env: &Env, new_governance: Address) {
        assert_not_paused(env);
        validate_governance(env, &new_governance);
        let admin = read_admin(env);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(env, events::GOVERNANCE_UPDATED_EVENT),),
            (admin, new_governance),
        );
    }

    fn _cancel_recovery(env: &Env) {
        if !env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(env, SettlementError::RecoveryNotPending);
        }
        let admin = read_admin(env);
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_cancelled(env, &admin);
    }

    fn _transfer_admin(env: &Env, new_admins: Vec<Address>, new_threshold: u32) {
        let old_admin = read_admin(env);
        validate_admins_and_threshold(env, &new_admins, new_threshold);
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
        let admin = read_admin(env);
        env.events().publish(
            (
                Symbol::new(env, events::CONTRACT_UPGRADED_EVENT),
                new_wasm_hash.clone(),
            ),
            admin,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Internal method to register a merchant.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`EmptyAddress`](SettlementError::EmptyAddress) — if the provided merchant address is empty.
    /// * [`ZeroAddress`](SettlementError::ZeroAddress) — if the provided merchant address is the zero address.
    /// * [`InvalidAdmin`](SettlementError::InvalidAdmin) — if attempting to register an admin as a merchant.
    /// * [`MerchantExists`](SettlementError::MerchantExists) — if the merchant is already registered.
    fn _register_merchant(env: &Env, merchant: Address) {
        assert_not_paused(env);
        validate_nonzero_address(
            env,
            &merchant,
            SettlementError::EmptyAddress,
            SettlementError::ZeroAddress,
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
        env.events().publish(
            (
                Symbol::new(env, events::MERCHANT_REGISTERED_EVENT),
                merchant,
            ),
            admin,
        );
    }

    /// Internal method to unregister a merchant.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not currently registered.
    fn _unregister_merchant(env: &Env, merchant: Address) {
        assert_not_paused(env);
        let admin = read_admin(env);

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
            events::emit_settlement_rule_cleared(env, &merchant, &admin, &old_rule, &fallback);
        }

        env.events().publish(
            (
                Symbol::new(env, events::MERCHANT_UNREGISTERED_EVENT),
                merchant,
            ),
            admin,
        );
    }

    fn _set_settlement_rule(env: &Env, merchant: Address, rule: SettlementRule) {
        assert_not_paused(env);
        let admin = read_admin(env);

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
            (admin, prev, rule),
        );
    }

    fn _clear_settlement_rule(env: &Env, merchant: Address) {
        assert_not_paused(env);
        let admin = read_admin(env);

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
            (admin, removed, fallback),
        );
    }

    fn _set_default_rule(env: &Env, new_rule: SettlementRule) {
        assert_not_paused(env);
        let admin = read_admin(env);

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
            .persistent()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .persistent()
            .set(&DataKey::DefaultRule, &new_rule);
        env.storage().persistent().extend_ttl(
            &DataKey::DefaultRule,
            RULE_TTL_THRESHOLD,
            RULE_TTL_BUMP,
        );

        env.events().publish(
            (Symbol::new(env, events::DEFAULT_RULE_UPDATED_EVENT),),
            (admin, prev, new_rule),
        );
    }
}
