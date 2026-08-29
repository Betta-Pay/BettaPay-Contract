//! Shared event payloads and emission helpers.
//!
//! Both contracts publish events under a small set of topic names and use
//! the same pair of structured payload types (`AdminTransferred`,
//! `PendingRecovery`). Putting them here so the two contracts cannot diverge.

use soroban_sdk::{contracttype, Address, Env, Symbol};

use crate::types::SettlementRule;

/// Structured payload emitted with the `admin_transferred` event so that
/// off-chain consumers can read the old and new admin by field name rather
/// than by positional order.
#[derive(Clone)]
#[contracttype]
pub struct AdminTransferred {
    pub old_admin: Address,
    pub new_admin: Address,
}

/// In-flight recovery operation. Lives between `initiate_recovery` and a
/// successful `execute_recovery` (or a `cancel_recovery`).
///
/// Both contracts use exactly this shape; the struct is encoded by field name
/// so any historical instance written with a copy of this struct in
/// `governance_contract` remains readable via `bettapay_common`.
#[derive(Clone)]
#[contracttype]
pub struct PendingRecovery {
    pub new_admin: Address,
    pub execute_after: u64,
}

// Event-topic registry.
//
// Soroban `Symbol`s are limited to 32 bytes; every value here comfortably
// fits. This is the single place every topic name used by any BettaPay
// contract is defined — governance_contract and settlement_contract must
// not construct a topic `Symbol` from an inline string literal; they import
// the constant from here instead. That is what lets an off-chain indexer
// trust a topic name meaning the same thing regardless of which contract
// emitted it, and is enforced by each contract's conformity tests (see
// `governance_contract::tests` and
// `settlement_contract::tests::event_topic_conformity_tests`).
//
// Constants are grouped below by which contract(s) emit them. A topic only
// moves to the "shared" group once two contracts emit it for the same
// underlying event; a name coincidence alone is not enough.

// --- Shared: emitted by both governance_contract and settlement_contract ---

/// Topic emitted when the admin role is transferred (also reused by
/// `execute_recovery`).
pub const ADMIN_TRANSFERRED_EVENT: &str = "admin_transferred";

/// Topic emitted when the contract is paused.
pub const PAUSED_EVENT: &str = "paused";

/// Topic emitted when the contract is unpaused.
pub const UNPAUSED_EVENT: &str = "unpaused";

/// Topic emitted when a recovery operation has been initiated.
pub const RECOVERY_INITIATED_EVENT: &str = "recovery_initiated";

/// Topic emitted when a recovery operation has been cancelled.
pub const RECOVERY_CANCELLED_EVENT: &str = "recovery_cancelled";

/// Topic emitted when a recovery operation has been executed.
pub const RECOVERY_EXECUTED_EVENT: &str = "recovery_executed";

/// Topic emitted when a contract's Wasm executable is upgraded.
pub const CONTRACT_UPGRADED_EVENT: &str = "contract_upgraded";

/// Topic emitted when the multisig admin threshold changes.
pub const THRESHOLD_CHANGED_EVENT: &str = "threshold_changed";

// --- governance_contract only ---

/// Topic emitted once, when `GovernanceContract::init` completes.
pub const INITIALIZED_EVENT: &str = "initialized";

/// Topic emitted when `migrate` completes (schema version management).
pub const MIGRATED_EVENT: &str = "migrated";

/// Topic emitted when an arbitrary system parameter is updated.
pub const SYS_PARAM_UPDATED_EVENT: &str = "sys_param_updated";

/// Topic emitted when the protocol fee configuration changes.
pub const FEE_CONFIG_UPDATED_EVENT: &str = "fee_config_updated";

/// Topic emitted when an asset's trusted anchor is created or replaced.
pub const ANCHOR_UPSERTED_EVENT: &str = "anchor_upserted";

/// Topic emitted when an asset's trusted anchor is removed.
pub const ANCHOR_REMOVED_EVENT: &str = "anchor_removed";

// --- settlement_contract only ---

/// Topic emitted when `update_governance` stores a new governance address.
pub const GOVERNANCE_UPDATED_EVENT: &str = "governance_updated";

/// Topic emitted when a merchant is registered.
pub const MERCHANT_REGISTERED_EVENT: &str = "merchant_registered";

/// Topic emitted when a merchant is unregistered.
pub const MERCHANT_UNREGISTERED_EVENT: &str = "merchant_unregistered";

/// Topic emitted when a merchant-specific settlement rule is set or replaced.
pub const SETTLEMENT_RULE_UPDATED_EVENT: &str = "settlement_rule_updated";

/// Topic emitted when a merchant-specific settlement rule is cleared —
/// either explicitly via `clear_settlement_rule`, or as a side effect of
/// `unregister_merchant` removing a merchant that had one set.
///
/// Both removal paths publish through [`emit_settlement_rule_cleared`], so
/// the data payload is always the same canonical triple
/// `(admin, removed, fallback)` (issue #491).
pub const SETTLEMENT_RULE_CLEARED_EVENT: &str = "settlement_rule_cleared";

/// Topic emitted when the global default settlement rule is updated.
pub const DEFAULT_RULE_UPDATED_EVENT: &str = "default_rule_updated";

/// Topic emitted when `read_rule_or_default` falls all the way through to
/// the hardcoded bootstrap rule because no merchant, default, or governance
/// rule is configured yet.
pub const BOOTSTRAP_FALLBACK_EVENT: &str = "bootstrap_fallback";

/// Topic emitted when a payment reference is stored via
/// `store_payment_reference`.
pub const PAYMENT_STORED_EVENT: &str = "payment_stored";

/// Topic emitted when a timelocked administrative operation is scheduled.
pub const OP_SCHEDULED_EVENT: &str = "op_scheduled";

/// Topic emitted when a scheduled administrative operation executes.
pub const OP_EXECUTED_EVENT: &str = "op_executed";

/// Topic emitted when a scheduled administrative operation is cancelled.
pub const OP_CANCELLED_EVENT: &str = "op_cancelled";

/// Emits the `admin_transferred` event with the structured
/// [`AdminTransferred`] payload.
///
/// Topics: `(Symbol("admin_transferred"),)`
/// Data:    `AdminTransferred { old_admin, new_admin }`
pub fn emit_admin_transferred(env: &Env, payload: &AdminTransferred) {
    env.events().publish(
        (Symbol::new(env, ADMIN_TRANSFERRED_EVENT),),
        payload.clone(),
    );
}

/// Emits the `paused` event.
///
/// Topics: `(Symbol("paused"),)`
/// Data:    `(admin_address, true)`
pub fn emit_paused(env: &Env, admin: &Address) {
    env.events()
        .publish((Symbol::new(env, PAUSED_EVENT),), (admin.clone(), true));
}

/// Emits the `unpaused` event.
///
/// Topics: `(Symbol("unpaused"),)`
/// Data:    `(admin_address, false)`
pub fn emit_unpaused(env: &Env, admin: &Address) {
    env.events()
        .publish((Symbol::new(env, UNPAUSED_EVENT),), (admin.clone(), false));
}

/// Emits the `recovery_initiated` event.
///
/// Topics: `(Symbol("recovery_initiated"),)`
/// Data:    `(recovery_address, new_admin, execute_after)`
pub fn emit_recovery_initiated(
    env: &Env,
    recovery: &Address,
    new_admin: &Address,
    execute_after: u64,
) {
    env.events().publish(
        (Symbol::new(env, RECOVERY_INITIATED_EVENT),),
        (recovery.clone(), new_admin.clone(), execute_after),
    );
}

/// Emits the `recovery_cancelled` event.
///
/// Topics: `(Symbol("recovery_cancelled"),)`
/// Data:    `admin_address`
pub fn emit_recovery_cancelled(env: &Env, admin: &Address) {
    env.events()
        .publish((Symbol::new(env, RECOVERY_CANCELLED_EVENT),), admin.clone());
}

/// Emits the `recovery_executed` event.
///
/// Topics: `(Symbol("recovery_executed"),)`
/// Data:    `AdminTransferred { old_admin, new_admin }`
pub fn emit_recovery_executed(env: &Env, payload: &AdminTransferred) {
    env.events().publish(
        (Symbol::new(env, RECOVERY_EXECUTED_EVENT),),
        payload.clone(),
    );
}

/// Emits the `settlement_rule_cleared` event with the canonical payload.
///
/// Topics: `(Symbol("settlement_rule_cleared"), Address merchant)`
/// Data:    `(Address admin, SettlementRule removed, SettlementRule fallback)`
///
/// Every path that removes a merchant-specific settlement rule must publish
/// through this helper — `clear_settlement_rule` and the side effect of
/// `unregister_merchant` — so an indexer always sees the same data arity for
/// the topic (issue #491). `removed` is the rule that was stored; `fallback`
/// is the rule the merchant falls back to (default rule, or the bootstrap
/// rule when none is stored).
pub fn emit_settlement_rule_cleared(
    env: &Env,
    merchant: &Address,
    admin: &Address,
    removed: &SettlementRule,
    fallback: &SettlementRule,
) {
    env.events().publish(
        (
            Symbol::new(env, SETTLEMENT_RULE_CLEARED_EVENT),
            merchant.clone(),
        ),
        (admin.clone(), removed.clone(), fallback.clone()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Map, TryFromVal, TryIntoVal, Val};

    /// The canonical payload shapes must keep their field names stable: an
    /// indexer decodes these structs by field name, so renaming a field would
    /// silently change the event's data shape for every consumer (issue #518).
    #[test]
    fn admin_transferred_payload_shape_is_canonical() {
        let env = Env::default();
        let old_admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let payload = AdminTransferred {
            old_admin: old_admin.clone(),
            new_admin: new_admin.clone(),
        };

        let val: Val = payload.try_into_val(&env).unwrap();
        let fields: Map<Symbol, Val> = Map::try_from_val(&env, &val).unwrap();

        assert!(fields.contains_key(Symbol::new(&env, "old_admin")));
        assert!(fields.contains_key(Symbol::new(&env, "new_admin")));
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn pending_recovery_payload_shape_is_canonical() {
        let env = Env::default();
        let new_admin = Address::generate(&env);
        let payload = PendingRecovery {
            new_admin,
            execute_after: 123,
        };

        let val: Val = payload.try_into_val(&env).unwrap();
        let fields: Map<Symbol, Val> = Map::try_from_val(&env, &val).unwrap();

        assert!(fields.contains_key(Symbol::new(&env, "new_admin")));
        assert!(fields.contains_key(Symbol::new(&env, "execute_after")));
        assert_eq!(fields.len(), 2);
    }
}
