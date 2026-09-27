//! TTL-bump behavior tests for ADR 003's per-read instance TTL policy
//! (`adr/003-ttl-value-selection.md`).
//!
//! `read_admins`, `read_governance`, and `read_recovery_address` extend the
//! shared instance-storage entry's TTL on every read using the
//! `READ_INSTANCE_TTL_THRESHOLD` / `READ_INSTANCE_TTL_BUMP` policy.
//! `read_threshold` and `read_pending_recovery` must do the same, so the
//! entry's lifetime does not depend on which particular instance read
//! happens to occur.

use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, BytesN, Env, Error};

use bettapay_common::events::PendingRecovery;
use bettapay_common::storage::CommonDataKey;

use crate::storage::{read_pending_recovery, read_threshold};
use crate::types::DataKey;
use crate::{
    SettlementError, MERCHANT_TTL_BUMP, READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD,
};

use super::setup;

/// Establishes a known instance-TTL baseline of exactly `READ_INSTANCE_TTL_BUMP`
/// ledgers from the current sequence, then advances the ledger far enough that
/// the remaining TTL drops below `READ_INSTANCE_TTL_THRESHOLD` — without letting
/// the entry actually expire, which would make any further `extend_ttl` call
/// error out instead of bumping.
fn make_instance_ttl_stale(env: &Env, client_address: &Address) {
    env.as_contract(client_address, || {
        env.storage()
            .instance()
            .extend_ttl(READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_BUMP);
    });

    let seq = env.ledger().sequence();
    // Advance most of the way to the bumped live-until ledger: remaining TTL
    // drops to 10k (< 50k threshold, so a read must re-bump it) while the
    // entry is still 10k ledgers away from actually expiring.
    env.ledger()
        .set_sequence_number(seq + (READ_INSTANCE_TTL_BUMP - READ_INSTANCE_TTL_THRESHOLD / 5));
}

#[test]
fn read_threshold_bumps_instance_ttl() {
    let (env, client, _admins, _merchant) = setup();

    make_instance_ttl_stale(&env, &client.address);

    env.as_contract(&client.address, || {
        let ttl_before = env.storage().instance().get_ttl();
        assert!(
            ttl_before < READ_INSTANCE_TTL_THRESHOLD,
            "test setup did not let the instance TTL decay below threshold: {ttl_before}"
        );

        read_threshold(&env);

        let ttl_after = env.storage().instance().get_ttl();
        assert_eq!(
            ttl_after, READ_INSTANCE_TTL_BUMP,
            "read_threshold did not bump the instance TTL to the read-bump floor"
        );
    });
}

#[test]
fn read_pending_recovery_bumps_instance_ttl() {
    let (env, client, _admins, _merchant) = setup();

    let new_admin = Address::generate(&env);
    let pending = PendingRecovery {
        new_admin,
        execute_after: env.ledger().timestamp(),
        initiated_by: Address::generate(&env),
    };
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
    });

    make_instance_ttl_stale(&env, &client.address);

    env.as_contract(&client.address, || {
        let ttl_before = env.storage().instance().get_ttl();
        assert!(
            ttl_before < READ_INSTANCE_TTL_THRESHOLD,
            "test setup did not let the instance TTL decay below threshold: {ttl_before}"
        );

        let read_back = read_pending_recovery(&env);
        assert_eq!(read_back.new_admin, pending.new_admin);

        let ttl_after = env.storage().instance().get_ttl();
        assert_eq!(
            ttl_after, READ_INSTANCE_TTL_BUMP,
            "read_pending_recovery did not bump the instance TTL to the read-bump floor"
        );
    });
}

#[test]
fn tombstone_survives_payment_read_attempts() {
    let (env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);
    let reference = BytesN::from_array(&env, &[1; 32]);
    client.store_payment_reference(&merchant, &reference, &1_000);

    // Unregistering the merchant writes the ArchivedMerchant tombstone and bumps its TTL
    client.unregister_merchant(&admins, &merchant);

    let tombstone_key = DataKey::ArchivedMerchant(merchant.clone());

    // Assert tombstone TTL was bumped on unregister
    env.as_contract(&client.address, || {
        assert!(env.storage().persistent().has(&tombstone_key));
        let ttl = env.storage().persistent().get_ttl(&tombstone_key);
        assert_eq!(
            ttl, MERCHANT_TTL_BUMP,
            "tombstone TTL must be bumped to MERCHANT_TTL_BUMP on unregister"
        );
    });

    // Advance sequence number slightly
    env.ledger().with_mut(|l| l.sequence_number += 100);

    // Single-record payment read attempt must fail with PaymentOrphaned
    let single_read =
        client.try_get_payment_reference(&merchant, &reference, &soroban_sdk::Vec::new(&env));
    assert!(
        matches!(
            single_read,
            Err(Ok(e)) if e == Error::from_contract_error(SettlementError::PaymentOrphaned as u32)
        ),
        "payment read for orphaned merchant must fail with PaymentOrphaned"
    );

    // Batch payment read attempt must also fail with PaymentOrphaned
    let batch_read = client.try_get_payments(
        &merchant,
        &soroban_sdk::vec![&env, reference.clone()],
        &soroban_sdk::vec![&env],
    );
    assert!(
        matches!(
            batch_read,
            Err(Ok(e)) if e == Error::from_contract_error(SettlementError::PaymentOrphaned as u32)
        ),
        "batch payment read for orphaned merchant must fail with PaymentOrphaned"
    );

    // Assert tombstone persists and remains present in storage across payment-read attempts
    env.as_contract(&client.address, || {
        assert!(
            env.storage().persistent().has(&tombstone_key),
            "tombstone must persist in storage across payment-read attempts"
        );
        let remaining_ttl = env.storage().persistent().get_ttl(&tombstone_key);
        assert!(
            remaining_ttl > 0,
            "tombstone TTL must remain valid across payment-read attempts"
        );
    });
}

/// Issue #759: a failed-auth call to `store_payment_reference` must not warm
/// the merchant marker. Before the fix, `is_merchant_registered_and_bump_ttl`
/// was called ahead of `merchant.require_auth()`, so the TTL extended even
/// when the caller had no authorization for the merchant address.
///
/// This test registers a merchant, records the marker's initial TTL, then
/// attempts a `store_payment_reference` call with no authorizations granted.
/// After the (expected) auth failure the marker TTL must be unchanged.
#[test]
fn store_payment_reference_failed_auth_does_not_bump_merchant_ttl() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let marker_key = DataKey::Merchant(merchant.clone());

    let ttl_before = env.as_contract(&client.address, || {
        env.storage().persistent().get_ttl(&marker_key)
    });

    env.ledger().with_mut(|l| l.sequence_number += 100);

    let reference = BytesN::<32>::from_array(&env, &[7u8; 32]);

    env.mock_auths(&[]);
    let result = client.try_store_payment_reference(&merchant, &reference, &1_000);
    assert!(
        result.is_err(),
        "expected store_payment_reference to fail without merchant auth"
    );

    let ttl_after = env.as_contract(&client.address, || {
        env.storage().persistent().get_ttl(&marker_key)
    });

    assert!(
        ttl_after <= ttl_before,
        "merchant marker TTL must not increase on a failed-auth store attempt (before={ttl_before}, after={ttl_after})"
    );
}
