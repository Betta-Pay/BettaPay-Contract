//! Regression tests pinning the admin-set invariant across recovery.
//!
//! `execute_recovery` rewrites `DataKey::Admin` directly instead of going
//! through `validate_admins_and_threshold`, so these tests pin both halves of
//! the invariant:
//!
//! 1. The set `execute_recovery` installs is always a single, non-empty,
//!    duplicate-free admin with threshold 1 — `get_admin()` never settles on
//!    an empty set, not even when the recovery target is already an admin.
//! 2. `validate_admins_and_threshold` — the guard every other admin-set write
//!    goes through — rejects empty and duplicate sets, both when called
//!    directly and when reached through `transfer_admin` from the recovered
//!    admin.
//!
//! Error codes referenced here: `InvalidAdmin` = 6, `ZeroAddress` = 306,
//! `InvalidThreshold` = 14.

use crate::storage::validate_admins_and_threshold;
use crate::*;
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env, Vec};

use bettapay_common::constants::RECOVERY_DELAY_SECONDS;

use super::{register_governance, setup};

/// Stellar's zero-address (all-zero ed25519 public key) in strkey form.
const ZERO_ADDRESS_STRKEY: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

fn zero_address(env: &Env) -> Address {
    Address::from_string(&soroban_sdk::String::from_str(env, ZERO_ADDRESS_STRKEY))
}

/// Wraps a [`SettlementError`] in the host-level error the `try_*` client
/// methods return, so failures can be asserted without unwinding the test.
fn contract_error(error: SettlementError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(error as u32)
}

/// Initialises a contract with `admins`/`threshold` and returns the client plus
/// the recovery address authorised to call `initiate_recovery`.
fn setup_with_admins(
    env: &Env,
    admins: &Vec<Address>,
    threshold: u32,
) -> (SettlementContractClient<'static>, Address) {
    let recovery_address = Address::generate(env);
    let governance = register_governance(env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(env, &contract_id);
    let deployer = Address::generate(env);
    client.init(
        &deployer,
        admins,
        &threshold,
        &governance,
        &recovery_address,
    );
    (client, recovery_address)
}

/// Runs `initiate_recovery(new_admin)` and warps past the recovery delay so the
/// pending recovery can be executed.
fn recover_to(env: &Env, client: &SettlementContractClient<'static>, new_admin: &Address) {
    client.initiate_recovery(new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();
}

// ---------------------------------------------------------------------------
// validate_admins_and_threshold — the guard itself
// ---------------------------------------------------------------------------

// An empty admin set can never satisfy the threshold check: any threshold is
// either 0 or greater than `admins.len() == 0`, so the guard rejects with
// `InvalidThreshold` (#14) before the `is_empty` branch is ever reached.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn validate_rejects_empty_admin_set_with_threshold_one() {
    let (env, client, _admins, _merchant) = setup();
    let empty: Vec<Address> = Vec::new(&env);
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &empty, 1);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn validate_rejects_empty_admin_set_with_zero_threshold() {
    let (env, client, _admins, _merchant) = setup();
    let empty: Vec<Address> = Vec::new(&env);
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &empty, 0);
    });
}

// Two entries, one distinct admin: the set cannot honour a threshold of 2, and
// even at threshold 1 it silently inflates the signer count. The guard must
// reject it with `InvalidAdmin` (#6).
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn validate_rejects_duplicate_admins() {
    let (env, client, _admins, _merchant) = setup();
    let admin = Address::generate(&env);
    let duplicated = soroban_sdk::vec![&env, admin.clone(), admin];
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &duplicated, 2);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn validate_rejects_duplicate_admins_at_threshold_one() {
    let (env, client, _admins, _merchant) = setup();
    let admin = Address::generate(&env);
    let duplicated = soroban_sdk::vec![&env, admin.clone(), admin];
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &duplicated, 1);
    });
}

// The duplicate need not be adjacent — the guard compares every pair.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn validate_rejects_non_adjacent_duplicate_admins() {
    let (env, client, _admins, _merchant) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, first.clone(), second, first];
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &admins, 2);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #306)")]
fn validate_rejects_zero_address_in_admin_set() {
    let (env, client, _admins, _merchant) = setup();
    let admins = soroban_sdk::vec![&env, Address::generate(&env), zero_address(&env)];
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &admins, 2);
    });
}

#[test]
fn validate_accepts_distinct_admin_set() {
    let (env, client, _admins, _merchant) = setup();
    let admins = soroban_sdk::vec![&env, Address::generate(&env), Address::generate(&env)];
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &admins, 2);
    });
}

// ---------------------------------------------------------------------------
// post-recovery admin set
// ---------------------------------------------------------------------------

// `execute_recovery` writes `DataKey::Admin` without calling the guard, so pin
// that what it installs would pass the guard: exactly one admin, threshold 1.
#[test]
fn recovery_settles_on_non_empty_single_admin_set() {
    let env = Env::default();
    env.mock_all_auths();
    let admins = soroban_sdk::vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env)
    ];
    let (client, _recovery_address) = setup_with_admins(&env, &admins, 2);
    let new_admin = Address::generate(&env);

    recover_to(&env, &client, &new_admin);

    let recovered = client.get_admin();
    assert_eq!(recovered, soroban_sdk::vec![&env, new_admin]);
    assert_eq!(
        recovered.len(),
        1,
        "recovery must not settle on an empty set"
    );
    let threshold = client.get_threshold();
    assert_eq!(threshold, 1);

    // The installed set satisfies the same guard every other admin-set write
    // must pass — no panic here means empty/duplicate/zero are all ruled out.
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &recovered, threshold);
    });
}

// Recovering to an address that is already an admin must collapse to a single
// entry rather than leaving that address in the set twice.
#[test]
fn recovery_to_existing_admin_does_not_duplicate_admin_set() {
    let env = Env::default();
    env.mock_all_auths();
    let incumbent = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, incumbent.clone(), Address::generate(&env)];
    let (client, _recovery_address) = setup_with_admins(&env, &admins, 2);

    recover_to(&env, &client, &incumbent);

    let recovered = client.get_admin();
    assert_eq!(recovered, soroban_sdk::vec![&env, incumbent]);
    assert_eq!(
        recovered.len(),
        1,
        "recovering to a sitting admin must not duplicate the entry"
    );
    env.as_contract(&client.address, || {
        validate_admins_and_threshold(&env, &recovered, 1);
    });
}

// The zero-address must never reach the pending recovery record in the first
// place, otherwise `execute_recovery` would install an admin set the guard
// would have rejected.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn initiate_recovery_rejects_zero_address_target() {
    let (env, client, _admins, _merchant) = setup();
    client.initiate_recovery(&zero_address(&env));
}

// ---------------------------------------------------------------------------
// transfer_admin from the recovered admin
// ---------------------------------------------------------------------------

// The recovered admin is a lone signer at threshold 1 — the state in which an
// unguarded `transfer_admin` would be easiest to lock the contract with.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn recovered_admin_cannot_transfer_to_empty_admin_set() {
    let env = Env::default();
    env.mock_all_auths();
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let (client, _recovery_address) = setup_with_admins(&env, &admins, 1);
    let new_admin = Address::generate(&env);
    recover_to(&env, &client, &new_admin);

    let signers = soroban_sdk::vec![&env, new_admin];
    let empty: Vec<Address> = Vec::new(&env);
    client.transfer_admin(&signers, &empty, &1);
}

// The duplicate-target case: handing `transfer_admin` the same address twice
// must be rejected with `InvalidAdmin` (#6) rather than storing a set whose
// length overstates how many distinct keys can actually sign.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn recovered_admin_cannot_transfer_to_duplicate_admin_set() {
    let env = Env::default();
    env.mock_all_auths();
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let (client, _recovery_address) = setup_with_admins(&env, &admins, 1);
    let recovered_admin = Address::generate(&env);
    recover_to(&env, &client, &recovered_admin);
    assert_eq!(
        client.get_admin(),
        soroban_sdk::vec![&env, recovered_admin.clone()]
    );

    let signers = soroban_sdk::vec![&env, recovered_admin];
    let duplicate_target = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, duplicate_target.clone(), duplicate_target];
    client.transfer_admin(&signers, &new_admins, &2);
}

// A rejected transfer must leave the recovered admin set untouched.
#[test]
fn rejected_duplicate_transfer_leaves_recovered_admin_set_intact() {
    let env = Env::default();
    env.mock_all_auths();
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let (client, _recovery_address) = setup_with_admins(&env, &admins, 1);
    let recovered_admin = Address::generate(&env);
    recover_to(&env, &client, &recovered_admin);

    let signers = soroban_sdk::vec![&env, recovered_admin.clone()];
    let duplicate_target = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, duplicate_target.clone(), duplicate_target];

    let result = client.try_transfer_admin(&signers, &new_admins, &2);
    assert_eq!(
        result,
        Err(Ok(contract_error(SettlementError::InvalidAdmin)))
    );

    let empty: Vec<Address> = Vec::new(&env);
    let empty_result = client.try_transfer_admin(&signers, &empty, &1);
    assert_eq!(
        empty_result,
        Err(Ok(contract_error(SettlementError::InvalidThreshold)))
    );

    assert_eq!(
        client.get_admin(),
        soroban_sdk::vec![&env, recovered_admin],
        "a rejected transfer must not disturb the stored admin set"
    );
    assert_eq!(client.get_threshold(), 1);
}
