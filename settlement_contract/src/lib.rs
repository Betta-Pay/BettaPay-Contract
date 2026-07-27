//! # BettaPay Settlement Contract
//!
//! This module provides the core implementation of the settlement contract for BettaPay.
//! It is responsible for managing merchant registration, settlement rules, and the payment storage architecture.
//!
//! ## Merchant Rules
//!
//! The contract maintains a registry of authorized merchants. For each registered merchant,
//! an admin can configure specific settlement rules defined by the `SettlementRule` struct.
//! A settlement rule dictates:
//! - **Platform Fee (BPS)**: The fee collected by the platform.
//! - **Network Fee (BPS)**: The fee collected by the network.
//! - **Settlement Delay**: The delay in ledger sequences before a settlement can occur.
//! - **Auto-settle**: A flag indicating whether auto-settlement is enabled.
//!
//! If a merchant lacks a specific rule, the system falls back to an admin-configured global default rule,
//! and ultimately to a hardcoded bootstrap default rule if necessary.
//!
//! ## Payment Storage Architecture
//!
//! Payments are tracked and stored using a unique 32-byte reference (`BytesN<32>`).
//! When `store_payment_reference` is invoked, the contract calculates the exact fee split
//! (platform fee, network fee, and net merchant amount) based on the merchant's effective settlement rule.
//!
//! The resulting data is persisted in a `PaymentRecord`, which encapsulates:
//! - The calculated amounts and fee BPS.
//! - The ledger sequence of the transaction.
//! - Settlement delay and auto-settle configurations.
//!
//! The contract leverages different `DataKey` variants (`Admin`, `Merchant`, `Rule`, `Payment`, etc.)
//! to securely organize persistent and instance storage, while applying TTL extensions to ensure
//! active records remain available and do not expire prematurely.
//!
//! ## Upgrade Process
//!
//! [`SettlementContract::upgrade`] replaces the Wasm and nothing else. That is
//! what makes it safe, and also why changing a stored type is a separate
//! problem: nothing converts existing entries, and nothing checks that they
//! still match the types the new code expects. A mismatched read fails at
//! runtime, after the upgrade has already landed.
//!
//! 1. Wasm upgrades replace code only; every storage entry survives untouched.
//! 2. Storage migrations run **inside the upgraded contract**, as an
//!    admin-gated `migrate` entry point — not from a separate migration
//!    contract. A contract can only reach its own storage, so another contract
//!    has no access path to `Payment`, `Merchant` or `Rule` entries.
//! 3. Ship the old type definition in the same Wasm as the new one. A
//!    `#[contracttype]` struct is encoded by field name, so a `PaymentRecord`
//!    written before a new field existed will not deserialise into the new
//!    struct — the old type is what keeps those entries readable.
//! 4. Order is: upgrade the Wasm, then call `migrate`, then verify the
//!    post-upgrade state, then remove the migration code in a later upgrade.
//! 5. `Payment(BytesN<32>)`, `Merchant(Address)` and `Rule(Address)` are keyed
//!    by value and Soroban cannot enumerate storage keys — which is why
//!    [`SettlementContract::get_payments`] takes the references from the
//!    caller. Convert these lazily on read, or pass the keys in explicitly.
//! 6. Call `extend_ttl` on anything the migration rewrites: `set` alone does
//!    not extend an entry's life, so a migrated record would otherwise expire
//!    sooner than an untouched one.
//!
//! Full guidance, including worked examples and how to test a migration, is in
//! [`DEVELOPMENT.md`](https://github.com/Betta-Pay/BettaPay-Contract/blob/main/DEVELOPMENT.md).

// TODO: Refactor flat file structure into modular hierarchy (Issue #84)
// Intended module structure:
// - mod types: Data structures (enums, structs)
// - mod storage: DataKey and storage access helpers
// - mod events: Event definitions and emission helpers
// - mod errors: Error enums
// - mod contract: Main contract trait implementation
// - mod test: Unit and integration tests

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, String, Symbol, Val, Vec,
};
use soroban_sdk::testutils::storage::Persistent;

const BPS_DENOMINATOR: u32 = 10_000;
const MIN_FEE_BPS: u32 = 5; // Must match governance_contract::MIN_FEE_BPS
const MIN_PAYMENT_AMOUNT: i128 = 100;
const MAX_SETTLEMENT_DELAY_LEDGER: u32 = 100_000;
const PAYMENT_TTL_THRESHOLD: u32 = 17280 * 14;
const PAYMENT_TTL_BUMP: u32 = 17280 * 30;
const RULE_TTL_THRESHOLD: u32 = 17280 * 14;
const RULE_TTL_BUMP: u32 = 17280 * 30;
const RECOVERY_DELAY_SECONDS: u64 = 7 * 24 * 60 * 60;
const MERCHANT_TTL_THRESHOLD: u32 = 17280 * 14;
const MERCHANT_TTL_BUMP: u32 = 17280 * 30;

// Used until the admin sets a global default settlement rule.
const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
    platform_fee_bps: 100,
    network_fee_bps: 0,
    settlement_delay_ledger: 0,
    auto_settle: false,
};

/// Configuration governing how merchant payments are settled.
///
/// This struct defines the fee allocation and settlement timing for a merchant,
/// including the platform and network fee shares as well as whether
/// settlement is processed automatically after a delay.
#[derive(Clone)]
#[contracttype]
pub struct SettlementRule {
    /// Platform fee charged on each payment, expressed in basis points.
    ///
    /// One basis point is 0.01%, and 100 basis points equals 1%.
    /// This value is used when calculating the platform's share of a payment.
    pub platform_fee_bps: u32,
    /// Network fee charged on each payment, expressed in basis points.
    ///
    /// This represents the portion reserved for network or protocol-related
    /// costs and is combined with other fees as validated elsewhere in the contract.
    pub network_fee_bps: u32,
    /// Number of ledger closes to wait before settlement becomes eligible.
    ///
    /// A value of `0` enables immediate settlement, while larger values delay
    /// settlement until the specified number of ledgers has elapsed.
    pub settlement_delay_ledger: u32,
    /// Indicates whether settlement should occur automatically.
    ///
    /// When set to `true`, settlements may be processed automatically after
    /// the configured settlement delay has elapsed; when `false`, settlement
    /// requires manual or external triggering.
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeSplit {
    /// The total gross amount of the payment before any fees are deducted.
    /// Mirrors the input amount for caller convenience — not independently meaningful.
    pub gross_amount: i128,
    /// Portion of the settlement fee allocated to the platform.
    /// This amount is calculated by applying the platform fee basis points to the gross amount.
    pub platform_fee_amount: i128,
    /// Portion of the settlement fee allocated to the network.
    /// This amount is calculated by applying the network fee basis points to the gross amount.
    pub network_fee_amount: i128,
    /// Net amount allocated to the merchant.
    /// This derived output is calculated as the gross amount minus the rounded platform and network fee amounts.
    pub merchant_amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct PaymentRecord {
    /// The total gross amount of the payment processed.
    /// Set upon payment creation and used to derive the fee split.
    pub amount: i128,
    /// The exact amount deducted for the platform fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub platform_fee_amount: i128,
    /// The exact amount deducted for the network fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub network_fee_amount: i128,
    /// The net payout amount owed to the merchant.
    /// Calculated at payment creation to ensure deterministic settlement value.
    pub merchant_amount: i128,
    /// The platform fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub platform_fee_bps: u32,
    /// The network fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub network_fee_bps: u32,
    /// Ledger sequence timestamp when the payment was recorded.
    /// Used alongside settlement_delay_ledger to verify if the payment is ripe for settlement.
    pub ledger: u32,
    /// The delay period (in ledgers) before settlement can occur.
    /// Sourced from the active settlement rule and used to prevent premature settlement.
    pub settlement_delay_ledger: u32,
    /// Indicates if the payment should participate in automated settlement batches.
    /// Set from the active rule and used by external auto-settlement processes.
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct PendingRecovery {
    pub new_admin: Address,
    pub execute_after: u64,
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Admin,
    RecoveryAddress,
    PendingRecovery,
    Governance,
    Merchant(Address),
    Rule(Address),
    DefaultRule,
    Payment(BytesN<32>),
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementError {
    /// `init()` has already been called. Only one initialization is permitted.
    AlreadyInitialized = 1,
    /// `init()` has not been called. All admin-guarded functions require prior initialization.
    NotInitialized = 2,
    /// The caller does not match the stored admin address.
    Unauthorized = 3,
    /// `register_merchant` was called for an address that is already registered.
    MerchantExists = 4,
    /// The target merchant address is not registered. Raised by
    /// `set_settlement_rule`, `store_payment_reference`, `calculate_fee_split`,
    /// and `unregister_merchant` when the merchant is missing.
    MerchantMissing = 5,
    /// The fee BPS values exceed 10 000 (`BPS_DENOMINATOR`) or their sum
    /// exceeds 10 000. Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidFeeBps = 6,
    /// The payment amount is below `MIN_PAYMENT_AMOUNT` (100) or is ≤ 0
    /// in `calculate_fee_split`.
    InvalidAmount = 7,
    /// `store_payment_reference` was called with a 32‑byte reference that
    /// already exists in storage.
    DuplicatePaymentReference = 8,
    /// The contract is paused. Most state‑mutating operations are blocked.
    Paused = 9,
    /// No merchant-specific rule has been set. The merchant will use the default rule or bootstrap fallback.
    MerchantRuleNotSet = 10,
    /// The supplied address is the zero‑address or an empty string.
    /// Raised by `register_merchant` and `transfer_admin`.
    InvalidAddress = 11,
    /// `store_payment_reference` was called with an all‑zero 32‑byte
    /// reference, which is reserved.
    InvalidPaymentReference = 12,
    /// `settlement_delay_ledger` exceeds `MAX_SETTLEMENT_DELAY_LEDGER`
    /// (100 000). Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidSettlementDelay = 13,
    /// `transfer_admin` was called with the current admin address as the
    /// new admin. The new admin must be different.
    InvalidAdmin = 14,
    InvalidGovernance = 15,
    InvalidRecoveryAddress = 16,
    RecoveryNotPending = 17,
    RecoveryDelayActive = 18,
    /// The payment amount is large enough that multiplying it by a fee's
    /// basis points would overflow `i128`. Raised by `calculate_split`
    /// before the multiplication is attempted.
    AmountOverflow = 19,
}

#[contract]
pub struct SettlementContract;

#[contractimpl]
impl SettlementContract {
    /// Initialize the contract with the given admin address.
    ///
    /// # Panics
    ///
    /// * [`AlreadyInitialized`](SettlementError::AlreadyInitialized) — if the contract has already been initialized.
    pub fn init(env: Env, admin: Address, governance: Address, recovery_address: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::AlreadyInitialized);
        }
        admin.require_auth();
        validate_governance(&env, &governance);
        validate_nonzero_address(
            &env,
            &recovery_address,
            SettlementError::InvalidRecoveryAddress,
        );
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage()
            .instance()
            .set(&DataKey::RecoveryAddress, &recovery_address);
    }

    /// Return the current admin address.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    pub fn get_admin(env: Env) -> Address {
        read_admin(&env)
    }

    pub fn get_governance(env: Env) -> Address {
        read_governance(&env)
    }

    pub fn get_recovery_address(env: Env) -> Address {
        read_recovery_address(&env)
    }

    pub fn update_governance(env: Env, new_governance: Address) {
        let admin = read_admin(&env);
        admin.require_auth();
        assert_not_paused(&env);
        validate_governance(&env, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(&env, "governance_updated"),),
            (admin, new_governance),
        );
    }

    pub fn initiate_recovery(env: Env, new_admin: Address) {
        let recovery_address = read_recovery_address(&env);
        recovery_address.require_auth();
        validate_nonzero_address(&env, &new_admin, SettlementError::InvalidAdmin);

        let pending = PendingRecovery {
            new_admin: new_admin.clone(),
            execute_after: env.ledger().timestamp() + RECOVERY_DELAY_SECONDS,
        };
        env.storage()
            .instance()
            .set(&DataKey::PendingRecovery, &pending);
        env.events().publish(
            (Symbol::new(&env, "recovery_initiated"),),
            (recovery_address, new_admin, pending.execute_after),
        );
    }

    pub fn cancel_recovery(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        if !env.storage().instance().has(&DataKey::PendingRecovery) {
            panic_with_error!(&env, SettlementError::RecoveryNotPending);
        }
        env.storage().instance().remove(&DataKey::PendingRecovery);
        env.events()
            .publish((Symbol::new(&env, "recovery_cancelled"),), admin);
    }

    pub fn execute_recovery(env: Env) {
        let pending = read_pending_recovery(&env);
        if env.ledger().timestamp() < pending.execute_after {
            panic_with_error!(&env, SettlementError::RecoveryDelayActive);
        }

        env.storage()
            .instance()
            .set(&DataKey::Admin, &pending.new_admin);
        env.storage().instance().remove(&DataKey::PendingRecovery);
        env.events()
            .publish((Symbol::new(&env, "recovery_executed"),), pending.new_admin);
    }

    /// Transfer the admin role to a new address.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`InvalidAddress`](SettlementError::InvalidAddress) — if `new_admin` is the zero address.
    /// * [`InvalidAdmin`](SettlementError::InvalidAdmin) — if `new_admin` is the same as the current admin.
    ///
    /// ## Emitted Event: `admin`
    ///
    /// **Topics**: `(Symbol("admin"),)`
    /// **Data**: `Address new_admin`
    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin = read_admin(&env);
        admin.require_auth();

        validate_nonzero_address(&env, &new_admin, SettlementError::InvalidAddress);
        let zero_addr: Address = Address::from_string(&soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        if new_admin == zero_addr {
            panic_with_error!(&env, SettlementError::InvalidAddress);
        }

        if new_admin == admin {
            panic_with_error!(&env, SettlementError::InvalidAdmin);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish((symbol_short!("admin"),), new_admin);
    }

    /// Upgrades the underlying Wasm bytecode implementation of the contract under strict admin authority.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the registered admin.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let admin = read_admin(&env);
        admin.require_auth();

        env.events().publish(
            (Symbol::new(&env, "contract_upgraded"), new_wasm_hash.clone()),
            admin,
        );

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Pause the contract, preventing certain operations.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    ///
    /// ## Emitted Event: `pause`
    ///
    /// **Topics**: `(Symbol("pause"),)`
    /// **Data**: `bool true`
    pub fn pause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("pause"),), true);
    }

    /// Unpause the contract, re-enabling paused operations.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    ///
    /// ## Emitted Event: `unpause`
    ///
    /// **Topics**: `(Symbol("unpause"),)`
    /// **Data**: `bool false`
    pub fn unpause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("unpause"),), false);
    }

    /// Returns `true` if the contract is currently paused, `false` otherwise.
    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// ## Emitted Event: `merchant_registered`
    ///
    /// **Topics**: `(Symbol("merchant_registered"), Address merchant)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address that was registered
    ///
    /// **Data**: `Address caller`
    /// - `caller`: the admin who authorized the registration
    pub fn register_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);

        validate_nonzero_address(&env, &merchant, SettlementError::InvalidAddress);

        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
        env.events()
            .publish((Symbol::new(&env, "merchant_registered"), merchant), admin);
    }

    /// Remove a merchant from the registry and clear any associated settlement rule.
    ///
    /// Note: If a settlement rule exists for this merchant, it is silently
    /// removed without emitting a `settlement_rule_cleared` event.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    ///
    /// ## Emitted Event: `merchant_unregistered`
    ///
    /// **Topics**: `(Symbol("merchant_unregistered"), Address merchant)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address that was unregistered
    ///
    /// **Data**: `Address caller`
    /// - `caller`: the admin who authorized the unregistration
    pub fn unregister_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        let rule_key = DataKey::Rule(merchant.clone());
        if env.storage().persistent().has(&rule_key) {
            env.storage().persistent().remove(&rule_key);
        }

        env.events().publish(
            (Symbol::new(&env, "merchant_unregistered"), merchant),
            admin,
        );
    }

    /// ## Emitted Event: `settlement_rule_updated`
    ///
    /// **Topics**: `(Symbol("settlement_rule_updated"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was updated
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the rule change
    /// - `previous`: the rule values before the update (or system defaults on first set)
    /// - `current`: the new rule values after the update
    pub fn set_settlement_rule(env: Env, merchant: Address, rule: SettlementRule) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(&env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);

        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_updated"), merchant),
            (admin, prev, rule),
        );
    }

    /// ## Emitted Event: `settlement_rule_cleared`
    ///
    /// **Topics**: `(Symbol("settlement_rule_cleared"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was cleared
    ///
    /// **Data**: `(Address caller, SettlementRule removed, SettlementRule fallback)`
    /// - `caller`: the admin who authorized the removal
    /// - `removed`: the rule values that were removed from storage
    /// - `fallback`: the effective rule the merchant will use after clearing (global default or bootstrap)
    pub fn clear_settlement_rule(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Rule(merchant.clone());
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::MerchantRuleNotSet));

        env.storage().persistent().remove(&key);

        let fallback = read_rule_or_default(&env, merchant.clone());

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_cleared"), merchant),
            (admin, removed, fallback),
        );
    }

    /// ## Emitted Event: `default_rule_updated`
    ///
    /// **Topics**: `(Symbol("default_rule_updated"),)`
    /// - First topic: fixed event-name symbol for filtering by event type
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the change
    /// - `previous`: the previous global default rule (or bootstrap fallback if none was set)
    /// - `current`: the new global default rule
    /// ## Event: `default_rule_updated`
    ///
    /// Emitted when the global default settlement rule is updated.
    pub fn set_default_rule(env: Env, new_rule: SettlementRule) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if new_rule.platform_fee_bps > BPS_DENOMINATOR || new_rule.network_fee_bps > BPS_DENOMINATOR
        {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps < MIN_FEE_BPS || new_rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .persistent()
            .set(&DataKey::DefaultRule, &new_rule);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::DefaultRule, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (Symbol::new(&env, "default_rule_updated"),),
            (admin, prev, new_rule),
        );
    }

    /// Returns the global default settlement rule, if one has been set.
    /// Automatically extends the persistent storage TTL to prevent archival
    /// during public read queries (clausal to TTL eviction).
    pub fn get_default_rule(env: Env) -> Option<SettlementRule> {
        let key = DataKey::DefaultRule;
        match env.storage().persistent().get::<_, SettlementRule>(&key) {
            Some(rule) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
                Some(rule)
            }
            None => None,
        }
    }

    /// Store a payment reference for a merchant and calculate the fee split.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`InvalidPaymentReference`](SettlementError::InvalidPaymentReference) — if `reference` is all zeros.
    /// * [`InvalidAmount`](SettlementError::InvalidAmount) — if `amount` is below the minimum.
    /// * [`DuplicatePaymentReference`](SettlementError::DuplicatePaymentReference) — if the reference already exists.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    ///
    /// ## Emitted Event: `payment_stored`
    ///
    /// **Topics**: `(Symbol("payment_stored"), Address merchant, BytesN<32> reference)`
    /// **Data**: `()`
    ///
    /// The fee split (platform fee, network fee, merchant amount, gross amount)
    /// is available on the `PaymentRecord` in this event's data; no separate
    /// split event is emitted.
    pub fn store_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        amount: i128,
    ) -> FeeSplit {
        assert_not_paused(&env);
        merchant.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if reference == BytesN::from_array(&env, &[0; 32]) {
            panic_with_error!(&env, SettlementError::InvalidPaymentReference);
        }
        if amount < MIN_PAYMENT_AMOUNT {
            panic_with_error!(&env, SettlementError::InvalidAmount);
        }

        let payment_key = DataKey::Payment(reference.clone());
        if env.storage().persistent().has(&payment_key) {
            panic_with_error!(&env, SettlementError::DuplicatePaymentReference);
        }

        let rule = read_rule_or_default(&env, merchant.clone());
        let split = calculate_split(&env, amount, &rule);
        let record = PaymentRecord {
            amount,
            platform_fee_amount: split.platform_fee_amount,
            network_fee_amount: split.network_fee_amount,
            merchant_amount: split.merchant_amount,
            platform_fee_bps: rule.platform_fee_bps,
            network_fee_bps: rule.network_fee_bps,
            ledger: env.ledger().sequence(),
            settlement_delay_ledger: rule.settlement_delay_ledger,
            auto_settle: rule.auto_settle,
        };

        env.storage().persistent().set(&payment_key, &record);
        env.storage().persistent().extend_ttl(
            &payment_key,
            PAYMENT_TTL_THRESHOLD,
            PAYMENT_TTL_BUMP,
        );

        env.events().publish(
            (Symbol::new(&env, "payment_stored"), merchant.clone(), reference.clone()),
            (),
        );

        split
    }

    /// Returns `true` if the given address is a registered merchant, `false` otherwise.
    pub fn is_merchant_registered(env: Env, merchant: Address) -> bool {
        is_merchant_registered_internal(&env, merchant)
    }

    /// Returns the merchant-specific settlement rule, if one has been set.
    /// Automatically extends the persistent storage TTL to prevent archival.
    pub fn get_settlement_rule(env: Env, merchant: Address) -> Option<SettlementRule> {
        let key = DataKey::Rule(merchant);

        if let Some(rule) = env.storage().persistent().get(&key) {
            // Extend the TTL using the same named constants as set_settlement_rule
            // so the read and write paths never drift apart if the policy changes.
            env.storage()
                .persistent()
                .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

            Some(rule)
        } else {
            None
        }
    }

    /// Calculate the fee split for a given merchant and amount without storing a payment reference.
    ///
    /// # Panics
    ///
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`InvalidAmount`](SettlementError::InvalidAmount) — if `amount` is zero or negative.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if amount <= 0 {
            panic_with_error!(&env, SettlementError::InvalidAmount);
        }
        let rule = read_rule_or_default(&env, merchant);
        calculate_split(&env, amount, &rule)
    }

    /// Retrieve a payment record by its reference, extending the storage TTL if found.
    pub fn get_payment_reference(env: Env, reference: BytesN<32>) -> Option<PaymentRecord> {
        let key = DataKey::Payment(reference);
        let record: Option<PaymentRecord> = env.storage().persistent().get(&key);
        if record.is_some() {
            let ttl = env.storage().persistent().get_ttl(&key);
            if ttl < PAYMENT_TTL_THRESHOLD {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
            }
        }
        record
    }

    /// Retrieve multiple payment records by a vector of references.
    ///
    /// Missing references are silently skipped.
    pub fn get_payments(env: Env, references: Vec<BytesN<32>>) -> Vec<PaymentRecord> {
        // `references.len()` is known upfront, so pre-allocating would avoid repeated
        // reallocation as this vector grows. soroban-sdk 21.7.7's Vec<T> has no
        // `with_capacity` constructor (only `new`, `from_array`, `from_slice`), so
        // this is left as a potential optimization for a future SDK version.
        let mut payments = Vec::new(&env);

        for reference in references.iter() {
            if let Some(payment) = Self::get_payment_reference(env.clone(), reference.clone()) {
                payments.push_back(payment);
            }
        }

        payments
    }
}

/// Reads the configured admin address and refreshes the instance TTL so it remains available.
fn read_admin(env: &Env) -> Address {
    env.storage().instance().extend_ttl(50_000, 100_000);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_governance(env: &Env) -> Address {
    env.storage().instance().extend_ttl(50_000, 100_000);
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_recovery_address(env: &Env) -> Address {
    env.storage().instance().extend_ttl(50_000, 100_000);
    env.storage()
        .instance()
        .get(&DataKey::RecoveryAddress)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_pending_recovery(env: &Env) -> PendingRecovery {
    env.storage()
        .instance()
        .get(&DataKey::PendingRecovery)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::RecoveryNotPending))
}

fn validate_governance(env: &Env, governance: &Address) {
    validate_nonzero_address(env, governance, SettlementError::InvalidGovernance);
    let args: Vec<Val> = Vec::new(env);
    let _: Option<FeeConfig> =
        env.invoke_contract(governance, &Symbol::new(env, "get_fee_config"), args);
}

fn validate_nonzero_address(env: &Env, address: &Address, error: SettlementError) {
    let zero_address = String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    );
    if address.to_string().len() == 0 || address.to_string() == zero_address {
        panic_with_error!(env, error);
    }
}

/// Returns whether a merchant has been registered and keeps the marker entry warm in storage.
fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    if env.storage().persistent().has(&key) {
        // Keep the merchant marker warm so active merchants do not expire early.
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
    }
    env.storage().persistent().get(&key).unwrap_or(false)
}

/// Resolves the effective settlement rule for a merchant by preferring the merchant-specific override,
/// then falling back to the global default, and finally using the bootstrap fallback.
fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
    // Merchant-specific rule wins over any shared configuration.
    let merchant_key = DataKey::Rule(merchant);
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&merchant_key)
    {
        env.storage()
            .persistent()
            .extend_ttl(&merchant_key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
        return rule;
    }
    // Fall back to the admin-controlled global default when present.
    let default_key = DataKey::DefaultRule;
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&default_key)
    {
        env.storage()
            .persistent()
            .extend_ttl(&default_key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
        return rule;
    }
    // Final fallback keeps the contract usable before any config is stored.
    env.events().publish(
        (Symbol::new(env, "bootstrap_fallback"),),
        BOOTSTRAP_DEFAULT_RULE,
    );
    BOOTSTRAP_DEFAULT_RULE
}

/// Returns whether the contract is currently paused.
fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// Ensures the contract is not paused before mutating state or performing privileged actions.
fn assert_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

/// Computes the platform, network, and merchant fee amounts for an amount using ceil-based rounding.
fn calculate_split(env: &Env, amount: i128, rule: &SettlementRule) -> FeeSplit {
    let denom = BPS_DENOMINATOR as i128;

    // Guard against `amount * bps + (denom - 1)` overflowing i128 before it is attempted below,
    // so callers get a readable AmountOverflow error instead of a raw arithmetic-overflow panic.
    // The `denom - 1` term (the ceil-rounding adjustment) is subtracted from the budget up front
    // so the check stays exact at the boundary instead of leaving a narrow window where the
    // multiplication is "safe" but the following `+ denom - 1` still overflows.
    let max_bps = core::cmp::max(rule.platform_fee_bps, rule.network_fee_bps) as i128;
    if max_bps > 0 && amount > (i128::MAX - (denom - 1)) / max_bps {
        panic_with_error!(env, SettlementError::AmountOverflow);
    }

    // Integer arithmetic is used instead of floats to ensure deterministic, reproducible smart contract execution.
    // Standard integer division (`/`) truncates fractions toward zero, causing precision loss and under-collecting fees.
    // To prevent fee under-collection, ceiling division is simulated by adding `BPS_DENOMINATOR - 1` to the numerator.
    // Edge case: For small amounts, ceil rounding can force fees to 1 unit even when the basis points represent a tiny fraction.
    let platform_fee_amount = (amount * (rule.platform_fee_bps as i128) + denom - 1) / denom;
    let network_fee_amount = (amount * (rule.network_fee_bps as i128) + denom - 1) / denom;

    // The merchant amount is calculated as the subtraction remainder of the gross amount minus all rounded-up fees.
    // This ensures the sum of the split amounts (platform fee + network fee + merchant share) always equals the gross amount.
    // Consequence: The merchant absorbs all rounding dust. For very small gross amounts with high/extreme fee percentages,
    // the sum of rounded-up fees can exceed the gross amount, resulting in a negative merchant payout.
    let merchant_amount = amount - platform_fee_amount - network_fee_amount;
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events, Ledger, MockAuth, MockAuthInvoke};
    use soroban_sdk::{FromVal, IntoVal};

    #[contract]
    struct MockGovernance;

    #[contractimpl]
    impl MockGovernance {
        pub fn get_fee_config(_env: Env) -> Option<FeeConfig> {
            None
        }
    }

    fn register_governance(env: &Env) -> Address {
        env.register_contract(None, MockGovernance)
    }

    fn setup() -> (Env, SettlementContractClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let recovery_address = Address::generate(&env);
        let merchant = Address::generate(&env);
        let governance = register_governance(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);
        client.init(&admin, &governance, &recovery_address);
        (env, client, admin, merchant)
    }

    #[test]
    fn executes_contract_wasm_upgrade_successfully() {
        let (env, client, admin, _) = setup();
        let wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
        let new_wasm_hash = env.deployer().upload_contract_wasm(wasm);
        
        let before = env.events().all().len();
        // Verifies the structural update pass completes without panicking
        client.upgrade(&new_wasm_hash);
        
        let events = env.events().all();
        assert!(events.len() > before);
        
        let event = events.last().unwrap();
        let (_contract_id, topics, data) = event;
        
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "contract_upgraded")
        );
        assert_eq!(
            BytesN::<32>::from_val(&env, &topics.get(1).unwrap()),
            new_wasm_hash
        );
        assert_eq!(Address::from_val(&env, &data), admin);

        // Ensure the upgraded contract remains callable and retains its state.
        let upgraded_client = SettlementContractClient::new(&env, &client.address);
        assert_eq!(upgraded_client.get_admin(), admin);
    }

    #[test]
    fn emits_event_on_initialization() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        client.init(&admin, &governance, &recovery_address);

        let events = env.events().all();
        assert_eq!(events.len(), 1, "exactly one event emitted on init");

        let (_contract_id, topics, data) = events.get(0).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "initialized")
        );
        assert_eq!(Address::from_val(&env, &data), admin);
    }

    #[test]
    #[should_panic]
    fn rejects_double_initialization() {
        let (env, client, admin, _) = setup();
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        client.init(&admin, &governance, &recovery_address);
        let _ = env;
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn get_admin_panics_before_init() {
        let env = Env::default();
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);
        client.get_admin();
    }

    #[test]
    fn registers_merchant_and_persists_flag() {
        let (env, client, _admin, merchant) = setup();
        let before = env.events().all().len();
        client.register_merchant(&merchant);
        assert!(client.is_merchant_registered(&merchant));
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn update_governance_stores_validated_address() {
        let (env, client, _admin, _merchant) = setup();
        let new_governance = register_governance(&env);

        client.update_governance(&new_governance);

        assert_eq!(client.get_governance(), new_governance);
    }

    #[test]
    fn recovery_executes_after_delay() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let recovery_address = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let governance = register_governance(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        client.init(&admin, &governance, &recovery_address);
        assert_eq!(client.get_recovery_address(), recovery_address);

        client.initiate_recovery(&new_admin);
        env.ledger()
            .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
        client.execute_recovery();

        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn emits_event_on_registration() {
        let (env, client, admin, merchant) = setup();

        client.register_merchant(&merchant);

        let events = env.events().all();
        let event = events.last().unwrap();
        let (_contract_id, topics, data) = event;

        // Topic 0: Event Name symbol
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "merchant_registered")
        );
        // Topic 1: Merchant Address
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
        // Data: Admin Address (the caller)
        assert_eq!(Address::from_val(&env, &data), admin);
    }

    #[test]
    #[should_panic]
    fn rejects_invalid_merchant_address() {
        let (env, client, _admin, _merchant) = setup();
        let zero_address = Address::from_string(&soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        client.register_merchant(&zero_address);
    }

    #[test]
    #[should_panic]
    fn rejects_zero_address_admin_transfer() {
        let (env, client, _admin, _merchant) = setup();
        let zero_address = Address::from_string(&soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        client.transfer_admin(&zero_address);
    }

    #[test]
    fn extends_ttl_when_updating_settlement_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        // This will successfully write and extend the TTL for the rule
        client.set_settlement_rule(&merchant, &rule);

        // Verify the persistent entry exists
        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            assert!(env.storage().persistent().has(&key));
        });
    }

    #[test]
    fn set_default_rule_extends_ttl() {
        let (env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 100,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        client.set_default_rule(&rule);

        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            assert!(env.storage().persistent().has(&key));
            let ttl = env.storage().persistent().get_ttl(&key);
            assert!(
                ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
                "TTL must be extended to at least ledger + RULE_TTL_BUMP"
            );
        });
    }

    // Issue #252: the TTL must be refreshed on every write to the default rule,
    // not just the first one — otherwise a rarely-updated (but frequently-read)
    // default rule could still expire between updates.
    #[test]
    fn set_default_rule_extends_ttl_on_update() {
        let (env, client, _admin, _merchant) = setup();

        let first_rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 100,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        client.set_default_rule(&first_rule);

        // Advance the ledger past RULE_TTL_THRESHOLD so the remaining TTL from
        // the first call drops below the threshold and a second write is
        // actually required to bump it back up (extend_ttl is a no-op while
        // the remaining TTL is still above the threshold). Advance in smaller
        // hops, touching the contract via get_admin() between hops, so the
        // instance's own (much shorter) TTL doesn't expire along the way.
        for _ in 0..5 {
            env.ledger().set_sequence_number(env.ledger().sequence() + 60_000);
            client.get_admin();
        }

        let second_rule = SettlementRule {
            platform_fee_bps: 400,
            network_fee_bps: 150,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };
        client.set_default_rule(&second_rule);

        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            let ttl = env.storage().persistent().get_ttl(&key);
            assert!(
                ttl >= RULE_TTL_BUMP,
                "TTL must be refreshed to at least RULE_TTL_BUMP on every write, not just the first"
            );
        });
    }

    #[test]
    fn store_payment_reference_extends_rule_ttl_on_read() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        env.ledger().set_sequence_number(env.ledger().sequence() + 1000);

        let reference = BytesN::from_array(&env, &[42; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);

        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            assert!(env.storage().persistent().has(&key));
            let ttl = env.storage().persistent().get_ttl(&key);
            assert!(
                ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
                "Merchant Rule TTL must be extended on read"
            );
        });
    }

    #[test]
    fn calculate_fee_split_extends_default_rule_ttl_on_read() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        env.ledger().set_sequence_number(env.ledger().sequence() + 1000);

        client.calculate_fee_split(&merchant, &50_000);

        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            assert!(env.storage().persistent().has(&key));
            let ttl = env.storage().persistent().get_ttl(&key);
            assert!(
                ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
                "DefaultRule TTL must be extended on read"
            );
        });
    }

    #[test]
    fn get_default_rule_extends_ttl_on_read() {
        let (env, client, _admin, _merchant) = setup();

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        env.ledger().set_sequence_number(env.ledger().sequence() + 1000);

        let retrieved = client.get_default_rule();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().platform_fee_bps, 200);

        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            assert!(env.storage().persistent().has(&key));
            let ttl = env.storage().persistent().get_ttl(&key);
            assert!(
                ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
                "DefaultRule TTL must be extended on public read via get_default_rule"
            );
        });
    }

    #[test]
    fn sets_and_reads_settlement_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: true,
        };

        let prev_count = env.events().all().len();
        client.set_settlement_rule(&merchant, &rule);
        let got = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");

        assert_eq!(got.platform_fee_bps, 175);
        assert_eq!(got.network_fee_bps, 25);
        assert_eq!(got.settlement_delay_ledger, 42);
        assert!(got.auto_settle);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

        // Topic[0] must be the fixed event-name symbol
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        // Topic[1] must be the merchant (rule identifier)
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
    }

    #[test]
    fn emits_structured_event_when_updating_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let first_rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &first_rule);

        let second_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 20,
            auto_settle: true,
        };

        let prev_count = env.events().all().len();
        client.set_settlement_rule(&merchant, &second_rule);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

        // Topic[0] must be the fixed event-name symbol
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        // Topic[1] must be the merchant
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

        // Verify storage was updated
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.platform_fee_bps, 200);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.settlement_delay_ledger, 20);
        assert!(stored.auto_settle);
    }

    #[test]
    fn stores_payment_reference_once_and_calculates_split() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[7; 32]);
        let before = env.events().all().len();
        let split = client.store_payment_reference(&merchant, &reference, &20_000);
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 100);
        assert_eq!(split.merchant_amount, 19_400);
        assert_eq!(stored.platform_fee_bps, 250);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.amount, 20_000);
        assert!(env.events().all().len() >= before + 2);
    }

    #[test]
    #[should_panic]
    fn rejects_all_zero_payment_reference() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[0; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);
    }

    #[test]
    fn reads_payment_reference_and_extends_ttl() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[8; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);

        // Call get_payment_reference, which should extend the TTL
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(stored.amount, 10_000);

        // Verify the persistent entry exists after read
        env.as_contract(&client.address, || {
            let key = DataKey::Payment(reference.clone());
            assert!(env.storage().persistent().has(&key));
        });
    }

    #[test]
    fn get_payment_reference_returns_none_for_unknown() {
        let (env, client, _, _) = setup();
        let unknown_ref = BytesN::from_array(&env, &[0xab; 32]);
        let result = client.get_payment_reference(&unknown_ref);
        assert!(result.is_none());
    }

    #[test]
    fn gets_payments_in_batches() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference_one = BytesN::from_array(&env, &[11; 32]);
        let reference_two = BytesN::from_array(&env, &[12; 32]);
        client.store_payment_reference(&merchant, &reference_one, &15_000);
        client.store_payment_reference(&merchant, &reference_two, &25_000);

        let references = Vec::from_array(&env, [reference_one.clone(), reference_two.clone()]);
        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), 2);
        assert_eq!(payments.get(0).unwrap().amount, 15_000);
        assert_eq!(payments.get(1).unwrap().amount, 25_000);
    }

    // Issue #298: verify get_payments returns an empty vector when given an empty input vector.
    #[test]
    fn get_payments_with_empty_input_vector_returns_empty_vector() {
        let (env, client, _admin, _merchant) = setup();
        let references = Vec::new(&env);
        let payments = client.get_payments(&references);
        assert_eq!(payments.len(), 0);
    }

    // Issue #299: verify get_payments returns an empty vector when all requested references are missing from storage.
    #[test]
    fn get_payments_with_all_missing_references_returns_empty_vector() {
        let (env, client, _admin, _merchant) = setup();
        let missing_one = BytesN::from_array(&env, &[90; 32]);
        let missing_two = BytesN::from_array(&env, &[91; 32]);
        let references = Vec::from_array(&env, [missing_one, missing_two]);
        let payments = client.get_payments(&references);
        assert_eq!(payments.len(), 0);
    }

    // Issue #300: verify get_payments correctly filters out missing references and returns records for valid ones.
    #[test]
    fn get_payments_with_mixed_valid_and_missing_references() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let valid_one = BytesN::from_array(&env, &[80; 32]);
        let valid_two = BytesN::from_array(&env, &[81; 32]);
        let missing_ref = BytesN::from_array(&env, &[82; 32]);

        client.store_payment_reference(&merchant, &valid_one, &10_000);
        client.store_payment_reference(&merchant, &valid_two, &20_000);

        // Query with: [valid_one, missing_ref, valid_two]
        let references = Vec::from_array(&env, [valid_one.clone(), missing_ref, valid_two.clone()]);
        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), 2);
        assert_eq!(payments.get(0).unwrap().amount, 10_000);
        assert_eq!(payments.get(1).unwrap().amount, 20_000);
    }

    // Issue #340: get_payments must still return every requested payment, in the
    // requested order, for a batch large enough to trigger multiple Vec growths
    // (regardless of whether the underlying Vec is pre-allocated).
    #[test]
    fn get_payments_returns_all_records_in_order_for_large_batch() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        const BATCH_SIZE: u8 = 20;
        let mut references = Vec::new(&env);
        for i in 1..=BATCH_SIZE {
            let reference = BytesN::from_array(&env, &[i; 32]);
            let amount = MIN_PAYMENT_AMOUNT + i as i128;
            client.store_payment_reference(&merchant, &reference, &amount);
            references.push_back(reference);
        }

        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), BATCH_SIZE as u32);
        for i in 1..=BATCH_SIZE {
            assert_eq!(
                payments.get((i - 1) as u32).unwrap().amount,
                MIN_PAYMENT_AMOUNT + i as i128
            );
        }
    }

    #[test]
    fn calculates_split_without_storing_reference() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500); // Because default is 100 bps
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);
    }

    #[test]
    #[should_panic]
    fn rejects_duplicate_merchant() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        client.register_merchant(&merchant);
    }

    #[test]
    fn unregisters_merchant_and_cleans_up() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        assert!(client.is_merchant_registered(&merchant));
        assert!(client.get_settlement_rule(&merchant).is_some());

        let before = env.events().all().len();
        client.unregister_merchant(&merchant);

        assert!(!client.is_merchant_registered(&merchant));
        assert!(client.get_settlement_rule(&merchant).is_none());
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn emits_structured_event_when_unregistering_merchant() {
        let (env, client, admin, merchant) = setup();
        client.register_merchant(&merchant);

        client.unregister_merchant(&merchant);

        let events = env.events().all();
        let event = events.last().unwrap();
        let (_contract_id, topics, data) = event;

        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "merchant_unregistered")
        );
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
        assert_eq!(Address::from_val(&env, &data), admin);
    }

    #[test]
    #[should_panic]
    fn unregister_rejects_missing_merchant() {
        let (_env, client, _admin, merchant) = setup();
        client.unregister_merchant(&merchant);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #8)")]
    fn rejects_duplicate_payment_reference() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[1; 32]);
        client.store_payment_reference(&merchant, &reference, &1000);
        client.store_payment_reference(&merchant, &reference, &2000);
    }

    #[test]
    #[should_panic]
    fn rejects_invalid_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[2; 32]);
        client.store_payment_reference(&merchant, &reference, &0);
    }

    #[test]
    #[should_panic]
    fn rejects_below_minimum_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[99; 32]);
        client.store_payment_reference(&merchant, &reference, &99);
    }

    #[test]
    fn accepts_valid_minimum_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[100; 32]);
        client.store_payment_reference(&merchant, &reference, &100);

        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");
        assert_eq!(stored.amount, 100);
    }

    // Issue #297: verify store_payment_reference succeeds when amount = MIN_PAYMENT_AMOUNT (100)
    // combined with a platform_fee_bps of 10_000 (100%). The contract must accept the call since
    // the amount meets the minimum threshold. With ceiling-based fee arithmetic, the platform fee
    // consumes the entire gross amount (100 bps * 100 / 10_000 rounded up = 100), leaving the
    // merchant with exactly 0. This documents the known edge case: at extreme fee rates and the
    // minimum payment amount, the merchant payout is zero.
    #[test]
    fn store_payment_reference_min_amount_with_maximum_platform_fee_yields_zero_merchant_payout() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        // Set a rule with platform_fee_bps = 10_000 (100%) and no network fee.
        let rule = SettlementRule {
            platform_fee_bps: 10_000,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        // Use a distinct reference (different from [100; 32] used in accepts_valid_minimum_amount).
        let reference = BytesN::from_array(&env, &[101; 32]);

        // store_payment_reference must succeed: amount = 100 satisfies the MIN_PAYMENT_AMOUNT check.
        let split = client.store_payment_reference(&merchant, &reference, &100);

        // With 100% platform fee and ceiling arithmetic:
        //   platform_fee_amount = ceil(100 * 10_000 / 10_000) = 100
        //   network_fee_amount  = 0
        //   merchant_amount     = 100 - 100 - 0 = 0
        assert_eq!(
            split.gross_amount, 100,
            "gross amount must equal the submitted payment amount"
        );
        assert_eq!(
            split.platform_fee_amount, 100,
            "platform fee must absorb the entire gross amount at 100% fee rate"
        );
        assert_eq!(
            split.network_fee_amount, 0,
            "network fee must be zero when network_fee_bps is 0"
        );
        assert_eq!(
            split.merchant_amount, 0,
            "merchant payout must be exactly 0 when fees consume the full gross amount"
        );

        // Confirm the stored record reflects the same computed values.
        let stored = client
            .get_payment_reference(&reference)
            .expect("payment record must be present after successful store");
        assert_eq!(stored.amount, 100);
        assert_eq!(stored.platform_fee_amount, 100);
        assert_eq!(stored.network_fee_amount, 0);
        assert_eq!(stored.merchant_amount, 0);
        assert_eq!(stored.platform_fee_bps, 10_000);
        assert_eq!(stored.network_fee_bps, 0);
    }

    #[test]
    #[should_panic]
    fn rejects_invalid_fee_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let bad_rule = SettlementRule {
            platform_fee_bps: 10_001,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    #[test]
    #[should_panic]
    fn rejects_settlement_rule_below_governance_min_fee() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let bad_rule = SettlementRule {
            platform_fee_bps: 4,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_sum_exceeding_10000_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let bad_rule = SettlementRule {
            platform_fee_bps: 6_000,
            network_fee_bps: 5_000,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    #[test]
    fn accepts_fee_sum_at_exactly_10000_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_000,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.platform_fee_bps, 5_000);
        assert_eq!(stored.network_fee_bps, 5_000);
    }

    #[test]
    fn admin_clears_custom_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: true,
        };
        client.set_settlement_rule(&merchant, &rule);

        client.clear_settlement_rule(&merchant);

        // Storage key is gone: getter returns None
        assert!(client.get_settlement_rule(&merchant).is_none());

        // find the settlement_rule_cleared event and verify its data
        let events = env.events().all();
        let cleared_event = events
            .iter()
            .rev()
            .find(|(_id, topics, _data)| {
                topics.len() >= 2
                    && Symbol::from_val(&env, &topics.get(0).unwrap())
                        == Symbol::new(&env, "settlement_rule_cleared")
                    && Address::from_val(&env, &topics.get(1).unwrap()) == merchant
            })
            .expect("expected settlement_rule_cleared event");
        let (_contract_id, _topics, data) = cleared_event;

        let (admin_addr, removed, fallback): (Address, SettlementRule, SettlementRule) =
            FromVal::from_val(&env, &data);
        assert_eq!(admin_addr, _admin);
        assert_eq!(removed.platform_fee_bps, rule.platform_fee_bps);
        assert_eq!(removed.network_fee_bps, rule.network_fee_bps);
        assert_eq!(removed.settlement_delay_ledger, rule.settlement_delay_ledger);
        assert_eq!(removed.auto_settle, rule.auto_settle);
        assert_eq!(fallback.platform_fee_bps, BOOTSTRAP_DEFAULT_RULE.platform_fee_bps);
        assert_eq!(fallback.network_fee_bps, BOOTSTRAP_DEFAULT_RULE.network_fee_bps);
        assert_eq!(fallback.settlement_delay_ledger, BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger);
        assert_eq!(fallback.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
    }

    #[test]
    fn clearing_rule_falls_back_to_defaults() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 200,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_settlement_rule(&merchant, &rule);

        client.clear_settlement_rule(&merchant);

        // calculate_fee_split should now use default rates (100 bps platform, 0 bps network)
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500); // 100 bps of 50_000
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);

        // find the settlement_rule_cleared event and verify its data
        let events = env.events().all();
        let cleared_event = events
            .iter()
            .rev()
            .find(|(_id, topics, _data)| {
                topics.len() >= 2
                    && Symbol::from_val(&env, &topics.get(0).unwrap())
                        == Symbol::new(&env, "settlement_rule_cleared")
                    && Address::from_val(&env, &topics.get(1).unwrap()) == merchant
            })
            .expect("expected settlement_rule_cleared event");
        let (_contract_id, _topics, data) = cleared_event;

        let (_caller, removed, fallback): (Address, SettlementRule, SettlementRule) =
            FromVal::from_val(&env, &data);
        assert_eq!(removed.platform_fee_bps, rule.platform_fee_bps);
        assert_eq!(removed.network_fee_bps, rule.network_fee_bps);
        assert_eq!(removed.settlement_delay_ledger, rule.settlement_delay_ledger);
        assert_eq!(removed.auto_settle, rule.auto_settle);
        assert_eq!(fallback.platform_fee_bps, BOOTSTRAP_DEFAULT_RULE.platform_fee_bps);
        assert_eq!(fallback.network_fee_bps, BOOTSTRAP_DEFAULT_RULE.network_fee_bps);
        assert_eq!(fallback.settlement_delay_ledger, BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger);
        assert_eq!(fallback.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
    }

    #[test]
    #[should_panic]
    fn clear_settlement_rule_fails_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        let contract_id: Address = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        // Authorize admin for init
        let invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: soroban_sdk::vec![
                &env,
                admin.to_val(),
                governance.to_val(),
                recovery_address.to_val()
            ],
            sub_invokes: &[],
        };
        let auth = MockAuth {
            address: &admin,
            invoke: &invoke,
        };
        env.set_auths(&[(&auth).into()]);
        client.init(&admin, &governance, &recovery_address);

        // Authorize admin for register_merchant
        let reg_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_merchant",
            args: soroban_sdk::vec![&env, merchant.to_val()],
            sub_invokes: &[],
        };
        let reg_auth = MockAuth {
            address: &admin,
            invoke: &reg_invoke,
        };
        env.set_auths(&[(&reg_auth).into()]);
        client.register_merchant(&merchant);

        // Do NOT authorize admin for clear_settlement_rule — should panic
        client.clear_settlement_rule(&merchant);
    }

    #[test]
    #[should_panic]
    fn clear_settlement_rule_fails_when_not_set() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        client.clear_settlement_rule(&merchant);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #10)")]
    fn clear_settlement_rule_fails_after_unregister_removes_rule() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        assert!(client.get_settlement_rule(&merchant).is_some());

        // Unregister silently removes the merchant-specific rule.
        client.unregister_merchant(&merchant);
        assert!(client.get_settlement_rule(&merchant).is_none());

        // The rule no longer exists, so clear_settlement_rule must panic with RuleNotSet.
        client.clear_settlement_rule(&merchant);
    }

    #[test]
    fn bootstrap_default_used_before_any_default_rule_set() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        // No global default set — falls back to hardcoded 100 bps
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);
    }

    #[test]
    fn bootstrap_fallback_emits_event_and_matches_bootstrap_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let before = env.events().all().len();
        let split = client.calculate_fee_split(&merchant, &50_000);

        // Verify the returned rule matches BOOTSTRAP_DEFAULT_RULE
        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);

        // Verify bootstrap_fallback event was emitted
        let events = env.events().all();
        assert!(
            events.len() > before,
            "at least one event expected from bootstrap fallback"
        );

        let fallback_event = events
            .iter()
            .skip(before as usize)
            .find(|(_id, topics, _data)| {
                !topics.is_empty()
                    && Symbol::from_val(&env, &topics.get(0).unwrap())
                        == Symbol::new(&env, "bootstrap_fallback")
            })
            .expect("expected bootstrap_fallback event to be emitted");

        let (_id, topics, data) = fallback_event;
        assert_eq!(topics.len(), 1);
        let emitted: SettlementRule = FromVal::from_val(&env, &data);
        assert_eq!(emitted.platform_fee_bps, 100);
        assert_eq!(emitted.network_fee_bps, 0);
        assert_eq!(emitted.settlement_delay_ledger, 0);
        assert!(!emitted.auto_settle);
    }

    // Regression coverage for read_rule_or_default's single-`get()`-per-key lookup
    // (see #264): each branch must resolve with exactly one storage read for the
    // key it needs, without probing or extending the TTL of the other rule key.

    #[test]
    fn read_rule_or_default_short_circuits_on_merchant_rule_without_touching_default() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let default_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&default_rule);

        let merchant_rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 75,
            settlement_delay_ledger: 20,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &merchant_rule);

        // Capture DefaultRule's absolute expiration ledger (sequence + remaining TTL)
        // rather than the raw remaining-TTL count, since the count alone decays with
        // every ledger that passes regardless of whether the entry was touched.
        let default_expiration_before = env.as_contract(&client.address, || {
            env.ledger().sequence() + env.storage().persistent().get_ttl(&DataKey::DefaultRule)
        });

        // Keep the contract instance itself alive across the large jump below —
        // otherwise it would archive first and mask the assertions this test cares about.
        env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .extend_ttl(RULE_TTL_THRESHOLD + 200_000, RULE_TTL_THRESHOLD + 1_000_000);
        });

        // Advance the ledger past RULE_TTL_THRESHOLD so the merchant rule's remaining
        // TTL actually falls below the threshold and a real extend_ttl bump is
        // triggered on read (also making a spurious bump on DefaultRule observable).
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + RULE_TTL_THRESHOLD + 50_000);

        let resolved = env.as_contract(&client.address, || {
            read_rule_or_default(&env, merchant.clone())
        });
        assert_eq!(resolved.platform_fee_bps, merchant_rule.platform_fee_bps);
        assert_eq!(resolved.network_fee_bps, merchant_rule.network_fee_bps);
        assert_eq!(
            resolved.settlement_delay_ledger,
            merchant_rule.settlement_delay_ledger
        );
        assert_eq!(resolved.auto_settle, merchant_rule.auto_settle);

        env.as_contract(&client.address, || {
            // get_ttl returns the remaining ledger count, not an absolute sequence
            // number, so a freshly extended entry's TTL settles back at RULE_TTL_BUMP.
            let merchant_ttl = env
                .storage()
                .persistent()
                .get_ttl(&DataKey::Rule(merchant.clone()));
            assert!(
                merchant_ttl >= RULE_TTL_BUMP,
                "merchant rule TTL must be extended when the merchant rule is resolved"
            );

            // DefaultRule must be left completely untouched: its absolute expiration
            // ledger is unchanged, proving it was never read once the merchant rule
            // was found (an extend_ttl call would have pushed this number forward).
            let default_expiration_after =
                env.ledger().sequence() + env.storage().persistent().get_ttl(&DataKey::DefaultRule);
            assert_eq!(
                default_expiration_after, default_expiration_before,
                "DefaultRule must not be read or have its TTL extended when a merchant rule exists"
            );
        });
    }

    #[test]
    fn read_rule_or_default_falls_back_to_default_without_creating_merchant_entry() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let default_rule = SettlementRule {
            platform_fee_bps: 150,
            network_fee_bps: 25,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        client.set_default_rule(&default_rule);

        let resolved = env.as_contract(&client.address, || {
            read_rule_or_default(&env, merchant.clone())
        });
        assert_eq!(resolved.platform_fee_bps, default_rule.platform_fee_bps);
        assert_eq!(resolved.network_fee_bps, default_rule.network_fee_bps);
        assert_eq!(
            resolved.settlement_delay_ledger,
            default_rule.settlement_delay_ledger
        );
        assert_eq!(resolved.auto_settle, default_rule.auto_settle);

        env.as_contract(&client.address, || {
            // No merchant-specific rule was ever written by the fallback lookup.
            assert!(!env
                .storage()
                .persistent()
                .has(&DataKey::Rule(merchant.clone())));

            let default_ttl = env.storage().persistent().get_ttl(&DataKey::DefaultRule);
            assert!(
                default_ttl >= RULE_TTL_BUMP,
                "DefaultRule TTL must be extended when it is the resolved rule"
            );
        });
    }

    #[test]
    fn read_rule_or_default_bootstrap_path_reads_only_leaves_no_storage_footprint() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let before = env.events().all().len();
        let resolved = env.as_contract(&client.address, || {
            read_rule_or_default(&env, merchant.clone())
        });

        assert_eq!(
            resolved.platform_fee_bps,
            BOOTSTRAP_DEFAULT_RULE.platform_fee_bps
        );
        assert_eq!(
            resolved.network_fee_bps,
            BOOTSTRAP_DEFAULT_RULE.network_fee_bps
        );
        assert_eq!(
            resolved.settlement_delay_ledger,
            BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
        );
        assert_eq!(resolved.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);

        // The worst case (neither key set) must remain read-only: no entry gets
        // created for either key as a side effect of resolving the bootstrap rule.
        env.as_contract(&client.address, || {
            assert!(!env
                .storage()
                .persistent()
                .has(&DataKey::Rule(merchant.clone())));
            assert!(!env.storage().persistent().has(&DataKey::DefaultRule));
        });

        let events = env.events().all();
        assert_eq!(
            events.len(),
            before + 1,
            "exactly one bootstrap_fallback event expected"
        );
    }

    #[test]
    fn global_default_used_when_no_explicit_merchant_rule() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
        assert_eq!(split.network_fee_amount, 250); // 50 bps
        assert_eq!(split.merchant_amount, 48_750);
    }

    #[test]
    fn explicit_merchant_rule_overrides_global_default() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let merchant_rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &merchant_rule);

        let split = client.calculate_fee_split(&merchant, &50_000);
        // Merchant rule (175/25) takes precedence over global default (200/50)
        assert_eq!(split.platform_fee_amount, 875); // 175 bps
        assert_eq!(split.network_fee_amount, 125); // 25 bps
        assert_eq!(split.merchant_amount, 49_000);
    }

    #[test]
    fn set_default_rule_stores_and_can_be_retrieved() {
        let (_env, client, _admin, _merchant) = setup();

        assert!(client.get_default_rule().is_none());

        let rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 100,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        client.set_default_rule(&rule);

        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 300);
        assert_eq!(stored.network_fee_bps, 100);
        assert_eq!(stored.settlement_delay_ledger, 5);
        assert!(stored.auto_settle);
    }

    #[test]
    fn set_default_rule_emits_event_with_correct_topic() {
        let (env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 150,
            network_fee_bps: 25,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_default_rule(&rule);

        let events = env.events().all();
        let (_contract_id, topics, _data) = events.get(events.len() - 1).unwrap();

        // Single-element topic: just the event name
        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "default_rule_updated")
        );
    }

    #[test]
    fn set_default_rule_updates_twice_emits_correct_previous() {
        let (_env, client, _admin, _merchant) = setup();

        let first = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&first);
        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 200);

        let second = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 100,
            settlement_delay_ledger: 20,
            auto_settle: false,
        };
        client.set_default_rule(&second);
        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 500);
    }

    #[test]
    fn clearing_rule_falls_back_to_global_default() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let merchant_rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 100,
            settlement_delay_ledger: 20,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &merchant_rule);

        let prev_count = env.events().all().len();
        client.clear_settlement_rule(&merchant);

        // After clearing, should fall back to global default (200/50), not bootstrap (100/0)
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
        assert_eq!(split.network_fee_amount, 250); // 50 bps
        assert_eq!(split.merchant_amount, 48_750);

        // Event check: fallback should be the global default rule
        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1);
        let (_contract_id, topics, data) = events.get(prev_count).unwrap();
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_cleared")
        );
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

        let (_caller, removed, fallback): (Address, SettlementRule, SettlementRule) =
            FromVal::from_val(&env, &data);
        assert_eq!(removed.platform_fee_bps, merchant_rule.platform_fee_bps);
        assert_eq!(removed.network_fee_bps, merchant_rule.network_fee_bps);
        assert_eq!(removed.settlement_delay_ledger, merchant_rule.settlement_delay_ledger);
        assert_eq!(removed.auto_settle, merchant_rule.auto_settle);
        assert_eq!(fallback.platform_fee_bps, global_rule.platform_fee_bps);
        assert_eq!(fallback.network_fee_bps, global_rule.network_fee_bps);
        assert_eq!(fallback.settlement_delay_ledger, global_rule.settlement_delay_ledger);
        assert_eq!(fallback.auto_settle, global_rule.auto_settle);
    }

    #[test]
    #[should_panic]
    fn set_default_rule_fails_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        let contract_id: Address = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        let invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: soroban_sdk::vec![
                &env,
                admin.to_val(),
                governance.to_val(),
                recovery_address.to_val()
            ],
            sub_invokes: &[],
        };
        let auth = MockAuth {
            address: &admin,
            invoke: &invoke,
        };
        env.set_auths(&[(&auth).into()]);
        client.init(&admin, &governance, &recovery_address);

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };

        // Do NOT authorize admin — should panic
        client.set_default_rule(&rule);
    }

    #[test]
    #[should_panic]
    fn set_default_rule_rejects_invalid_fee_bps() {
        let (_env, client, _admin, _merchant) = setup();

        let bad_rule = SettlementRule {
            platform_fee_bps: 10_001,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_default_rule(&bad_rule);
    }

    #[test]
    #[should_panic]
    fn set_default_rule_rejects_below_governance_min_fee() {
        let (_env, client, _admin, _merchant) = setup();

        let bad_rule = SettlementRule {
            platform_fee_bps: 4,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_default_rule(&bad_rule);
    }

    #[test]
    fn settlement_min_fee_matches_governance_min_fee() {
        // Both contracts must enforce the same minimum fee of 5 bps.
        // Governance rejects fee configs with any value below 5 bps,
        // and settlement rejects settlement rules with any value below 5 bps.
        let governance_min_fee_bps: u32 = 5;
        let settlement_min_fee_bps: u32 = MIN_FEE_BPS;
        assert_eq!(
            governance_min_fee_bps, settlement_min_fee_bps,
            "settlement MIN_FEE_BPS must match governance MIN_FEE_BPS"
        );
    }

    #[test]
    fn accepts_valid_settlement_delay_zero() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 0);
    }

    #[test]
    fn accepts_valid_settlement_delay_one() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 1,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 1);
    }

    #[test]
    fn accepts_settlement_delay_at_maximum_boundary() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 100_000,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 100_000);
    }

    #[test]
    #[should_panic]
    fn rejects_settlement_delay_above_maximum() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 100_001,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
    }

    #[test]
    #[should_panic]
    fn rejects_settlement_delay_at_u32_max() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: u32::MAX,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
    }

    #[test]
    fn accepts_default_rule_with_valid_settlement_delay() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 50_000,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
        let stored = client.get_default_rule().expect("expected default rule");
        assert_eq!(stored.settlement_delay_ledger, 50_000);
    }

    #[test]
    fn accepts_default_rule_at_settlement_delay_maximum() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 100_000,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
        let stored = client.get_default_rule().expect("expected default rule");
        assert_eq!(stored.settlement_delay_ledger, 100_000);
    }

    #[test]
    #[should_panic]
    fn rejects_default_rule_with_settlement_delay_above_maximum() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 100_001,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
    }

    #[test]
    #[should_panic]
    fn rejects_default_rule_with_settlement_delay_at_u32_max() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: u32::MAX,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
    }
    #[test]
    #[should_panic]
    fn set_settlement_rule_requires_admin_auth() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);

        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        env.mock_all_auths();
        client.init(&admin, &governance, &recovery_address);
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        // Switch to explicit mock_auths to test authorization failure.
        // We only provide authorization for the non_admin, but the contract
        // requires authorization from the admin address.
        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "set_settlement_rule",
                args: soroban_sdk::vec![
                    &env,
                    merchant.clone().into_val(&env),
                    rule.clone().into_val(&env)
                ],
                sub_invokes: &[],
            },
        }]);

        client.set_settlement_rule(&merchant, &rule);
    }

    #[test]
    fn verify_payment_storage_events() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[77; 32]);
        let before = env.events().all().len();
        client.store_payment_reference(&merchant, &reference, &20_000);

        let events = env.events().all();
        assert_eq!(
            events.len(),
            before + 1,
            "exactly one event should be emitted by store_payment_reference"
        );

        // payment_stored carries the full fee split via the embedded PaymentRecord.
        let event1 = events.get(before).unwrap();
        let (_contract_id, topics1, data1) = event1;
        assert_eq!(topics1.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics1.get(0).unwrap()),
            Symbol::new(&env, "payment_stored")
        );
        assert_eq!(Address::from_val(&env, &topics1.get(1).unwrap()), merchant);

        let (ref1, record): (BytesN<32>, PaymentRecord) =
            FromVal::from_val(&env, &data1);
        assert_eq!(ref1, reference);
        assert_eq!(record.amount, 20_000);
        assert_eq!(record.platform_fee_amount, 500);
        assert_eq!(record.network_fee_amount, 100);
        assert_eq!(record.merchant_amount, 19_400);
        assert_eq!(record.platform_fee_bps, 250);
        assert_eq!(record.network_fee_bps, 50);
    }

    #[test]
    #[should_panic]
    fn store_payment_reference_requires_merchant_auth() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        // Authorize admin for init
        let init_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: soroban_sdk::vec![
                &env,
                admin.to_val(),
                governance.to_val(),
                recovery_address.to_val()
            ],
            sub_invokes: &[],
        };
        let init_auth = MockAuth {
            address: &admin,
            invoke: &init_invoke,
        };
        env.set_auths(&[(&init_auth).into()]);
        client.init(&admin, &governance, &recovery_address);

        // Authorize admin for register_merchant
        let reg_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_merchant",
            args: soroban_sdk::vec![&env, merchant.to_val()],
            sub_invokes: &[],
        };
        let reg_auth = MockAuth {
            address: &admin,
            invoke: &reg_invoke,
        };
        env.set_auths(&[(&reg_auth).into()]);
        client.register_merchant(&merchant);

        // Do NOT authorize the merchant for store_payment_reference — should panic.
        let reference = BytesN::from_array(&env, &[15; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);
    }

    /// Verify that `set_settlement_rule` rejects fee combinations whose
    /// `platform_fee_bps + network_fee_bps` sum exceeds 10,000 bps with
    /// the specific `InvalidFeeBps` contract error (#6).
    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn assert_fee_sum_above_10000_bps_panics_with_invalid_fee_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        // 6_000 + 5_000 = 11_000 which is 1_000 bps over the 10_000 cap.
        let bad_rule = SettlementRule {
            platform_fee_bps: 6_000,
            network_fee_bps: 5_000,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    // Issue #76: verify only admin can register merchants

    #[test]
    #[should_panic]
    fn register_merchant_requires_admin_auth() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);
        env.mock_all_auths();
        client.init(&admin, &governance, &recovery_address);
        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "register_merchant",
                args: soroban_sdk::vec![&env, merchant.clone().into_val(&env)],
                sub_invokes: &[],
            },
        }]);
        client.register_merchant(&merchant);
    }

    // Issue #77: verify duplicate merchant registration fails with MerchantExists
    #[test]
    #[should_panic]
    fn duplicate_merchant_registration_fails() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        client.register_merchant(&merchant);
    }

    // Issue #90 / #271: verify the fee split is available via payment_stored,
    // without a redundant payment_split event.
    #[test]
    fn split_data_available_on_payment_stored() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let reference = BytesN::from_array(&env, &[42; 32]);
        let before = env.events().all().len();
        client.store_payment_reference(&merchant, &reference, &10_000);
        let events = env.events().all();
        assert!(events.len() >= before + 2);
        let found_split = events
            .iter()
            .skip(before as usize)
            .any(|(_id, topics, _data)| {
                topics.len() >= 1
                    && Symbol::from_val(&env, &topics.get(0).unwrap())
                        == Symbol::new(&env, "payment_split")
            });
        assert!(found_split, "payment_split event not emitted");
    }

    // Issue #85: verify default fee split falls back to 100 BPS
    #[test]
    fn default_fee_split_uses_100_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let split = client.calculate_fee_split(&merchant, &10_000);
        assert_eq!(split.platform_fee_amount, 100);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 9_900);
    }

    // Issue #72: verify non-admin transfer_admin calls are rejected
    #[test]
    #[should_panic]
    fn transfer_admin_rejected_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);
        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        env.mock_all_auths();
        client.init(&admin, &governance, &recovery_address);
        env.mock_auths(&[MockAuth {
            address: &non_admin,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "transfer_admin",
                args: soroban_sdk::vec![&env, new_admin.clone().into_val(&env)],
                sub_invokes: &[],
            },
        }]);
        client.transfer_admin(&new_admin);
    }

    // Issue #88: verify set_settlement_rule publishes event with caller and rule data
    #[test]
    fn set_settlement_rule_publishes_event_with_rule_data() {
        let (env, client, admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 75,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        let before = env.events().all().len();
        client.set_settlement_rule(&merchant, &rule);
        let events = env.events().all();
        assert_eq!(events.len(), before + 1, "exactly one event emitted");
        let (_contract_id, topics, data) = events.get(before).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
        let (caller, _prev, current): (Address, SettlementRule, SettlementRule) =
            FromVal::from_val(&env, &data);
        assert_eq!(caller, admin);
        assert_eq!(current.platform_fee_bps, 300);
        assert_eq!(current.network_fee_bps, 75);
        assert_eq!(current.settlement_delay_ledger, 5);
        assert!(current.auto_settle);
    }

    // Issue #84: store_payment_reference rejects zero amount with InvalidAmount (#7)
    #[test]
    #[should_panic(expected = "Error(Contract, #7)")]
    fn store_payment_reference_rejects_zero_amount_with_invalid_amount_error() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[55; 32]);
        client.store_payment_reference(&merchant, &reference, &0);
    }

    // Issue #84: store_payment_reference rejects negative amounts with InvalidAmount (#7)
    #[test]
    #[should_panic(expected = "Error(Contract, #7)")]
    fn store_payment_reference_rejects_negative_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[56; 32]);
        client.store_payment_reference(&merchant, &reference, &-1);
    }

    // Issue #86: calculate_fee_split output matches custom rule parameters
    #[test]
    fn calculate_fee_split_uses_custom_rule_parameters() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 250,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let split = client.calculate_fee_split(&merchant, &100_000);
        assert_eq!(split.gross_amount, 100_000);
        assert_eq!(split.platform_fee_amount, 5_000);
        assert_eq!(split.network_fee_amount, 2_500);
        assert_eq!(split.merchant_amount, 92_500);
    }

    // Issue #248: calculate_fee_split panics with a readable AmountOverflow error
    // instead of a raw arithmetic-overflow panic when amount * bps would overflow i128.
    #[test]
    #[should_panic(expected = "Error(Contract, #19)")]
    fn calculate_fee_split_rejects_amount_that_would_overflow() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        // (i128::MAX - (BPS_DENOMINATOR - 1)) / 500 is the largest amount that stays
        // safe through the ceil-rounding addition; one past it overflows.
        let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500 + 1;
        client.calculate_fee_split(&merchant, &amount);
    }

    // Issue #248: store_payment_reference is also protected, since it shares
    // the same calculate_split code path as calculate_fee_split.
    #[test]
    #[should_panic(expected = "Error(Contract, #19)")]
    fn store_payment_reference_rejects_amount_that_would_overflow() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 250,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let reference = BytesN::from_array(&env, &[77; 32]);
        // The max of the two bps values (500) determines the overflow boundary.
        let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500 + 1;
        client.store_payment_reference(&merchant, &reference, &amount);
    }

    // Issue #248: amounts right at the overflow boundary must still succeed normally.
    #[test]
    fn calculate_fee_split_accepts_amount_at_overflow_boundary() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500;
        let split = client.calculate_fee_split(&merchant, &amount);
        assert_eq!(split.gross_amount, amount);
    }

    // Issue #248: a zero-bps rule must never divide by zero in the overflow precheck.
    #[test]
    fn calculate_fee_split_with_zero_bps_rule_accepts_max_amount() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 0,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let split = client.calculate_fee_split(&merchant, &i128::MAX);
        assert_eq!(split.gross_amount, i128::MAX);
        assert_eq!(split.platform_fee_amount, 0);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, i128::MAX);
    }

    #[test]
    fn merchant_registration_succeeds_when_paused() {
        let (_env, client, _admin, merchant) = setup();
        client.pause();
        assert!(client.is_paused());

        client.register_merchant(&merchant);
        assert!(client.is_merchant_registered(&merchant));
    }

    #[test]
    #[should_panic]
    fn set_settlement_rule_rejected_when_paused() {
        let (_env, client, _admin, merchant) = setup();
        client.pause();
        assert!(client.is_paused());

        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 7,
            auto_settle: true,
        };
        client.set_settlement_rule(&merchant, &rule);
    }

    // Issue #231: the global default settlement rule must not be updated while paused.
    #[test]
    #[should_panic(expected = "Error(Contract, #9)")]
    fn set_default_rule_rejected_when_paused() {
        let (_env, client, _admin, _merchant) = setup();
        client.pause();
        assert!(client.is_paused());

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 7,
            auto_settle: true,
        };
        client.set_default_rule(&rule);
    }

    // Issue #75: verify pause flag changes state in settlement contract
    #[test]
    fn pause_flag_changes_state() {
        let (_env, client, _admin, _merchant) = setup();
        assert!(!client.is_paused());
        client.pause();
        assert!(client.is_paused());
        client.unpause();
        assert!(!client.is_paused());
    }

    // Issue #73: verify non-admins cannot pause the settlement contract
    #[test]
    #[should_panic]
    fn pause_rejected_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        let governance = register_governance(&env);
        let recovery_address = Address::generate(&env);
        let init_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: soroban_sdk::vec![&env, admin.to_val(), governance.to_val(), recovery_address.to_val()],
            sub_invokes: &[],
        };
        let init_auth = MockAuth {
            address: &admin,
            invoke: &init_invoke,
        };
        env.set_auths(&[(&init_auth).into()]);
        client.init(&admin, &governance, &recovery_address);

        let pause_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: soroban_sdk::vec![&env],
            sub_invokes: &[],
        };
        let pause_auth = MockAuth {
            address: &non_admin,
            invoke: &pause_invoke,
        };
        env.set_auths(&[(&pause_auth).into()]);
        client.pause();
    }

    // Issue #82: verify storing reference for non-merchant panics with MerchantMissing
    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn store_payment_reference_fails_for_unregistered_merchant() {
        let (env, client, _admin, merchant) = setup();
        let reference = BytesN::from_array(&env, &[99; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);
    }

    // Verify event emitted on admin transfer
    #[test]
    fn emits_event_on_admin_transfer() {
        let (env, client, _admin, _merchant) = setup();
        let new_admin = Address::generate(&env);

        let before = env.events().all().len();
        client.transfer_admin(&new_admin);

        let events = env.events().all();
        assert_eq!(
            events.len(),
            before + 1,
            "exactly one event should be emitted by transfer_admin"
        );

        let event = events.last().unwrap();
        let (contract_id, topics, data) = event;

        assert_eq!(contract_id, client.address);
        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "admin")
        );
        assert_eq!(Address::from_val(&env, &data), new_admin);
    }
}
