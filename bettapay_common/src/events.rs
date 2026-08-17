//! Shared event payloads and emission helpers.
//!
//! Both contracts publish events under a small set of topic names and use
//! the same pair of structured payload types (`AdminTransferred`,
//! `PendingRecovery`). Putting them here so the two contracts cannot diverge.

use soroban_sdk::{contracttype, Address, Env, Symbol};

/// Structured payload emitted with the `admin_transferred` event so that
/// off-chain consumers can read the old and new admin by field name rather
/// than by positional order.
#[derive(Clone)]
#[contracttype]
pub struct AdminTransferred {
    pub old_admin: Address,
    pub new_admin: Address,
}

/// Structured payload emitted with the `anchor_upserted` event so that
/// off-chain indexers can reconstruct an unambiguous per-asset history even
/// when rapid upserts land in the same ledger (issue #584).
///
/// `version` is a per-asset monotonically increasing counter starting at 1
/// for the first upsert. It is persisted alongside the anchor entry and
/// survives across successive upserts and removals of the same asset.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct AnchorUpserted {
    /// Previous anchor address for this asset, or `None` if this is the
    /// first upsert (or the prior entry was removed / expired).
    pub previous: Option<Address>,
    /// The newly stored anchor address.
    pub current: Address,
    /// Per-asset monotonic version counter (starts at 1, increments on
    /// every upsert). Allows indexers to order events unambiguously.
    pub version: u64,
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

// Event-name strings.
//
// Soroban `Symbol`s are limited to 32 bytes; every value here comfortably
// fits. Centralising them in this crate keeps the topic name identical across
// the contracts.

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

/// Topic emitted when an anchor is created or replaced for an asset.
pub const ANCHOR_UPSERTED_EVENT: &str = "anchor_upserted";

/// Topic emitted when an anchor is removed for an asset.
pub const ANCHOR_REMOVED_EVENT: &str = "anchor_removed";

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

/// Emits the `anchor_upserted` event with the structured
/// [`AnchorUpserted`] payload (issue #584).
///
/// Topics: `(Symbol("anchor_upserted"), asset)`
/// Data:    `AnchorUpserted { previous, current, version }`
pub fn emit_anchor_upserted(env: &Env, asset: &Address, payload: &AnchorUpserted) {
    env.events().publish(
        (Symbol::new(env, ANCHOR_UPSERTED_EVENT), asset.clone()),
        payload.clone(),
    );
}

/// Emits the `anchor_removed` event.
///
/// Topics: `(Symbol("anchor_removed"), asset)`
/// Data:    `version` — the last version counter for this asset, so
///          indexers can correlate the removal with the prior upsert chain.
pub fn emit_anchor_removed(env: &Env, asset: &Address, version: u64) {
    env.events().publish(
        (Symbol::new(env, ANCHOR_REMOVED_EVENT), asset.clone()),
        version,
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

    /// Issue #584: `AnchorUpserted` must keep its field names stable so
    /// indexers can decode by name. The `version` field is the ordering
    /// metadata that makes rapid upserts unambiguous.
    #[test]
    fn anchor_upserted_payload_shape_is_canonical() {
        let env = Env::default();
        let current = Address::generate(&env);
        let payload = AnchorUpserted {
            previous: None,
            current,
            version: 1,
        };

        let val: Val = payload.try_into_val(&env).unwrap();
        let fields: Map<Symbol, Val> = Map::try_from_val(&env, &val).unwrap();

        assert!(fields.contains_key(Symbol::new(&env, "previous")));
        assert!(fields.contains_key(Symbol::new(&env, "current")));
        assert!(fields.contains_key(Symbol::new(&env, "version")));
        assert_eq!(fields.len(), 3);
    }
}
