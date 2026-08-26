//! Regression coverage for the settlement administrative timelock.

use crate::{Operation, DEFAULT_TIMELOCK_DELAY_SECONDS};
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::Address;

use super::setup;

#[test]
fn scheduled_operation_executes_only_after_delay() {
    let (env, client, admins, _) = setup();
    let new_admin = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, new_admin.clone()];
    let operation = Operation::TransferAdmin(new_admins.clone(), 1);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_execute(&operation).is_err());
    assert_eq!(client.get_admin(), admins);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
    assert_eq!(client.get_threshold(), 1);
    assert!(client.try_execute(&operation).is_err());
}

#[test]
fn schedule_rejects_non_admin_and_insufficient_delay() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let non_admin = Address::generate(&env);

    assert!(client
        .try_schedule(&soroban_sdk::vec![&env, non_admin], &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
    assert!(client
        .try_schedule(
            &admins,
            &operation,
            &(DEFAULT_TIMELOCK_DELAY_SECONDS - 1),
        )
        .is_err());
}

#[test]
fn duplicate_schedule_is_rejected() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
}

#[test]
fn admin_can_cancel_but_non_admin_cannot() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_cancel(&soroban_sdk::vec![&env, Address::generate(&env)], &operation)
        .is_err());
    client.cancel(&admins, &operation);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_execute(&operation).is_err());
    assert!(client
        .try_cancel(&soroban_sdk::vec![&env, admins.get(0).unwrap()], &operation)
        .is_err());
}

#[test]
fn multisig_schedule_and_cancel_require_two_of_three_signers() {
    let (env, client, admins, merchant) = setup_multisig();
    let operation = Operation::RegisterMerchant(merchant);
    let one_signer = soroban_sdk::vec![&env, admins.get(0).unwrap()];
    let two_signers = soroban_sdk::vec![&env, admins.get(0).unwrap(), admins.get(1).unwrap()];

    assert!(client
        .try_schedule(&one_signer, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
    client.schedule(&two_signers, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_cancel(&one_signer, &operation).is_err());
    client.cancel(&two_signers, &operation);
    assert!(client.try_execute(&operation).is_err());
}

#[test]
fn multisig_schedule_and_execute_apply_operation_after_delay() {
    let (env, client, admins, merchant) = setup_multisig();
    let operation = Operation::RegisterMerchant(merchant.clone());
    let two_signers = soroban_sdk::vec![&env, admins.get(0).unwrap(), admins.get(1).unwrap()];

    client.schedule(&two_signers, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(!client.is_merchant_registered(&merchant));

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS - 1);
    assert!(client.try_execute(&operation).is_err());
    assert!(!client.is_merchant_registered(&merchant));

    env.ledger().with_mut(|ledger| ledger.timestamp += 1);
    client.execute(&operation);
    assert!(client.is_merchant_registered(&merchant));
}

#[test]
#[should_panic(expected = "Error(Storage, InternalError)")]
fn expired_schedule_cannot_execute() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(
        &admins,
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    // `schedule` bumps the persistent entry to 30 days (518,400 ledgers).
    // Keep the contract instance alive while advancing past only the
    // scheduled operation's TTL.
    for _ in 0..5 {
        env.ledger()
            .with_mut(|ledger| ledger.sequence_number += 100_000);
        client.get_admin();
    }
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS + 1;
        ledger.sequence_number += 18_401;
    });

    // The host rejects access to an archived key before the contract can map
    // it to `OperationNotScheduled`, so expiry is observed as a host panic in
    // the in-memory test environment.
    client.execute(&operation);
}

fn setup_multisig() -> (Env, SettlementContractClient<'static>, soroban_sdk::Vec<Address>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, a1, a2, a3];
    let recovery = Address::generate(&env);
    let governance = super::register_governance(&env);
    let contract_id = env.register_contract(None, crate::SettlementContract);
    let client = crate::SettlementContractClient::new(&env, &contract_id);
    client.init(&admins, &2, &governance, &recovery);
    let merchant = Address::generate(&env);
    (env, client, admins, merchant)
}

// ---------------------------------------------------------------------------
// Issue #472: the timelocked upgrade path must validate the Wasm hash the
// same way the direct `upgrade` entry point does.
// ---------------------------------------------------------------------------

/// A scheduled upgrade to Wasm that does not implement the required interface
/// (here: an uploaded but empty Wasm with no `supports_interface` export)
/// must be rejected at execution time with the typed `InvalidWasmInterface`
/// (#13), leaving the running code untouched.
#[test]
fn timelocked_upgrade_rejects_non_conforming_wasm() {
    let (env, client, admins, _) = setup();
    let admin = admins.get(0).unwrap();
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    let operation = Operation::Upgrade(bad_hash);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    // The typed contract error (#13), not a raw host panic, is raised.
    match client.try_execute(&operation) {
        Err(Ok(e)) => assert_eq!(e, soroban_sdk::Error::from_contract_error(13)),
        other => panic!("expected InvalidWasmInterface (#13), got: {other:?}"),
    }

    // The contract remains operational after the rejected upgrade.
    assert_eq!(client.get_admin(), admins);
}

/// A scheduled upgrade to a Wasm hash that was never uploaded cannot even be
/// probed: protocol 21 exposes no wasm-presence check to contract code, so
/// the probe deployment traps with a host-level `Storage`/`MissingValue`
/// error ("Wasm does not exist") before `InvalidWasmInterface` can be raised.
/// Documented here — mirroring `expired_schedule_cannot_execute` — so the
/// boundary of the typed-error guarantee is explicit. See
/// `bettapay_common::upgrade::probe_supports_interface`.
#[test]
#[should_panic(expected = "Error(Storage, MissingValue)")]
fn timelocked_upgrade_rejects_never_uploaded_wasm() {
    let (env, client, admins, _) = setup();
    let admin = admins.get(0).unwrap();
    let garbage = soroban_sdk::BytesN::from_array(&env, &[0x47u8; 32]);
    let operation = Operation::Upgrade(garbage);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);
}

/// Issue #473: the scheduled path must publish `contract_upgraded` at the
/// same logical point as the direct path — after the interface check,
/// immediately before the code swap. A rejected scheduled upgrade therefore
/// emits no events at all (neither `contract_upgraded` nor the wrapping
/// `op_executed`), pinning the event's position relative to validation.
#[test]
fn rejected_timelocked_upgrade_emits_no_events() {
    let (env, client, admins, _) = setup();
    let admin = admins.get(0).unwrap();
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    let operation = Operation::Upgrade(bad_hash);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    let before = env.events().all().len();
    assert!(client.try_execute(&operation).is_err());
    assert_eq!(
        env.events().all().len(),
        before,
        "no contract_upgraded or op_executed event on failed upgrade"
    );
}

// ---------------------------------------------------------------------------
// Issue #2: TransferAdmin parity — timelocked path must accept the same
// (Vec<Address>, u32) shape as the direct transfer_admin entry point.
// ---------------------------------------------------------------------------

/// Verifies that `Operation::TransferAdmin` now carries the full admin set +
/// threshold, matching the direct `transfer_admin` entry point in shape and
/// effect.  A multi-member admin set with threshold > 1 is used to confirm the
/// timelocked path writes the complete configuration, not just a single address.
#[test]
fn timelocked_transfer_admin_parity_with_direct_path() {
    use crate::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::testutils::Ledger;
    use soroban_sdk::Env;

    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let recovery = Address::generate(&env);

    let governance = super::register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    let initial_admins = soroban_sdk::vec![&env, a1.clone()];
    client.init(&initial_admins, &1, &governance, &recovery);

    // New admin set: three members, threshold 2 — same shape the direct path accepts.
    let new_admins = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let new_threshold: u32 = 2;

    // --- Direct path ---
    client.transfer_admin(&initial_admins, &new_admins, &new_threshold);
    assert_eq!(client.get_admin(), new_admins, "direct path stores full admin set");
    assert_eq!(client.get_threshold(), new_threshold, "direct path stores threshold");

    // Reset back to single-admin so the timelock path starts from a clean state.
    let reset_admins = soroban_sdk::vec![&env, a1.clone()];
    client.transfer_admin(&new_admins, &reset_admins, &1);

    // --- Timelocked path ---
    let operation = Operation::TransferAdmin(new_admins.clone(), new_threshold);
    client.schedule(&soroban_sdk::vec![&env, a1.clone()], &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);

    assert_eq!(
        client.get_admin(),
        new_admins,
        "timelocked path stores the same full admin set as the direct path"
    );
    assert_eq!(
        client.get_threshold(),
        new_threshold,
        "timelocked path stores the same threshold as the direct path"
    );
}
