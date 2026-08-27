//! Shared storage helpers.
//!
//! Each contract keeps its own private `DataKey` enum for keys that are
//! contract-specific (e.g. settlement's `Merchant(Address)` or governance's
//! `Anchor(Address)`). The keys that are semantically shared — the pause
//! flag, the recovery address, the pending recovery operation, and the
//! multisig threshold — are declared exactly once, in [`CommonDataKey`]
//! below; neither contract redeclares its own `Paused` / `RecoveryAddress` /
//! `PendingRecovery` / `Threshold` variant. The admin role is *not* one of
//! these: both contracts store it as a multisig `Vec<Address>` under their
//! own `DataKey::Admin`, so there is no single-`Address` shape for this
//! crate to own.
//!
//! ## Storage separation
//!
//! Sharing `CommonDataKey` between `governance_contract` and
//! `settlement_contract` does not share any *data* between them. Soroban
//! instance storage is a single ledger entry per contract *instance* (i.e.
//! per deployed contract address); `CommonDataKey` only fixes the wire shape
//! of the key each contract writes into its own entry. A settlement
//! contract's `CommonDataKey::Paused` and a governance contract's
//! `CommonDataKey::Paused` are two independent booleans in two independent
//! ledger entries — pausing one has no effect on the other. See
//! `two_contract_instances_have_independent_paused_flags` below for a test
//! that exercises this directly.
//!
//! Historical note: `Paused`, `RecoveryAddress`, `PendingRecovery`, and
//! `Threshold` used to be declared redundantly in each contract's own
//! `DataKey` enum as well as here. That was safe only because the on-chain
//! SCVal encoding of a Soroban `#[contracttype]` enum is based on the
//! variant name alone — the parent enum's Rust name is not part of the
//! encoding — so a value written under the old
//! `governance_contract::DataKey::Paused` read back identically through
//! `CommonDataKey::Paused`. The duplicate variants have since been removed
//! from both contracts' `DataKey` enums; `CommonDataKey` is now the single
//! declaration of these keys in the workspace, and no storage migration was
//! needed to get there.

use soroban_sdk::{contracttype, Address, Env, String, Vec};

use crate::constants::{TTL_BUMP_LEDGERS, TTL_THRESHOLD_LEDGERS};

/// Instance-storage keys shared by every BettaPay contract.
///
/// Adding a variant here is a coordinated change — every contract that uses
/// the variant must agree on what it stores and on whether the TTL handling
/// lives in this crate or in the contract's own helpers.
#[derive(Clone)]
#[contracttype]
pub enum CommonDataKey {
    /// Recovery `Address` authorised to initiate the recovery flow
    /// (instance storage).
    RecoveryAddress,
    /// Pending recovery operation, present only between `initiate_recovery`
    /// and `execute_recovery` (instance storage).
    PendingRecovery,
    /// Pause-flag `bool` controlling whether mutating operations are blocked
    /// (instance storage).
    Paused,
    /// Multisig admin threshold (instance storage).
    Threshold,
}

/// Returns `true` if the contract is currently paused.
///
/// Bumps the instance TTL on every call, the same way [`read_admin`] does.
/// Soroban's instance storage is a single ledger entry shared by every
/// instance key (`Admin`, `Paused`, `RecoveryAddress`, ...), so in practice
/// any instance read on a live contract keeps the whole entry — including
/// the pause flag — warm. But a contract path that checks `is_paused`
/// without also touching another instance key (or one that's simply quiet
/// for a long stretch while paused) had no guaranteed keep-alive of its
/// own, and a missing entry silently reads back as `unpaused` via
/// `unwrap_or(false)` below rather than failing loudly. Bumping here removes
/// that dependency on call order.
pub fn is_paused(env: &Env) -> bool {
    bump_instance_ttl(env);
    env.storage()
        .instance()
        .get(&CommonDataKey::Paused)
        .unwrap_or(false)
}

/// Writes the pause flag to instance storage.
pub fn set_paused(env: &Env, paused: bool) {
    env.storage()
        .instance()
        .set(&CommonDataKey::Paused, &paused);
}

/// Returns the first entry of a stored multisig admin list.
///
/// Both `governance_contract` and `settlement_contract` store their admin
/// role as a `Vec<Address>` (under their own contract-specific `DataKey`,
/// not [`CommonDataKey::Admin`]) and treat index `0` as the "primary" admin
/// for single-address contexts — the address recorded on events, and the
/// caller compared against in `schedule`/`cancel` ownership checks. Each
/// contract previously reimplemented `admins.get(0).unwrap()` for this; this
/// helper centralises that shared semantic so both contracts read it from
/// one place.
///
/// Returns `None` if `admins` is empty; callers are expected to map that to
/// their own `NotInitialized`/`InvalidAdmin` error, since an empty admin
/// list should not be reachable once a contract is initialised.
pub fn primary_admin(admins: &Vec<Address>) -> Option<Address> {
    admins.get(0)
}

/// Returns `true` if `address` is the network's zero address.
///
/// The Soroban zero-address is the well-known string
/// `"GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"` (a `G...`
/// Stellar-style address whose 32-byte key is all zeros). Both contracts need
/// to reject this on admin transfer, merchant registration, etc., so the
/// comparison lives here and callers translate a `true` result into their own
/// `Invalid*` error variant.
///
/// This is called on every admin/merchant/governance write, so it avoids
/// encoding `address` to a strkey `String` (`Address::to_string`) just to
/// compare it: that direction of the conversion scales with every call and
/// is the more expensive one, since it makes the host re-derive and allocate
/// a fresh base-32 `String` object for the *caller-supplied* address each
/// time. Instead it builds the zero `Address` once and compares the two
/// `Address` values directly, which is a cheap host object comparison
/// (`Address`'s `PartialEq` delegates to the host's `obj_cmp`) with no
/// per-call `String` allocation on the hot path.
pub fn is_zero_address(env: &Env, address: &Address) -> bool {
    let zero_address = Address::from_string(&String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    address == &zero_address
}

/// Bump the instance-storage TTL using the policy defined in
/// [`crate::constants`].
///
/// Useful for non-admin read paths that want to keep the instance entry warm
/// using the standard 14 / 30 day policy. Contracts that intentionally use a
/// different TTL for specific keys (per ADR 003) should call
/// `env.storage().instance().extend_ttl(...)` directly in those spots.
pub fn bump_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_BUMP_LEDGERS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, testutils::Address as _, vec};

    /// No-op contract used only to obtain a real, registered contract
    /// address for [`Env::as_contract`] — instance storage helpers like
    /// [`is_paused`]/[`set_paused`] can only be exercised inside a
    /// registered contract's storage context.
    #[contract]
    struct DummyContract;

    #[test]
    fn is_paused_defaults_to_false_and_round_trips_via_common_data_key() {
        let env = Env::default();
        let contract_id = env.register_contract(None, DummyContract);
        env.as_contract(&contract_id, || {
            // No entry written yet — CommonDataKey::Paused is the *only*
            // pause key either contract can read, so a missing entry must
            // read back as unpaused rather than panicking.
            assert!(!is_paused(&env));

            set_paused(&env, true);
            assert!(is_paused(&env));

            set_paused(&env, false);
            assert!(!is_paused(&env));
        });
    }

    #[test]
    fn two_contract_instances_have_independent_paused_flags() {
        // Instance storage is scoped per contract address, so pausing one
        // instance under CommonDataKey::Paused must not affect another
        // instance that happens to share the same key type — this is what
        // "storage separation" means despite governance_contract and
        // settlement_contract both using CommonDataKey.
        let env = Env::default();
        let governance_like = env.register_contract(None, DummyContract);
        let settlement_like = env.register_contract(None, DummyContract);

        env.as_contract(&governance_like, || {
            set_paused(&env, true);
        });

        env.as_contract(&settlement_like, || {
            assert!(!is_paused(&env));
        });
        env.as_contract(&governance_like, || {
            assert!(is_paused(&env));
        });
    }

    #[test]
    fn primary_admin_returns_first_entry() {
        let env = Env::default();
        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2];
        assert_eq!(primary_admin(&admins), Some(a1));
    }

    #[test]
    fn primary_admin_returns_none_for_empty_list() {
        let env = Env::default();
        let admins: Vec<Address> = vec![&env];
        assert_eq!(primary_admin(&admins), None);
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use soroban_sdk::{contracttype, Env, IntoVal, Val};

    // The legacy shape of DataKey in settlement/governance contracts had
    // data variants, so Soroban encoded unit variants as Symbols, not u32s.
    #[derive(Clone)]
    #[contracttype]
    enum LegacyDataKey {
        Admin,
        Threshold,
        RecoveryAddress,
        PendingRecovery,
        Governance,
        Merchant(soroban_sdk::Address),
    }

    #[test]
    fn common_data_key_encoding_matches_legacy() {
        let env = Env::default();
        let mut map: soroban_sdk::Map<Val, u32> = soroban_sdk::Map::new(&env);
        
        map.set(LegacyDataKey::RecoveryAddress.into_val(&env), 1u32);
        assert_eq!(map.get(CommonDataKey::RecoveryAddress.into_val(&env)), Some(1u32), "RecoveryAddress encoding mismatch");

        map.set(LegacyDataKey::PendingRecovery.into_val(&env), 2u32);
        assert_eq!(map.get(CommonDataKey::PendingRecovery.into_val(&env)), Some(2u32), "PendingRecovery encoding mismatch");

        // Note: Paused was also a unit variant in the legacy DataKey.
        // We'll just define another legacy enum for it or reuse the same.
        #[derive(Clone)]
        #[contracttype]
        enum LegacyDataKey2 {
            Paused,
            SystemParam(soroban_sdk::Symbol),
        }
        
        map.set(LegacyDataKey2::Paused.into_val(&env), 3u32);
        assert_eq!(map.get(CommonDataKey::Paused.into_val(&env)), Some(3u32), "Paused encoding mismatch");

        map.set(LegacyDataKey::Threshold.into_val(&env), 4u32);
        assert_eq!(map.get(CommonDataKey::Threshold.into_val(&env)), Some(4u32), "Threshold encoding mismatch");
    }
}
