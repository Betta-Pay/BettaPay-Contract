//! Tests for issues #1117, #1118, and #1121:
//! - get_payment_count authorization, orphaning, and prune decrements (#1117)
//! - cancel_all batch limit, partial matches, and queue counter sync (#1118)
//! - schedule ExecutionDelayTooLong and PayloadTooLarge bounds (#1121)

use super::setup;
use crate::errors::SettlementError;
use crate::policy::{
    DEFAULT_TIMELOCK_DELAY_SECONDS, MAX_CANCEL_ALL_BATCH, MAX_SCHEDULED_OPERATION_PAYLOAD_LEN,
    MAX_TIMELOCK_DELAY_SECONDS,
};
use crate::types::Operation;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Vec};

#[test]
fn get_payment_count_authorization_orphaning_and_prune_decrements() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // Initial count is 0
    let merchant_signers = soroban_sdk::vec![&env, merchant.clone()];
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        0
    );
    assert_eq!(client.get_payment_count(&merchant, &admins), 0);

    // Unauthorized signer fails
    let unauthorized_signer = Address::generate(&env);
    let unauthorized_signers = soroban_sdk::vec![&env, unauthorized_signer];
    assert!(client
        .try_get_payment_count(&merchant, &unauthorized_signers)
        .is_err());

    // Store references: count increments
    let ref1 = BytesN::from_array(&env, &[1; 32]);
    let ref2 = BytesN::from_array(&env, &[2; 32]);
    let ref3 = BytesN::from_array(&env, &[3; 32]);

    client.store_payment_reference(&merchant, &ref1, &1_000);
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        1
    );

    client.store_payment_reference(&merchant, &ref2, &2_000);
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        2
    );

    client.store_payment_reference(&merchant, &ref3, &3_000);
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        3
    );

    // Single prune: count decrements
    client.prune_payment(&admins, &merchant, &ref1);
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        2
    );

    // Batch prune: count decrements by batch size
    let refs_batch = soroban_sdk::vec![&env, ref2, ref3];
    client.prune_payments(&admins, &merchant, &refs_batch);
    assert_eq!(
        client.get_payment_count(&merchant, &merchant_signers),
        0
    );

    // Unregister merchant: get_payment_count raises PaymentOrphaned
    client.unregister_merchant(&admins, &merchant);
    let res = client.try_get_payment_count(&merchant, &merchant_signers);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::PaymentOrphaned.into()
    );
}

#[test]
fn cancel_all_batch_limit_partial_matches_and_queue_sync() {
    let (env, client, admins, _merchant) = setup();

    // 1. Batch size exceeding MAX_CANCEL_ALL_BATCH fails with BatchTooLarge
    let mut large_batch: Vec<Operation> = Vec::new(&env);
    for _ in 0..=(MAX_CANCEL_ALL_BATCH) {
        let op = Operation::TransferAdmin(
            soroban_sdk::vec![&env, Address::generate(&env)],
            1,
        );
        large_batch.push_back(op);
    }
    let res_large = client.try_cancel_all(&admins, &large_batch);
    assert_eq!(
        res_large.unwrap_err().unwrap(),
        SettlementError::BatchTooLarge.into()
    );

    // 2. Paused contract rejects cancel_all with Paused
    client.pause(&admins, &0);
    let op1 = Operation::TransferAdmin(
        soroban_sdk::vec![&env, Address::generate(&env)],
        1,
    );
    let mut batch: Vec<Operation> = Vec::new(&env);
    batch.push_back(op1.clone());
    let res_paused = client.try_cancel_all(&admins, &batch);
    assert_eq!(
        res_paused.unwrap_err().unwrap(),
        SettlementError::Paused.into()
    );
    client.unpause(&admins);

    // 3. Schedule 2 operations
    let op2 = Operation::TransferAdmin(
        soroban_sdk::vec![&env, Address::generate(&env)],
        1,
    );
    client.schedule(&admins, &op1, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.schedule(&admins, &op2, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.get_pending_operation_count(), 2);

    // Mixed batch: op1 (present), op1 (duplicate in same batch), op3 (missing)
    let op3 = Operation::TransferAdmin(
        soroban_sdk::vec![&env, Address::generate(&env)],
        1,
    );
    let mut mixed_batch: Vec<Operation> = Vec::new(&env);
    mixed_batch.push_back(op1.clone());
    mixed_batch.push_back(op1); // duplicate
    mixed_batch.push_back(op3); // missing

    let cancelled = client.cancel_all(&admins, &mixed_batch);
    assert_eq!(cancelled, 1);
    assert_eq!(client.get_pending_operation_count(), 1);

    // Cancel remaining op2
    let mut final_batch: Vec<Operation> = Vec::new(&env);
    final_batch.push_back(op2);
    let cancelled2 = client.cancel_all(&admins, &final_batch);
    assert_eq!(cancelled2, 1);
    assert_eq!(client.get_pending_operation_count(), 0);
}

#[test]
fn schedule_delay_and_payload_bounds() {
    let (env, client, admins, _merchant) = setup();

    let op = Operation::TransferAdmin(
        soroban_sdk::vec![&env, Address::generate(&env)],
        1,
    );

    // 1. MAX_TIMELOCK_DELAY_SECONDS succeeds
    client.schedule(&admins, &op, &MAX_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.get_pending_operation_count(), 1);
    client.cancel(&admins, &op);
    assert_eq!(client.get_pending_operation_count(), 0);

    // 2. MAX_TIMELOCK_DELAY_SECONDS + 1 fails with ExecutionDelayTooLong
    let res_delay = client.try_schedule(&admins, &op, &(MAX_TIMELOCK_DELAY_SECONDS + 1));
    assert_eq!(
        res_delay.unwrap_err().unwrap(),
        SettlementError::ExecutionDelayTooLong.into()
    );

    // 3. Oversized payload fails with PayloadTooLarge
    // Construct an Operation::TransferAdmin with enough admins so encoded XDR > MAX_SCHEDULED_OPERATION_PAYLOAD_LEN
    let mut huge_admins: Vec<Address> = Vec::new(&env);
    // Each Address in XDR is > 36 bytes. 150 addresses > 5400 bytes > 4096.
    for _ in 0..150 {
        huge_admins.push_back(Address::generate(&env));
    }
    let oversized_op = Operation::TransferAdmin(huge_admins, 1);

    let res_payload = client.try_schedule(&admins, &oversized_op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(
        res_payload.unwrap_err().unwrap(),
        SettlementError::PayloadTooLarge.into()
    );
    assert_eq!(client.get_pending_operation_count(), 0);
}
