//! Regression coverage for the settlement administrative timelock.

use crate::types::{DataKey, ScheduledOp};
use crate::{
    Operation, SettlementContractClient, SettlementError, SettlementRule,
    DEFAULT_TIMELOCK_DELAY_SECONDS, MAX_TIMELOCK_DELAY_SECONDS, MIN_TIMELOCK_DELAY_SECONDS,
};
use bettapay_common::constants::RECOVERY_DELAY_SECONDS;
use bettapay_common::events;
use bettapay_common::events::AdminTransferred;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol, TryFromVal};

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

    assert_eq!(
        client.get_admin(),
        soroban_sdk::vec![&env, new_admin.clone()]
    );
    assert_eq!(client.get_threshold(), 1);
}

#[test]
fn recovery_vetoes_scheduled_operation_before_timelock_expiry() {
    let (env, client, admins, _merchant) = setup();
    let operation = Operation::TransferAdmin(soroban_sdk::vec![&env, Address::generate(&env)], 1);
    let admin_signers = soroban_sdk::vec![&env, admins.get(0).unwrap()];

    client.schedule(&admin_signers, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += RECOVERY_DELAY_SECONDS;
    });
    client.initiate_recovery(&Address::generate(&env));

    // Recovery begins at the same boundary as the timelock and must win the
    // transaction race: a scheduled operation cannot execute while recovery
    // is pending, even when its nominal delay has elapsed.
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS;
    });
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
    assert_eq!(client.get_admin(), admins);
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

    // The scheduled operation was consumed by `cancel`: execution after the
    // delay fails, and a second cancel is rejected as well.
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
    assert!(client
        .try_execute(&two_signers.get(0).unwrap(), &operation)
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
    assert!(client
        .try_execute(&two_signers.get(0).unwrap(), &operation)
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
    client.execute(&reset_admins.get(0).unwrap(), &operation);

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

/// Issue #995: pins the documented pause behavior for the timelocked queue.
/// `schedule`, `execute` and `cancel` are all rejected with `Paused` while the
/// contract is paused, a queued operation survives untouched, and the direct
/// incident-response path (`initiate_recovery`, `unpause`) stays available.
#[test]
fn paused_timelock_operations_are_rejected_and_queue_survives() {
    let (env, client, admins, merchant) = setup();
    let queued = Operation::RegisterMerchant(merchant.clone());
    let other = Operation::UnregisterMerchant(merchant.clone());

    client.schedule(&admins, &queued, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    client.pause(&admins);

    assert_eq!(
        client.try_schedule(&admins, &other, &DEFAULT_TIMELOCK_DELAY_SECONDS),
        Err(Ok(SettlementError::Paused))
    );
    assert_eq!(
        client.try_execute(&admins.get(0).unwrap(), &queued),
        Err(Ok(SettlementError::Paused))
    );
    assert_eq!(
        client.try_cancel(&admins, &queued),
        Err(Ok(SettlementError::Paused))
    );

    // Incident-response entry points are not gated by pause.
    client.initiate_recovery(&Address::generate(&env));

    // The queue was not consumed or altered: after unpausing, the original
    // entry is still there to be cancelled.
    client.unpause(&admins);
    client.cancel(&admins, &queued);
}

// ---------------------------------------------------------------------------
// Governance update validation on the scheduled path (issue #562)
// ---------------------------------------------------------------------------

/// The scheduled path (`Operation::UpdateGovernance` executed through the
/// timelock) must enforce the exact same `validate_governance` check as the
/// direct `update_governance` entry point. `schedule` is admin-gated but does
/// not inspect the operation payload, so a zero governance address must be
/// rejected with `InvalidGovernance` (#309) at execution time instead of
/// being stored.
#[test]
#[should_panic(expected = "Error(Contract, #309)")]
fn scheduled_update_governance_rejects_zero_address() {
    let (env, client, admins, _) = setup();
    let zero_address = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    let operation = Operation::UpdateGovernance(zero_address);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    // The scheduled path must fail the same validation the direct path does.
    client.execute(&admins.get(0).unwrap(), &operation);
}

/// The scheduled path accepts a governance address that passes
/// `validate_governance`, mirroring the direct path.
#[test]
fn scheduled_update_governance_accepts_valid_address() {
    let (env, client, admins, _) = setup();
    let new_governance = super::register_governance(&env);
    let operation = Operation::UpdateGovernance(new_governance.clone());

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    client.execute(&admins.get(0).unwrap(), &operation);

    assert_eq!(client.get_governance(), new_governance);
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
    let dummy_executor = Address::generate(&env);

    // CancelRecovery runs first: `execute` vetoes every scheduled operation
    // while a recovery is pending (issue #501) except `CancelRecovery` itself,
    // so clearing the pending marker first lets the remaining variants through.
    client.execute(&dummy_executor, &op_cancel_recovery);
    assert!(client.try_execute_recovery().is_err());

    client.execute(&dummy_executor, &op_update_governance);
    assert_eq!(client.get_governance(), new_gov);

    client.execute(&dummy_executor, &op_transfer_admin);
    assert_eq!(client.get_admin(), new_admins);
    assert_eq!(client.get_threshold(), 1);

    client.execute(&dummy_executor, &op_register_merchant);
    assert!(client.is_merchant_registered(&merchant));

    client.execute(&dummy_executor, &op_unregister_merchant);
    assert!(!client.is_merchant_registered(&merchant2));

    client.execute(&dummy_executor, &op_set_settlement_rule);
    let stored_rule = client.get_settlement_rule(&merchant3).unwrap();
    assert_eq!(stored_rule.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(stored_rule.network_fee_bps, rule.network_fee_bps);
    assert_eq!(
        stored_rule.settlement_delay_ledger,
        rule.settlement_delay_ledger
    );
    assert_eq!(stored_rule.auto_settle, rule.auto_settle);

    client.execute(&dummy_executor, &op_clear_settlement_rule);
    assert!(client.get_settlement_rule(&merchant4).is_none());

    client.execute(&dummy_executor, &op_set_default_rule);
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
    client.execute(&dummy_executor, &op_upgrade);
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
    client.execute(&Address::generate(&env), &op);

    // The pending recovery is gone.
    assert!(client.try_execute_recovery().is_err());
}

// ---------------------------------------------------------------------------
// Issue #824: Recovery happy-path delay test with timestamp warp
// ---------------------------------------------------------------------------

/// Verifies the recovery happy path: `execute_recovery` fails before the
/// 7-day delay has elapsed and succeeds exactly at the boundary.
///
/// Two timestamps are probed:
/// 1. `RECOVERY_DELAY_SECONDS - 1` — one second before expiry → must panic
///    with `RecoveryDelayActive` (#9).
/// 2. `RECOVERY_DELAY_SECONDS` — exactly at the expiry boundary → must
///    succeed and install the new admin.
#[test]
fn recovery_execute_succeeds_exactly_at_delay_boundary() {
    let (env, client, _admins, _) = setup();
    let new_admin = Address::generate(&env);

    client.initiate_recovery(&new_admin);

    // One second before the delay expires: execute_recovery must be refused.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS - 1);
    assert!(
        client.try_execute_recovery().is_err(),
        "execute_recovery must fail before the delay has elapsed"
    );

    // Advance by the final second to reach the exact expiry boundary.
    env.ledger().with_mut(|ledger| ledger.timestamp += 1);
    client.execute_recovery();

    assert_eq!(
        client.get_admin(),
        soroban_sdk::vec![&env, new_admin],
        "execute_recovery at the boundary must install the new admin"
    );
    assert_eq!(client.get_threshold(), 1);

    // The pending record is consumed: a second execute_recovery must fail.
    assert!(
        client.try_execute_recovery().is_err(),
        "pending recovery must be consumed after a successful execute"
    );
}

// ---------------------------------------------------------------------------
// Issue #820: Execute-before-timelock rejection test
// ---------------------------------------------------------------------------

/// Verifies that `execute` is rejected with `ExecutionNotReady` (#10) at every
/// point before the scheduled operation's delay has fully elapsed.
///
/// Three timestamps are probed to isolate the enforcement boundary:
/// 1. `t = 0` (immediately after scheduling) — must be rejected.
/// 2. `t = DEFAULT_TIMELOCK_DELAY_SECONDS - 1` (one second before expiry) — must
///    still be rejected.
/// 3. The operation is cancelled after the boundary checks so storage is left
///    clean; this confirms the rejection did not consume the scheduled entry.
#[test]
fn execute_before_timelock_is_rejected_with_execution_not_ready() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant.clone());

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    // Immediately after scheduling: no time has passed.
    let err_immediate = client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .unwrap_err();
    assert_eq!(
        err_immediate,
        Ok(soroban_sdk::Error::from_contract_error(
            crate::SettlementError::ExecutionNotReady as u32
        )),
        "execute at t=0 must return ExecutionNotReady"
    );

    // One second before the delay expires.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS - 1);
    let err_early = client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .unwrap_err();
    assert_eq!(
        err_early,
        Ok(soroban_sdk::Error::from_contract_error(
            crate::SettlementError::ExecutionNotReady as u32
        )),
        "execute one second before expiry must return ExecutionNotReady"
    );

    // The scheduled entry must still be present: cancel succeeds.
    client.cancel(&admins, &operation);
    assert!(!client.is_merchant_registered(&merchant));
}

// ---------------------------------------------------------------------------
// Issue #822: Cancel-of-unscheduled operation test
// ---------------------------------------------------------------------------

/// Verifies that `cancel` returns `OperationNotScheduled` (#11) when the
/// operation has never been scheduled.
///
/// The cancel path mirrors execute: both look up the operation by its hash and
/// panic with `OperationNotScheduled` when nothing is found.  This test
/// isolates that branch so it has its own regression coverage independent of
/// the execute-path tests.
#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn cancel_unscheduled_operation_returns_operation_not_scheduled() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    // The operation was never scheduled — cancel must panic with #11.
    client.cancel(&admins, &operation);
}

// ---------------------------------------------------------------------------
// Execute-of-unscheduled operation test
// ---------------------------------------------------------------------------

/// Verifies that `execute` returns `OperationNotScheduled` (#11) when the
/// operation has never been scheduled.
///
/// Mirrors `cancel_unscheduled_operation_returns_operation_not_scheduled`:
/// execute looks up the operation by its hash the same way cancel does, and
/// panics with `OperationNotScheduled` when nothing is found. This pins that
/// branch on the execute path independently.
#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn execute_unscheduled_operation_returns_operation_not_scheduled() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    // The operation was never scheduled — execute must panic with #11.
    client.execute(&admins.get(0).unwrap(), &operation);
}

// ---------------------------------------------------------------------------
// Issue #974: Per-operation delay floor boundary tests
// ---------------------------------------------------------------------------

/// `schedule` with `execute_in` equal to the floor succeeds and stores a
/// `ScheduledOp`.
#[test]
fn schedule_at_floor_succeeds() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant.clone());

    client.schedule(&admins, &operation, &MIN_TIMELOCK_DELAY_SECONDS);

    // The operation is stored and not yet executable until the delay elapses.
    assert!(client
        .try_execute(&admins.get(0).unwrap(), &operation)
        .is_err());
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += MIN_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap(), &operation);
    assert!(client.is_merchant_registered(&merchant));
}

/// `schedule` with `execute_in` equal to floor minus one panics with
/// `ExecutionNotReady` (#10) and does not store the operation.
#[test]
fn schedule_below_floor_is_rejected() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant.clone());

    let result = client.try_schedule(&admins, &operation, &(MIN_TIMELOCK_DELAY_SECONDS - 1));
    assert_eq!(
        result.unwrap_err().unwrap(),
        SettlementError::ExecutionNotReady.into(),
        "schedule below the delay floor must return ExecutionNotReady"
    );
    assert!(!client.is_merchant_registered(&merchant));
}

/// Returns the `ScheduledOperation` storage key for `operation`.
fn scheduled_op_key(env: &Env, operation: &Operation) -> DataKey {
    let op_hash: BytesN<32> = env.crypto().sha256(&operation.clone().to_xdr(env)).into();
    DataKey::ScheduledOperation(op_hash)
}

/// Reads the stored `ScheduledOp` for `operation`, if any.
fn read_scheduled_op(
    env: &Env,
    client: &SettlementContractClient,
    operation: &Operation,
) -> Option<ScheduledOp> {
    let key = scheduled_op_key(env, operation);
    env.as_contract(&client.address, || env.storage().persistent().get(&key))
}

/// Returns how many `op_scheduled` events the last invocation emitted.
fn op_scheduled_event_count(env: &Env) -> u32 {
    let topic = Symbol::new(env, events::OP_SCHEDULED_EVENT);
    let mut count = 0;
    for (_contract, topics, _data) in env.events().all().iter() {
        if !topics.is_empty() && Symbol::from_val(env, &topics.get(0).unwrap()) == topic {
            count += 1;
        }
    }
    count
}

/// Issues #885, #893: a zero delay collapses the timelock entirely. `schedule`
/// must reject it with the typed `ExecutionNotReady` error, write no
/// `ScheduledOperation` entry, and emit no `op_scheduled` event.
/// Boundary test: delay 0 is rejected, while the minimum valid delay
/// (`MIN_TIMELOCK_DELAY_SECONDS`) successfully schedules and emits `op_scheduled`.
#[test]
fn schedule_with_zero_delay_is_rejected() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    let result = client.try_schedule(&admins, &operation, &0);
    assert_eq!(
        result,
        Err(Ok(SettlementError::ExecutionNotReady.into())),
        "a zero delay must return ExecutionNotReady"
    );
    assert!(
        read_scheduled_op(&env, &client, &operation).is_none(),
        "no ScheduledOperation entry may exist after a rejected zero delay"
    );
    assert_eq!(
        op_scheduled_event_count(&env),
        0,
        "no op_scheduled event may be emitted on a rejected zero delay"
    );

    let now = env.ledger().timestamp();
    client.schedule(&admins, &operation, &MIN_TIMELOCK_DELAY_SECONDS);
    assert_eq!(op_scheduled_event_count(&env), 1);
    assert_eq!(
        read_scheduled_op(&env, &client, &operation)
            .expect("the minimal valid delay must schedule")
            .execute_at,
        now + MIN_TIMELOCK_DELAY_SECONDS
    );
}

// ---------------------------------------------------------------------------
// Issue #894: schedule delay overflow
// ---------------------------------------------------------------------------

/// A realistic, non-zero ledger timestamp for the overflow tests.
const OVERFLOW_TEST_TIMESTAMP: u64 = 1_700_000_000;

/// `execute_at` is computed as `timestamp + execute_in`. A delay near
/// `u64::MAX` would wrap that sum (or trap on overflow) — the
/// `MAX_TIMELOCK_DELAY_SECONDS` bound must reject it first with the typed
/// `ExecutionDelayTooLong` error, leaving no queue entry and no event.
#[test]
fn schedule_rejects_overflowing_delay_with_typed_error() {
    let (env, client, admins, merchant) = setup();
    env.ledger()
        .with_mut(|ledger| ledger.timestamp = OVERFLOW_TEST_TIMESTAMP);
    let operation = Operation::RegisterMerchant(merchant);

    for execute_in in [
        u64::MAX,
        u64::MAX - OVERFLOW_TEST_TIMESTAMP + 1,
        MAX_TIMELOCK_DELAY_SECONDS + 1,
    ] {
        let result = client.try_schedule(&admins, &operation, &execute_in);
        assert_eq!(
            result,
            Err(Ok(SettlementError::ExecutionDelayTooLong.into())),
            "execute_in {execute_in} must be rejected by the max-delay bound, not wrap or panic"
        );
        assert!(read_scheduled_op(&env, &client, &operation).is_none());
        assert_eq!(op_scheduled_event_count(&env), 0);
    }

    // Without the bound, the two largest delays above would overflow.
    assert!(OVERFLOW_TEST_TIMESTAMP.checked_add(u64::MAX).is_none());
    assert!(OVERFLOW_TEST_TIMESTAMP
        .checked_add(u64::MAX - OVERFLOW_TEST_TIMESTAMP + 1)
        .is_none());
}

/// The largest valid delay (`MAX_TIMELOCK_DELAY_SECONDS`) schedules and
/// stores an exact `execute_at` that is strictly after the current
/// timestamp, matching the `op_scheduled` event payload.
#[test]
fn schedule_accepts_max_delay_with_exact_execute_at() {
    let (env, client, admins, merchant) = setup();
    env.ledger()
        .with_mut(|ledger| ledger.timestamp = OVERFLOW_TEST_TIMESTAMP);
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admins, &operation, &MAX_TIMELOCK_DELAY_SECONDS);

    let expected = OVERFLOW_TEST_TIMESTAMP + MAX_TIMELOCK_DELAY_SECONDS;
    let stored = read_scheduled_op(&env, &client, &operation)
        .expect("the maximum valid delay must schedule");
    assert_eq!(stored.execute_at, expected);
    assert!(
        stored.execute_at > OVERFLOW_TEST_TIMESTAMP,
        "execute_at must not wrap below the current timestamp"
    );

    let (_contract, _topics, data) = env.events().all().last().unwrap();
    let (_caller, event_execute_at): (Address, u64) = <(Address, u64)>::from_val(&env, &data);
    assert_eq!(event_execute_at, expected);
}

// ---------------------------------------------------------------------------
// Issue #976: Admin set change confirmation across read helpers and events
// ---------------------------------------------------------------------------

/// An admin set change must be reflected consistently whether it goes through
/// the direct `transfer_admin` path or the timelocked `Operation::TransferAdmin`
/// path: `get_admin`/`get_threshold` return the new values, `admin_transferred`
/// carries the old and new primary admin, and the old signer set no longer
/// satisfies an admin-gated call.
#[test]
fn admin_set_change_is_confirmed_across_read_helpers_and_events() {
    let (env, client, admins, _merchant) = setup();

    // --- Direct path ---
    let new_admin = Address::generate(&env);
    let new_admins = soroban_sdk::vec![&env, new_admin.clone()];
    client.transfer_admin(&admins, &new_admins, &1);

    assert_eq!(client.get_admin(), new_admins);
    assert_eq!(client.get_threshold(), 1);

    let (_, _, data) = env.events().all().last().unwrap();
    let payload = AdminTransferred::try_from_val(&env, &data).unwrap();
    assert_eq!(payload.old_admins, admins);
    assert_eq!(payload.new_admins, new_admins);

    assert!(
        client.try_pause(&admins).is_err(),
        "the old signer set must no longer satisfy an admin-gated call"
    );

    // --- Timelocked path ---
    let next_admin = Address::generate(&env);
    let next_admins = soroban_sdk::vec![&env, next_admin.clone()];
    let operation = Operation::TransferAdmin(next_admins.clone(), 1);
    client.schedule(&new_admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&new_admin, &operation);

    assert_eq!(client.get_admin(), next_admins);
    assert_eq!(client.get_threshold(), 1);

    let (_, _, data) = env.events().all().last().unwrap();
    let payload = AdminTransferred::try_from_val(&env, &data).unwrap();
    assert_eq!(payload.old_admins, new_admins);
    assert_eq!(payload.new_admins, next_admins);

    assert!(
        client.try_pause(&new_admins).is_err(),
        "the superseded signer set must no longer satisfy an admin-gated call"
    );
}

// ---------------------------------------------------------------------------
// Issue #895: cancel after execute
// ---------------------------------------------------------------------------

/// `execute` consumes the `ScheduledOperation` slot, so a later `cancel` of the
/// same operation by valid admin signers must fail with the typed
/// `OperationNotScheduled` error — not resurrect the slot, not panic untyped,
/// and not roll back the effect the execution already applied.
#[test]
fn cancel_after_execute_returns_operation_not_scheduled() {
    use crate::types::DataKey;
    use soroban_sdk::xdr::ToXdr;
    use soroban_sdk::BytesN;

    let (env, client, admins, _) = setup();
    let new_governance = super::register_governance(&env);
    let operation = Operation::UpdateGovernance(new_governance.clone());

    let op_hash: BytesN<32> = env.crypto().sha256(&operation.clone().to_xdr(&env)).into();
    let scheduled_key = DataKey::ScheduledOperation(op_hash);
    let slot_present = || {
        env.as_contract(&client.address, || {
            env.storage().persistent().has(&scheduled_key)
        })
    };

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(slot_present(), "schedule must write the scheduled slot");

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS + 1);
    client.execute(&admins.get(0).unwrap(), &operation);

    assert_eq!(client.get_governance(), new_governance);
    assert!(!slot_present(), "execute must consume the scheduled slot");

    // `try_cancel` surfaces the contract error as a value: a typed
    // `Err(Ok(..))` proves the failure is a `SettlementError` raised via
    // `panic_with_error!`, not an untyped panic crossing the boundary.
    let result = client.try_cancel(&admins, &operation);
    assert_eq!(
        result.err(),
        Some(Ok(soroban_sdk::Error::from(
            SettlementError::OperationNotScheduled
        )))
    );

    // The executed effect is not rolled back and the slot stays absent.
    assert_eq!(client.get_governance(), new_governance);
    assert!(
        !slot_present(),
        "a failed cancel must not resurrect the scheduled slot"
    );
}

// ---------------------------------------------------------------------------
// Issue #888: execute consumes the ScheduledOperation on every path
// ---------------------------------------------------------------------------

/// The scheduled slot is removed before the operation is applied, so a
/// successful execution is a one-shot transition. A retry fails with the
/// typed `OperationNotScheduled` error, and retrying that failure again leaves
/// both the slot and the error unchanged — the operation can never be replayed.
#[test]
fn execute_consumes_slot_and_failed_retry_stays_operation_not_scheduled() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant.clone());
    let executor = admins.get(0).unwrap();

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(
        read_scheduled_op(&env, &client, &operation).is_some(),
        "schedule must queue the operation"
    );

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&executor, &operation);

    assert!(client.is_merchant_registered(&merchant));
    assert!(
        read_scheduled_op(&env, &client, &operation).is_none(),
        "execute must consume the scheduled slot before applying the operation"
    );

    let failed_retry = client.try_execute(&executor, &operation);
    assert_eq!(
        failed_retry.err(),
        Some(Ok(SettlementError::OperationNotScheduled.into())),
        "a replay after execution must fail with OperationNotScheduled"
    );

    // A retry of the failed execute is not a re-schedule: the queue stays
    // empty and the same typed error is returned.
    assert!(read_scheduled_op(&env, &client, &operation).is_none());
    let retry_after_failure = client.try_execute(&executor, &operation);
    assert_eq!(
        retry_after_failure.err(),
        Some(Ok(SettlementError::OperationNotScheduled.into())),
        "a retry after a failed execute must stay OperationNotScheduled"
    );
}

/// The recovery veto is checked before the slot is consumed, so a vetoed
/// execution leaves the operation queued. Once the recovery is cancelled the
/// same operation executes exactly once, after which retries are
/// `OperationNotScheduled`.
#[test]
fn recovery_veto_preserves_scheduled_slot_until_recovery_resolves() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = super::register_governance(&env);
    let operation = Operation::UpdateGovernance(new_governance.clone());
    let executor = admins.get(0).unwrap();

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    client.initiate_recovery(&Address::generate(&env));
    let vetoed = client.try_execute(&executor, &operation);
    assert_eq!(
        vetoed.err(),
        Some(Ok(SettlementError::RecoveryDelayActive.into())),
        "a pending recovery must veto the execution"
    );
    assert!(
        read_scheduled_op(&env, &client, &operation).is_some(),
        "the RecoveryDelayActive veto must leave the scheduled slot intact"
    );

    client.cancel_recovery(&admins);
    client.execute(&executor, &operation);
    assert_eq!(client.get_governance(), new_governance);
    assert!(
        read_scheduled_op(&env, &client, &operation).is_none(),
        "execution must consume the slot once the veto is lifted"
    );

    let retry = client.try_execute(&executor, &operation);
    assert_eq!(
        retry.err(),
        Some(Ok(SettlementError::OperationNotScheduled.into()))
    );
}

// ---------------------------------------------------------------------------
// Issue #887: scheduled-operation queue-depth metric
// ---------------------------------------------------------------------------

/// The queue-depth query starts at zero on a fresh contract and tracks
/// schedule, execute, and cancel. A failed cancel of a missing operation
/// leaves the count untouched and a failed replay after execution cannot
/// underflow it.
#[test]
fn pending_operation_count_tracks_schedule_execute_and_cancel() {
    let (env, client, admins, _merchant) = setup();

    assert_eq!(
        client.get_pending_operation_count(),
        0,
        "a fresh contract must report an empty timelock queue"
    );

    let first_merchant = Address::generate(&env);
    let second_merchant = Address::generate(&env);
    let first = Operation::RegisterMerchant(first_merchant.clone());
    let second = Operation::RegisterMerchant(second_merchant.clone());

    client.schedule(&admins, &first, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.get_pending_operation_count(), 1);
    client.schedule(&admins, &second, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.get_pending_operation_count(), 2);

    // Cancelling an operation that is not queued fails before any removal, so
    // the counter cannot underflow.
    let missing = Operation::RegisterMerchant(Address::generate(&env));
    let missing_cancel = client.try_cancel(&admins, &missing);
    assert_eq!(
        missing_cancel.err(),
        Some(Ok(SettlementError::OperationNotScheduled.into()))
    );
    assert_eq!(
        client.get_pending_operation_count(),
        2,
        "a failed cancel of a missing operation must not change the count"
    );

    client.cancel(&admins, &first);
    assert_eq!(client.get_pending_operation_count(), 1);
    assert!(read_scheduled_op(&env, &client, &first).is_none());

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap(), &second);
    assert_eq!(client.get_pending_operation_count(), 0);
    assert!(client.is_merchant_registered(&second_merchant));

    // A replay fails with OperationNotScheduled; the counter must stay at 0.
    let replay = client.try_execute(&admins.get(0).unwrap(), &second);
    assert_eq!(
        replay.err(),
        Some(Ok(SettlementError::OperationNotScheduled.into()))
    );
    assert_eq!(
        client.get_pending_operation_count(),
        0,
        "a failed replay must not decrement the counter below zero"
    );
}

/// `cancel_all` removes each actually-cancelled queue entry and decrements the
/// metric once per removal, skipping missing entries without touching it.
#[test]
fn pending_operation_count_tracks_cancel_all() {
    let (env, client, admins, _merchant) = setup();

    let first = Operation::RegisterMerchant(Address::generate(&env));
    let second = Operation::RegisterMerchant(Address::generate(&env));
    client.schedule(&admins, &first, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.schedule(&admins, &second, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.get_pending_operation_count(), 2);

    let missing = Operation::RegisterMerchant(Address::generate(&env));
    let batch = soroban_sdk::vec![&env, first.clone(), missing, first.clone()];
    assert_eq!(client.cancel_all(&admins, &batch), 1);
    assert_eq!(
        client.get_pending_operation_count(),
        1,
        "cancel_all must decrement only for operations it actually removed"
    );

    client.cancel(&admins, &second);
    assert_eq!(client.get_pending_operation_count(), 0);
}
