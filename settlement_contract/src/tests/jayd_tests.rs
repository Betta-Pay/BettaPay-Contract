//! Tests for issues #1116, #1119, and #1120:
//! - store_payment_reference duplicate check and storage rollback on AmountOverflow (#1116)
//! - dry_run mirroring execute validation without mutating state or emitting events (#1119)
//! - pause reason bounds (MAX_PAUSE_REASON and MAX_PAUSE_REASON + 1) and event payload (#1120)

use super::setup;
use crate::errors::SettlementError;
use crate::types::{DataKey, Operation};
use bettapay_common::constants::{
    DEFAULT_TIMELOCK_DELAY_SECONDS, MAX_PAUSE_REASON,
};
use bettapay_common::events::PAUSED_EVENT;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, FromVal, Symbol};

#[test]
fn store_payment_reference_overflow_rolls_back_storage() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let reference = BytesN::from_array(&env, &[42; 32]);
    let res = client.try_store_payment_reference(&merchant, &reference, &i128::MAX);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::AmountOverflow.into()
    );

    // Verify Payment record and PaymentCount remain absent
    env.as_contract(&client.address, || {
        assert!(!env
            .storage()
            .persistent()
            .has(&DataKey::Payment(merchant.clone(), reference)));
        assert_eq!(crate::storage::read_payment_count(&env, &merchant), 0);
    });
}

#[test]
fn dry_run_mirrors_execute_validation_and_leaves_state_untouched() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let new_admin = Address::generate(&env);
    let operation = Operation::TransferAdmin(soroban_sdk::vec![&env, new_admin.clone()], 1);

    // 1. Error parity: OperationNotScheduled
    assert_eq!(
        client.try_dry_run(&operation).unwrap_err().unwrap(),
        SettlementError::OperationNotScheduled.into()
    );
    assert_eq!(
        client.try_execute(&admin, &operation).unwrap_err().unwrap(),
        SettlementError::OperationNotScheduled.into()
    );

    // Schedule operation
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    // 2. Error parity: ExecutionNotReady (timelock delay has not elapsed)
    assert_eq!(
        client.try_dry_run(&operation).unwrap_err().unwrap(),
        SettlementError::ExecutionNotReady.into()
    );
    assert_eq!(
        client.try_execute(&admin, &operation).unwrap_err().unwrap(),
        SettlementError::ExecutionNotReady.into()
    );

    // Advance ledger so timelock passes, but initiate recovery
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS;
    });
    let recovery_address = Address::generate(&env);
    client.initiate_recovery(&recovery_address);

    // 3. Error parity: RecoveryDelayActive
    assert_eq!(
        client.try_dry_run(&operation).unwrap_err().unwrap(),
        SettlementError::RecoveryDelayActive.into()
    );
    assert_eq!(
        client.try_execute(&admin, &operation).unwrap_err().unwrap(),
        SettlementError::RecoveryDelayActive.into()
    );

    // Cancel recovery to clear veto
    client.cancel_recovery(&admins);

    // 4. Error parity: Paused
    client.pause(&admins, &0);
    assert_eq!(
        client.try_dry_run(&operation).unwrap_err().unwrap(),
        SettlementError::Paused.into()
    );
    assert_eq!(
        client.try_execute(&admin, &operation).unwrap_err().unwrap(),
        SettlementError::Paused.into()
    );
    client.unpause(&admins);

    // 5. Success case: dry_run succeeds without mutating state or emitting events
    let events_before = env.events().all().len();

    let dry_run_res = client.try_dry_run(&operation);
    assert!(dry_run_res.is_ok(), "dry_run must succeed when validation passes");

    // Assert zero events emitted by dry_run
    assert_eq!(env.events().all().len(), events_before);

    // Assert ScheduledOperation remains in storage and operation can still be executed
    assert_eq!(client.get_pending_operation_count(), 1);

    // Execute actually succeeds and consumes the slot
    client.execute(&admin, &operation);
    assert_eq!(client.get_pending_operation_count(), 0);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
}

#[test]
fn pause_reason_bounds_and_event_payload() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();

    // 1. reason > MAX_PAUSE_REASON fails with InvalidPauseReason
    let invalid_reason = MAX_PAUSE_REASON + 1;
    let res = client.try_pause(&admins, &invalid_reason);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::InvalidPauseReason.into()
    );

    // 2. reason == MAX_PAUSE_REASON succeeds
    client.pause(&admins, &MAX_PAUSE_REASON);
    assert!(client.is_paused());

    // 3. Verify emitted event payload contains expected reason
    let (_, topics, data) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, PAUSED_EVENT)
    );
    let (emitted_admin, paused_flag, emitted_reason): (Address, bool, u32) =
        FromVal::from_val(&env, &data);
    assert_eq!(emitted_admin, admin);
    assert_eq!(paused_flag, true);
    assert_eq!(emitted_reason, MAX_PAUSE_REASON);
}
