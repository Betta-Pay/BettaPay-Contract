//! Regression coverage for issue #570: the `ScheduledOperation` storage key
//! is `sha256(operation.to_xdr())`. A real SHA-256 collision can't be
//! manufactured in a test, so these tests simulate one by writing a
//! `ScheduledOp` directly into the exact storage slot a *different*
//! operation would use, then confirm `schedule`/`execute`/`cancel` detect
//! the mismatch instead of silently treating the hash match as proof the
//! stored data belongs to the operation in hand.

use crate::types::{DataKey, ScheduledOp};
use crate::{Operation, DEFAULT_TIMELOCK_DELAY_SECONDS};
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
    let (env, client, _admins, merchant) = setup();
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
    client.execute(&operation);
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
