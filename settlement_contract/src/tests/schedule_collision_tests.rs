//! Regression coverage for issue #570: the `ScheduledOperation` storage key
//! is `sha256(operation.to_xdr())`. A real SHA-256 collision can't be
//! manufactured in a test, so these tests simulate one by writing a
//! `ScheduledOp` directly into the exact storage slot a *different*
//! operation would use, then confirm `schedule`/`execute`/`cancel` detect
//! the mismatch instead of silently treating the hash match as proof the
//! stored data belongs to the operation in hand.

use crate::types::{DataKey, ScheduledOp};
use crate::{Operation, DEFAULT_TIMELOCK_DELAY_SECONDS};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{Address, BytesN, Env};

use super::setup;

/// Writes a `ScheduledOp` directly under the storage slot that `operation`
/// would hash to, with attacker/unrelated-looking `operation_xdr` bytes.
/// This stands in for a genuine hash collision: same key, different
/// underlying operation content.
fn plant_colliding_slot(
    env: &Env,
    contract_id: &Address,
    operation: &Operation,
    fake_operation_xdr: soroban_sdk::Bytes,
    execute_at: u64,
) -> BytesN<32> {
    let op_hash: BytesN<32> = env.crypto().sha256(&operation.clone().to_xdr(env)).into();
    env.as_contract(contract_id, || {
        env.storage().persistent().set(
            &DataKey::ScheduledOperation(op_hash.clone()),
            &ScheduledOp {
                operation_xdr: fake_operation_xdr,
                execute_at,
            },
        );
    });
    op_hash
}

/// `schedule()` must not treat a hash match as "this exact operation is
/// already scheduled" when the stored bytes belong to something else — that
/// would either silently block the legitimate schedule forever under the
/// wrong error, or (pre-fix) blindly overwrite the other pending operation.
/// It must instead raise the distinct `OperationHashCollision` error (#316).
#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn schedule_detects_collision_with_unrelated_pending_operation() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    let unrelated_bytes = soroban_sdk::Bytes::from_slice(&env, b"not this operation's xdr");
    plant_colliding_slot(
        &env,
        &client.address,
        &operation,
        unrelated_bytes,
        env.ledger().timestamp() + DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
}

/// `execute()` must not run an operation that only *hashes* into an
/// occupied slot — the stored bytes must match the operation supplied.
/// Otherwise a hash collision would let an operation that was never
/// scheduled (and never passed the timelock/threshold checks for its own
/// content) ride an unrelated pending operation's slot the moment that
/// slot's `execute_at` has passed.
#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn execute_rejects_operation_that_only_collides_on_hash() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    let unrelated_bytes = soroban_sdk::Bytes::from_slice(&env, b"not this operation's xdr");
    // Plant it already ripe for execution.
    plant_colliding_slot(
        &env,
        &client.address,
        &operation,
        unrelated_bytes,
        env.ledger().timestamp(),
    );

    // Would have executed and registered the merchant under the pre-fix
    // behaviour, since only the hash was checked.
    client.execute(&admins.get(0).unwrap().clone(), &operation);
    client.execute(&admins.get(0).unwrap(), &operation);
}

/// `cancel()` must apply the same bytes-match check as `execute()`.
#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn cancel_rejects_operation_that_only_collides_on_hash() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    let unrelated_bytes = soroban_sdk::Bytes::from_slice(&env, b"not this operation's xdr");
    plant_colliding_slot(
        &env,
        &client.address,
        &operation,
        unrelated_bytes,
        env.ledger().timestamp() + DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    client.cancel(&admins, &operation);
}

/// Sanity check that a genuine re-schedule of the *same* operation (the
/// common case, no collision involved) still raises the original
/// `OperationAlreadyScheduled` (#12), not the new collision error — the two
/// codes must stay distinguishable.
#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn rescheduling_the_same_operation_is_still_a_plain_duplicate() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
}

// ---------------------------------------------------------------------------
// Issue #819: schedule hash-collision branch test
// ---------------------------------------------------------------------------

/// Isolates the collision branch inside `schedule` in its own test.
///
/// When a storage slot already holds a `ScheduledOp` whose `operation_xdr`
/// does *not* match the operation being scheduled (simulating an actual
/// SHA-256 hash collision), `schedule` must raise `OperationHashCollision`
/// (#316) rather than treating it as a plain duplicate or silently
/// overwriting the existing entry.
#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn schedule_collision_branch_raises_operation_hash_collision() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    // Inject a slot whose XDR belongs to a completely different operation,
    // but occupies the same storage key as `operation` (simulated collision).
    let colliding_bytes =
        soroban_sdk::Bytes::from_slice(&env, b"collision: unrelated operation xdr");
    plant_colliding_slot(
        &env,
        &client.address,
        &operation,
        colliding_bytes,
        env.ledger().timestamp() + DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    // `schedule` must detect the mismatch and panic with #316, not #12.
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
}

// ---------------------------------------------------------------------------
// Issue #818: schedule duplicate-operation rejection test
// ---------------------------------------------------------------------------

/// Isolates the duplicate-schedule guard in its own test.
///
/// When the exact same operation is already pending in the queue (both the
/// hash and the stored XDR match), `schedule` must raise
/// `OperationAlreadyScheduled` (#12) and must not silently overwrite the
/// existing entry or return a different error code.
#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn schedule_duplicate_operation_raises_operation_already_scheduled() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    // First schedule succeeds.
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    // Scheduling the identical operation a second time must panic with #12.
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
}

// ---------------------------------------------------------------------------
// Issue #996: duplicate vs hash-collision canonical hash test
// ---------------------------------------------------------------------------

/// Locks in the distinction between a true duplicate and a hash collision,
/// asserting the exact error variant for each branch in one place.
///
/// * A true duplicate (same hash, byte-identical stored XDR) must return
///   `OperationAlreadyScheduled`.
/// * A different payload occupying the same hash slot must return
///   `OperationHashCollision`.
///
/// The distinction rests solely on the byte-for-byte comparison of the stored
/// `ScheduledOp::operation_xdr`. A refactor that collapses the branches, or
/// drops the comparison, makes one of these assertions fail. Neither failed
/// schedule may disturb what is already stored.
#[test]
fn duplicate_and_hash_collision_are_distinct_errors() {
    use crate::errors::SettlementError;

    let (env, client, admins, merchant) = setup();

    // Branch 1: true duplicate. Schedule once, then re-schedule the identical
    // operation.
    let duplicate_op = Operation::RegisterMerchant(merchant);
    client.schedule(&admins, &duplicate_op, &DEFAULT_TIMELOCK_DELAY_SECONDS);

    let duplicate = client.try_schedule(&admins, &duplicate_op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(
        matches!(
            duplicate,
            Err(Ok(e)) if e == soroban_sdk::Error::from_contract_error(SettlementError::OperationAlreadyScheduled as u32)
        ),
        "identical re-schedule must return OperationAlreadyScheduled",
    );

    // Branch 2: collision. Plant a different payload in the slot another
    // operation hashes to, then schedule that operation.
    let colliding_op = Operation::UnregisterMerchant(Address::generate(&env));
    let foreign_xdr = soroban_sdk::Bytes::from_slice(&env, b"different payload, same hash slot");
    let colliding_hash = plant_colliding_slot(
        &env,
        &client.address,
        &colliding_op,
        foreign_xdr.clone(),
        env.ledger().timestamp() + DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    let collision = client.try_schedule(&admins, &colliding_op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(
        matches!(
            collision,
            Err(Ok(e)) if e == soroban_sdk::Error::from_contract_error(SettlementError::OperationHashCollision as u32)
        ),
        "same-hash different-payload schedule must return OperationHashCollision",
    );

    // The two outcomes must be different errors.
    assert_ne!(
        SettlementError::OperationAlreadyScheduled as u32,
        SettlementError::OperationHashCollision as u32,
    );

    // The foreign entry survives the rejected schedule untouched.
    let stored: ScheduledOp = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get(&DataKey::ScheduledOperation(colliding_hash))
            .expect("colliding slot must still be occupied")
    });
    assert_eq!(stored.operation_xdr, foreign_xdr);
}
