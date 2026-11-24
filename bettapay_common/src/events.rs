//! Shared event payloads and emission helpers.
//!
//! Both contracts publish events under a small set of topic names and use
//! the same pair of structured payload types (`AdminTransferred`,
//! `PendingRecovery`). Putting them here so the two contracts cannot diverge.

use soroban_sdk::{contracttype, Address, Env, Symbol, Vec};

use crate::types::SettlementRule;

/// Structured payload emitted with the `admin_transferred` event so that
/// off-chain consumers can read the old and new admin **sets** by field name
/// rather than by positional order, and reconstruct an authorization
/// transition without a separate storage scan (issue #977).
///
/// Both fields carry the full multisig admin set, not just the primary
/// (index `0`) admin, so a threshold change that accompanies the transfer is
/// also visible directly from the event. Field order is fixed as old then
/// new.
#[derive(Clone)]
#[contracttype]
pub struct AdminTransferred {
    pub old_admins: Vec<Address>,
    pub new_admins: Vec<Address>,
}

/// In-flight recovery operation. Lives between `initiate_recovery` and a
/// successful `execute_recovery` (or a `cancel_recovery`).
///
/// `initiated_by` records the address that started this specific recovery -
/// the recovery address that authenticated at initiation time (issue #560) -
/// so `cancel_recovery` can validate the cancellation against it and
/// off-chain consumers can audit who initiated. Both contracts use exactly
/// this shape; the struct is encoded by field name so any historical
/// instance written with a copy of this struct in `governance_contract`
/// remains readable via `bettapay_common`.
#[derive(Clone)]
#[contracttype]
pub struct PendingRecovery {
    pub new_admin: Address,
    pub execute_after: u64,
    pub initiated_by: Address,
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

/// Topic emitted once, when either contract's `init` completes.
pub const INITIALIZED_EVENT: &str = "initialized";

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
///
/// Both contracts publish the same canonical data payload through
/// [`emit_threshold_changed`]: `(current_threshold, new_threshold)`. Keep the
/// actor out of the payload so a single indexer decoder works for events from
/// either contract.
pub const THRESHOLD_CHANGED_EVENT: &str = "threshold_changed";

/// Topic emitted when the recovery address is rotated (issue #694).
pub const RECOVERY_ADDRESS_UPDATED_EVENT: &str = "recovery_address_updated";

// --- governance_contract only ---

/// Topic emitted when `migrate` completes (schema version management).
pub const MIGRATED_EVENT: &str = "migrated";

/// Topic emitted when an arbitrary system parameter is updated.
pub const SYS_PARAM_UPDATED_EVENT: &str = "sys_param_updated";

/// Topic emitted when a stored system parameter is removed.
pub const SYS_PARAM_REMOVED_EVENT: &str = "sys_param_removed";

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

/// Topic emitted alongside `merchant_registered`, marking that the merchant
/// itself authorized its own registration (`merchant.require_auth()`), as
/// distinct from the admin action that invoked `register_merchant` (issue #978).
pub const MERCHANT_CONSENT_EVENT: &str = "merchant_consent";

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

/// Topic emitted once per `prune_payments` call. Data is
/// `(admin, removed_count)`.
pub const PAYMENTS_PRUNED_EVENT: &str = "payments_pruned";

/// Topic emitted when a single payment record is removed by `prune_payment`.
///
/// Topics: `(Symbol("payment_pruned"), Address merchant, BytesN<32> reference)`
/// Data:   `(Address merchant, BytesN<32> reference)`
///
/// Emitted only when a record actually existed and was removed; pruning a
/// missing record is a no-op and stays silent (issue #890).
pub const PAYMENT_PRUNED_EVENT: &str = "payment_pruned";

/// Topic emitted when a timelocked administrative operation is scheduled.
pub const OP_SCHEDULED_EVENT: &str = "op_scheduled";

/// Topic emitted when a scheduled administrative operation executes.
pub const OP_EXECUTED_EVENT: &str = "op_executed";

/// Topic emitted when a scheduled administrative operation is cancelled.
pub const OP_CANCELLED_EVENT: &str = "op_cancelled";

/// Every topic name defined in this registry, across all groups (shared,
/// governance-only, settlement-only).
///
/// Each contract's event-topic tests pin this complete set and assert every
/// emitted `topic[0]` belongs to it, so additions are deliberate and call
/// sites cannot silently drift from the registry.
pub const ALL_EVENT_TOPICS: &[&str] = &[
    // Shared
    INITIALIZED_EVENT,
    ADMIN_TRANSFERRED_EVENT,
    PAUSED_EVENT,
    UNPAUSED_EVENT,
    RECOVERY_INITIATED_EVENT,
    RECOVERY_CANCELLED_EVENT,
    RECOVERY_EXECUTED_EVENT,
    CONTRACT_UPGRADED_EVENT,
    THRESHOLD_CHANGED_EVENT,
    RECOVERY_ADDRESS_UPDATED_EVENT,
    // governance_contract only
    MIGRATED_EVENT,
    SYS_PARAM_UPDATED_EVENT,
    SYS_PARAM_REMOVED_EVENT,
    FEE_CONFIG_UPDATED_EVENT,
    ANCHOR_UPSERTED_EVENT,
    ANCHOR_REMOVED_EVENT,
    // settlement_contract only
    GOVERNANCE_UPDATED_EVENT,
    MERCHANT_REGISTERED_EVENT,
    MERCHANT_CONSENT_EVENT,
    MERCHANT_UNREGISTERED_EVENT,
    SETTLEMENT_RULE_UPDATED_EVENT,
    SETTLEMENT_RULE_CLEARED_EVENT,
    DEFAULT_RULE_UPDATED_EVENT,
    BOOTSTRAP_FALLBACK_EVENT,
    PAYMENT_STORED_EVENT,
    PAYMENTS_PRUNED_EVENT,
    PAYMENT_PRUNED_EVENT,
    OP_SCHEDULED_EVENT,
    OP_EXECUTED_EVENT,
    OP_CANCELLED_EVENT,
];

/// Emits the `admin_transferred` event with the structured
/// [`AdminTransferred`] payload.
///
/// Topics: `(Symbol("admin_transferred"),)`
/// Data:    `AdminTransferred { old_admins, new_admins }`
pub fn emit_admin_transferred(env: &Env, payload: &AdminTransferred) {
    env.events().publish(
        (Symbol::new(env, ADMIN_TRANSFERRED_EVENT),),
        payload.clone(),
    );
}

/// Emits the `threshold_changed` event with the canonical shared payload.
///
/// Topics: `(Symbol("threshold_changed"),)`
/// Data:    `(current_threshold, new_threshold)` — both `u32`.
///
/// Both `governance_contract::change_threshold` and
/// `settlement_contract::change_threshold` publish through this helper so the
/// topic has one payload shape across the two contracts. Indexers decoding a
/// `threshold_changed` event therefore never have to guess which contract
/// emitted it (issue: align THRESHOLD_CHANGED_EVENT payloads).
pub fn emit_threshold_changed(env: &Env, current_threshold: u32, new_threshold: u32) {
    env.events().publish(
        (Symbol::new(env, THRESHOLD_CHANGED_EVENT),),
        (current_threshold, new_threshold),
    );
}

/// Emits the `paused` event.
///
/// Topics: `(Symbol("paused"),)`
/// Data:    `(admin_address, true, reason)`
/// This payload intentionally has one more field than `unpaused`.
///
/// `reason` is a bounded numeric code (issue #991) — see
/// [`crate::constants::MAX_PAUSE_REASON`] — carrying the machine-readable
/// cause of the pause for incident forensics. Callers that don't expose a
/// reason to their own callers pass [`crate::constants::PAUSE_REASON_NONE`].
pub fn emit_paused(env: &Env, admin: &Address, reason: u32) {
    env.events().publish(
        (Symbol::new(env, PAUSED_EVENT),),
        (admin.clone(), true, reason),
    );
}

/// Emits the `unpaused` event.
///
/// Topics: `(Symbol("unpaused"),)`
/// Data:    `(admin_address, false)`
/// Unlike `paused`, this payload has no reason field.
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
/// Data:    `canceller` - the address that cancelled the recovery: an admin,
///          or the initiator cancelling its own recovery (issue #560).
pub fn emit_recovery_cancelled(env: &Env, canceller: &Address) {
    env.events().publish(
        (Symbol::new(env, RECOVERY_CANCELLED_EVENT),),
        canceller.clone(),
    );
}

/// Emits the `recovery_executed` event.
///
/// Topics: `(Symbol("recovery_executed"),)`
/// Data:    `AdminTransferred { old_admins, new_admins }`
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
    use soroban_sdk::testutils::{Address as _, Events};
    use soroban_sdk::{contract, FromVal, Map, TryFromVal, TryIntoVal, Val};

    #[contract]
    struct DummyContract;

    /// The canonical payload shapes must keep their field names stable: an
    /// indexer decodes these structs by field name, so renaming a field would
    /// silently change the event's data shape for every consumer (issue #518).
    #[test]
    fn admin_transferred_payload_shape_is_canonical() {
        let env = Env::default();
        let old_admins = Vec::from_array(&env, [Address::generate(&env)]);
        let new_admins = Vec::from_array(&env, [Address::generate(&env)]);
        let payload = AdminTransferred {
            old_admins: old_admins.clone(),
            new_admins: new_admins.clone(),
        };

        let val: Val = payload.try_into_val(&env).unwrap();
        let fields: Map<Symbol, Val> = Map::try_from_val(&env, &val).unwrap();

        assert!(fields.contains_key(Symbol::new(&env, "old_admins")));
        assert!(fields.contains_key(Symbol::new(&env, "new_admins")));
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn pending_recovery_payload_shape_is_canonical() {
        let env = Env::default();
        let new_admin = Address::generate(&env);
        let payload = PendingRecovery {
            new_admin,
            execute_after: 123,
            initiated_by: Address::generate(&env),
        };

        let val: Val = payload.try_into_val(&env).unwrap();
        let fields: Map<Symbol, Val> = Map::try_from_val(&env, &val).unwrap();

        assert!(fields.contains_key(Symbol::new(&env, "new_admin")));
        assert!(fields.contains_key(Symbol::new(&env, "execute_after")));
        assert!(fields.contains_key(Symbol::new(&env, "initiated_by")));
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn paused_and_unpaused_payload_arities_are_canonical() {
        let env = Env::default();
        let admin = Address::generate(&env);

        emit_paused(&env, &admin, 7);
        let (_, _, paused_data) = env.events().all().last().unwrap();
        let paused_payload: (Address, bool, u32) = FromVal::from_val(&env, &paused_data);
        assert_eq!(paused_payload, (admin.clone(), true, 7));

        emit_unpaused(&env, &admin);
        let (_, _, unpaused_data) = env.events().all().last().unwrap();
        let unpaused_payload: (Address, bool) = FromVal::from_val(&env, &unpaused_data);
        assert_eq!(unpaused_payload, (admin, false));
    }

    #[test]
    fn shared_event_helpers_emit_canonical_topics_and_data() {
        let env = Env::default();
        let contract_id = env.register_contract(None, DummyContract);
        let old_admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let old_admins = Vec::from_array(&env, [old_admin.clone()]);
        let new_admins = Vec::from_array(&env, [new_admin.clone()]);
        let recovery = Address::generate(&env);
        let merchant = Address::generate(&env);
        let removed = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 12,
            auto_settle: true,
        };
        let fallback = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 25,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        env.as_contract(&contract_id, || {
            emit_threshold_changed(&env, 2, 3);
            emit_paused(&env, &old_admin, 7);
            emit_unpaused(&env, &old_admin);
            emit_admin_transferred(
                &env,
                &AdminTransferred {
                    old_admins: old_admins.clone(),
                    new_admins: new_admins.clone(),
                },
            );
            emit_recovery_initiated(&env, &recovery, &new_admin, 1234);
            emit_recovery_cancelled(&env, &recovery);
            emit_recovery_executed(
                &env,
                &AdminTransferred {
                    old_admins: old_admins.clone(),
                    new_admins: new_admins.clone(),
                },
            );
            emit_settlement_rule_cleared(
                &env,
                &merchant,
                &old_admin,
                &removed,
                &fallback,
            );
        });

        let emitted = env.events().all();
        assert_eq!(emitted.len(), 8);

        let (_, topics, data) = emitted.get(0).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, THRESHOLD_CHANGED_EVENT)
        );
        let (current_threshold, new_threshold): (u32, u32) =
            TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!((current_threshold, new_threshold), (2, 3));

        let (_, topics, data) = emitted.get(1).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, PAUSED_EVENT)
        );
        let (paused_by, paused, reason): (Address, bool, u32) =
            TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(paused_by, old_admin);
        assert!(paused);
        assert_eq!(reason, 7);

        let (_, topics, data) = emitted.get(2).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, UNPAUSED_EVENT)
        );
        let (unpaused_by, unpaused): (Address, bool) =
            TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(unpaused_by, old_admin);
        assert!(!unpaused);

        let (_, topics, data) = emitted.get(3).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, ADMIN_TRANSFERRED_EVENT)
        );
        let payload: AdminTransferred = TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(payload.old_admins, old_admins);
        assert_eq!(payload.new_admins, new_admins);

        let (_, topics, data) = emitted.get(4).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, RECOVERY_INITIATED_EVENT)
        );
        let (actual_recovery, actual_admin, execute_after): (Address, Address, u64) =
            TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(actual_recovery, recovery);
        assert_eq!(actual_admin, new_admin);
        assert_eq!(execute_after, 1234);

        let (_, topics, data) = emitted.get(5).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, RECOVERY_CANCELLED_EVENT)
        );
        let canceller: Address = TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(canceller, recovery);

        let (_, topics, data) = emitted.get(6).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, RECOVERY_EXECUTED_EVENT)
        );
        let payload: AdminTransferred = TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(payload.old_admins, old_admins);
        assert_eq!(payload.new_admins, new_admins);

        let (_, topics, data) = emitted.get(7).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, SETTLEMENT_RULE_CLEARED_EVENT)
        );
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
        let (admin, actual_removed, actual_fallback): (Address, SettlementRule, SettlementRule) =
            TryFromVal::try_from_val(&env, &data).unwrap();
        assert_eq!(admin, old_admin);
        assert_eq!(actual_removed, removed);
        assert_eq!(actual_fallback, fallback);
    }

    #[test]
    fn all_event_topics_are_unique_non_empty_and_valid_symbols() {
        for (index, topic) in ALL_EVENT_TOPICS.iter().enumerate() {
            assert!(!topic.is_empty(), "event topic must not be empty");
            assert!(
                topic.len() <= 32,
                "event topic {topic:?} exceeds the 32-byte Symbol limit"
            );
            assert!(
                !ALL_EVENT_TOPICS[..index].contains(topic),
                "event topic {topic:?} is duplicated"
            );
        }
    }
}
