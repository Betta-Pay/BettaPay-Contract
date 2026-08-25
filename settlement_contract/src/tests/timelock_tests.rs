//! Regression coverage for the settlement administrative timelock.

use crate::{
    Operation, DEFAULT_TIMELOCK_DELAY_SECONDS, SCHEDULED_OP_TTL_BUMP, SCHEDULED_OP_TTL_THRESHOLD,
};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::Address;

use super::setup;

#[test]
fn scheduled_operation_executes_only_after_delay() {
    let (env, client, admins, _) = setup();
    let admin = admins.get(0).unwrap();
    let new_admin = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, new_admin.clone()];
    let operation = Operation::TransferAdmin(new_admins.clone(), 1);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
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
        .try_schedule(&non_admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
    assert!(client
        .try_schedule(
            &admins.get(0).unwrap(),
            &operation,
            &(DEFAULT_TIMELOCK_DELAY_SECONDS - 1),
        )
        .is_err());
}

#[test]
fn duplicate_schedule_is_rejected() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let admin = admins.get(0).unwrap();

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
}

#[test]
fn admin_can_cancel_but_non_admin_cannot() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let admin = admins.get(0).unwrap();

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_cancel(&Address::generate(&env), &operation)
        .is_err());
    client.cancel(&admin, &operation);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_execute(&operation).is_err());
    assert!(client.try_cancel(&admin, &operation).is_err());
}

#[test]
#[should_panic(expected = "Error(Storage, InternalError)")]
fn expired_schedule_cannot_execute() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(
        &admins.get(0).unwrap(),
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
    client.schedule(&a1, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

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

// ---------------------------------------------------------------------------
// Scheduled-operation TTL must match the named policy constants.
// ---------------------------------------------------------------------------

/// Asserts that the persistent TTL applied by `schedule` to the
/// `ScheduledOperation` key equals `SCHEDULED_OP_TTL_BUMP`. This prevents the
/// schedule path from drifting from the policy constants (see ADR 003 and
/// issue #474).
#[test]
fn scheduled_operation_ttl_matches_policy_constants() {
    use crate::types::DataKey;
    use soroban_sdk::testutils::storage::Persistent;
    use soroban_sdk::xdr::ToXdr;

    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(
        &admins.get(0).unwrap(),
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let op_hash: soroban_sdk::BytesN<32> =
        env.crypto().sha256(&operation.to_xdr(&env)).into();
    let key = DataKey::ScheduledOperation(op_hash);

    let contract_id = client.address.clone();
    let ttl = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&key)
    });

    // Soroban `extend_ttl(key, threshold, bump)` sets the TTL to `bump` when
    // the current TTL is below `threshold`.
    assert_eq!(
        ttl,
        SCHEDULED_OP_TTL_BUMP,
        "scheduled-operation TTL must equal SCHEDULED_OP_TTL_BUMP"
    );
}
