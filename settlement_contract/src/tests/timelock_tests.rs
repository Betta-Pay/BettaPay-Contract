//! Regression coverage for the settlement administrative timelock.

use crate::{Operation, SettlementContractClient, SettlementRule, DEFAULT_TIMELOCK_DELAY_SECONDS};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

use super::setup;

#[test]
fn scheduled_operation_executes_only_after_delay() {
    let (env, client, admins, _) = setup();
    let new_admin = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, new_admin.clone()];
    let operation = Operation::TransferAdmin(new_admins.clone(), 1);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
    assert_eq!(client.get_admin(), admins);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap(), &operation);

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
    assert_eq!(client.get_threshold(), 1);
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
}

#[test]
fn schedule_rejects_non_admin_and_insufficient_delay() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let non_admin = Address::generate(&env);

    assert!(client
        .try_schedule(
            &soroban_sdk::vec![&env, non_admin],
            &operation,
            &DEFAULT_TIMELOCK_DELAY_SECONDS
        )
        .is_err());
    assert!(client
        .try_schedule(&admins, &operation, &(DEFAULT_TIMELOCK_DELAY_SECONDS - 1),)
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
        .try_cancel(
            &soroban_sdk::vec![&env, Address::generate(&env)],
            &operation
        )
        .is_err());
    client.cancel(&admins, &operation);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
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
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
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
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
    assert!(!client.is_merchant_registered(&merchant));

    env.ledger().with_mut(|ledger| ledger.timestamp += 1);
    client.execute(&admins.get(0).unwrap(), &operation);
    assert!(client.is_merchant_registered(&merchant));
}

#[test]
#[should_panic(expected = "Error(Storage, InternalError)")]
fn expired_schedule_cannot_execute() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

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
    client.execute(&admins.get(0).unwrap(), &operation);
}

fn setup_multisig() -> (
    Env,
    SettlementContractClient<'static>,
    soroban_sdk::Vec<Address>,
    Address,
) {
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
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &governance, &recovery);
    let merchant = Address::generate(&env);
    (env, client, admins, merchant)
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
    let deployer = Address::generate(&env);
    client.init(&deployer, &initial_admins, &1, &governance, &recovery);

    // New admin set: three members, threshold 2 — same shape the direct path accepts.
    let new_admins = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let new_threshold: u32 = 2;

    // --- Direct path ---
    client.transfer_admin(&initial_admins, &new_admins, &new_threshold);
    assert_eq!(
        client.get_admin(),
        new_admins,
        "direct path stores full admin set"
    );
    assert_eq!(
        client.get_threshold(),
        new_threshold,
        "direct path stores threshold"
    );

    // Reset back to single-admin so the timelock path starts from a clean state.
    let reset_admins = soroban_sdk::vec![&env, a1.clone()];
    client.transfer_admin(&new_admins, &reset_admins, &1);

    // --- Timelocked path ---
    let operation = Operation::TransferAdmin(new_admins.clone(), new_threshold);
    client.schedule(
        &soroban_sdk::vec![&env, a1.clone()],
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&a1.clone(), &operation);

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
// Timelock Pause-Gating Tests
// ---------------------------------------------------------------------------

#[test]
fn schedule_rejects_when_contract_is_paused() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.pause(&admins);
    assert!(client.is_paused());

    let result = client.try_schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(
        result.is_err(),
        "schedule must be rejected while contract is paused"
    );
}

#[test]
fn cancel_rejects_when_contract_is_paused() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    // Schedule while active
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    // Pause contract
    client.pause(&admins);

    let result = client.try_cancel(&admins, &operation);
    assert!(
        result.is_err(),
        "cancel must be rejected while contract is paused"
    );

    // Unpause contract and verify cancel now works
    client.unpause(&admins);
    client.cancel(&admins, &operation);
}

#[test]
fn execute_rejects_when_contract_is_paused_and_preserves_scheduled_op() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant.clone());

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    // Pause contract
    client.pause(&admins);

    let result = client.try_execute(&admins.get(0).unwrap(), &operation);
    assert!(
        result.is_err(),
        "execute must be rejected while contract is paused"
    );
    assert!(!client.is_merchant_registered(&merchant));

    // Unpause contract and verify execution succeeds
    client.unpause(&admins);
    client.execute(&admins.get(0).unwrap(), &operation);
    assert!(client.is_merchant_registered(&merchant));
}

// ---------------------------------------------------------------------------
// Uniform execution auth policy (issue #561)
// ---------------------------------------------------------------------------

/// Verifies the uniform `execute` auth policy across **every** `Operation`
/// variant: once an operation has been scheduled by the admins and its
/// timelock delay has elapsed, `execute` performs **no caller authentication**
/// (issue #693). Authorization is enforced at `schedule`/`cancel`, never
/// inside `execute`.
///
/// Caller-auth mocking is disabled (`set_auths(&[])`) before the executions,
/// so any `require_auth` inside `execute` would fail with `Unauthorized` and
/// break the test. `CancelRecovery` — the variant that historically required
/// primary-admin auth (issue #561) — is executed under the same no-auth
/// conditions as every other variant.
#[test]
fn test_execute_uniform_auth_all_variants() {
    let (env, client, admins, _) = setup();

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 100,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // Schedule every variant while auth mocking is enabled — `schedule` is the
    // admin-multisig-gated boundary.
    let new_gov = super::register_governance(&env);
    let op_update_governance = Operation::UpdateGovernance(new_gov.clone());
    client.schedule(
        &admins,
        &op_update_governance,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let recovery_target = Address::generate(&env);
    client.initiate_recovery(&recovery_target);
    let op_cancel_recovery = Operation::CancelRecovery;
    client.schedule(
        &admins,
        &op_cancel_recovery,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let new_admin = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, new_admin.clone()];
    let op_transfer_admin = Operation::TransferAdmin(new_admins.clone(), 1);
    client.schedule(&admins, &op_transfer_admin, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    // The empty-Wasm upgrade is executed last (see below): the test host
    // accepts empty Wasm and the `execute`-path `_upgrade` performs no
    // interface probe, so this arm succeeds rather than erroring.
    let empty_wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let empty_hash = env.deployer().upload_contract_wasm(empty_wasm);
    let op_upgrade = Operation::Upgrade(empty_hash);
    client.schedule(&admins, &op_upgrade, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    let merchant = Address::generate(&env);
    let op_register_merchant = Operation::RegisterMerchant(merchant.clone());
    client.schedule(
        &admins,
        &op_register_merchant,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let merchant2 = Address::generate(&env);
    client.register_merchant(&admins, &merchant2);
    let op_unregister_merchant = Operation::UnregisterMerchant(merchant2.clone());
    client.schedule(
        &admins,
        &op_unregister_merchant,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let merchant3 = Address::generate(&env);
    client.register_merchant(&admins, &merchant3);
    let op_set_settlement_rule = Operation::SetSettlementRule(merchant3.clone(), rule.clone());
    client.schedule(
        &admins,
        &op_set_settlement_rule,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let merchant4 = Address::generate(&env);
    client.register_merchant(&admins, &merchant4);
    client.set_settlement_rule(&admins, &merchant4, &rule);
    let op_clear_settlement_rule = Operation::ClearSettlementRule(merchant4.clone());
    client.schedule(
        &admins,
        &op_clear_settlement_rule,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let op_set_default_rule = Operation::SetDefaultRule(rule.clone());
    client.schedule(
        &admins,
        &op_set_default_rule,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    // Ripen every scheduled operation.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    // Disable caller-auth mocking: any `require_auth` inside `execute` now
    // fails with `Unauthorized`. Every variant must still execute.
    env.set_auths(&[]);

    client.execute(&admins.get(0).unwrap(), &op_update_governance);
    assert_eq!(client.get_governance(), new_gov);

    client.execute(&admins.get(0).unwrap(), &op_cancel_recovery);
    assert!(client.try_execute_recovery().is_err());

    client.execute(&admins.get(0).unwrap(), &op_transfer_admin);
    assert_eq!(client.get_admin(), new_admins);
    assert_eq!(client.get_threshold(), 1);

    client.execute(&admins.get(0).unwrap(), &op_register_merchant);
    assert!(client.is_merchant_registered(&merchant));

    client.execute(&admins.get(0).unwrap(), &op_unregister_merchant);
    assert!(!client.is_merchant_registered(&merchant2));

    client.execute(&admins.get(0).unwrap(), &op_set_settlement_rule);
    let stored_rule = client.get_settlement_rule(&merchant3).unwrap();
    assert_eq!(stored_rule.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(stored_rule.network_fee_bps, rule.network_fee_bps);
    assert_eq!(
        stored_rule.settlement_delay_ledger,
        rule.settlement_delay_ledger
    );
    assert_eq!(stored_rule.auto_settle, rule.auto_settle);

    client.execute(&admins.get(0).unwrap(), &op_clear_settlement_rule);
    assert!(client.get_settlement_rule(&merchant4).is_none());

    client.execute(&admins.get(0).unwrap(), &op_set_default_rule);
    let stored_default = client.get_default_rule().unwrap();
    assert_eq!(stored_default.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(stored_default.network_fee_bps, rule.network_fee_bps);
    assert_eq!(
        stored_default.settlement_delay_ledger,
        rule.settlement_delay_ledger
    );
    assert_eq!(stored_default.auto_settle, rule.auto_settle);

    // `Upgrade` runs last: the test host lets an empty Wasm stand in for a
    // valid contract, and `execute`'s `_upgrade` (unlike the admin-gated
    // `upgrade` path) does not probe `supports_interface`. So this arm
    // succeeds — and succeeding with caller-auth mocking disabled is the
    // proof that it has no auth gate either.
    client.execute(&admins.get(0).unwrap(), &op_upgrade);
}

/// Focused regression for the variant named in issue #561: a scheduled
/// `CancelRecovery` must be executable by a caller with **no auth** once the
/// timelock has elapsed (issue #693). Before the normalization, this arm
/// required the primary admin to sign the `execute` transaction.
#[test]
fn scheduled_cancel_recovery_executes_without_caller_auth() {
    let (env, client, admins, _) = setup();

    let recovery_target = Address::generate(&env);
    client.initiate_recovery(&recovery_target);

    let op = Operation::CancelRecovery;
    client.schedule(&admins, &op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    // No caller auth is mocked: the old primary-admin `require_auth` would
    // fail here with `Unauthorized`.
    env.set_auths(&[]);
    client.execute(&admins.get(0).unwrap(), &op);

    // The pending recovery is gone.
    assert!(client.try_execute_recovery().is_err());
}
