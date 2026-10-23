//! TTL behavior tests for ADR 003's reader policy
//! (`adr/003-ttl-value-selection.md`), including the entry-point classification
//! registry that requires every public API method to declare its TTL behavior.
//!
//! `read_admins`, `read_governance`, and `read_recovery_address` extend the
//! shared instance-storage entry's TTL on every read using the
//! `READ_INSTANCE_TTL_THRESHOLD` / `READ_INSTANCE_TTL_BUMP` policy.
//! `read_threshold` and `read_pending_recovery` must do the same, so the
//! entry's lifetime does not depend on which particular instance read
//! happens to occur. Public read APIs not classified as keep-alive readers
//! explicitly declare why they are TTL-neutral.

use alloc::collections::BTreeSet;
use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _};
use soroban_sdk::{Address, BytesN, Env, Error, FromVal, Symbol, TryFromVal};

use bettapay_common::events::{self, PendingRecovery};
use bettapay_common::storage::CommonDataKey;

use crate::storage::{read_pending_recovery, read_threshold};
use crate::types::{DataKey, SettlementRule};
use crate::{
    SettlementError, MERCHANT_TTL_BUMP, MERCHANT_TTL_THRESHOLD, PAYMENT_TTL_BUMP,
    PAYMENT_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD, RULE_TTL_BUMP,
    RULE_TTL_THRESHOLD,
};

use super::setup;

fn make_rule(platform_fee_bps: u32, network_fee_bps: u32, delay: u32) -> SettlementRule {
    SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger: delay,
        auto_settle: true,
    }
}

/// Compares two SettlementRule values directly.
fn rules_equal(a: &SettlementRule, b: &SettlementRule) -> bool {
    a == b
}

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
        &0,
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

// ─── TTL-neutral rule readers (issues #760–#763) ────────────────────────────

/// Sets a merchant-specific rule, then decays the persistent `Rule` entry's
/// TTL below the bump threshold without letting it expire, returning the TTL
/// observed right before the call under test.
fn make_rule_ttl_stale(env: &Env, contract_address: &Address, merchant: &Address) -> u32 {
    let rule_key = DataKey::Rule(merchant.clone());
    env.as_contract(contract_address, || {
        env.storage()
            .persistent()
            .extend_ttl(&rule_key, RULE_TTL_BUMP, RULE_TTL_BUMP);
    });

    let seq = env.ledger().sequence();
    // Advance so remaining TTL (RULE_TTL_BUMP - advance) sits below the
    // RULE_TTL_THRESHOLD while staying well clear of actual expiry.
    env.ledger()
        .set_sequence_number(seq + (RULE_TTL_BUMP - RULE_TTL_THRESHOLD / 5));

    env.as_contract(contract_address, || {
        env.storage().persistent().get_ttl(&rule_key)
    })
}

fn make_payment_ttl_stale(
    env: &Env,
    contract_address: &Address,
    merchant: &Address,
    reference: &BytesN<32>,
) -> u32 {
    let key = DataKey::Payment(merchant.clone(), reference.clone());
    env.as_contract(contract_address, || {
        env.storage()
            .persistent()
            .extend_ttl(&key, PAYMENT_TTL_BUMP, PAYMENT_TTL_BUMP);
    });
    let seq = env.ledger().sequence();
    env.ledger()
        .set_sequence_number(seq + (PAYMENT_TTL_BUMP - PAYMENT_TTL_THRESHOLD / 5));
    env.as_contract(contract_address, || {
        env.storage().persistent().get_ttl(&key)
    })
}

fn make_merchant_ttl_stale(env: &Env, contract_address: &Address, merchant: &Address) -> u32 {
    let key = DataKey::Merchant(merchant.clone());
    env.as_contract(contract_address, || {
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_BUMP, MERCHANT_TTL_BUMP);
    });
    let seq = env.ledger().sequence();
    env.ledger()
        .set_sequence_number(seq + (MERCHANT_TTL_BUMP - MERCHANT_TTL_THRESHOLD / 5));
    env.as_contract(contract_address, || {
        env.storage().persistent().get_ttl(&key)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TtlClassification {
    InstanceKeepAlive,
    PersistentKeepAlive,
    Neutral(&'static str),
    NotReader,
}

// Any new public entry point must be added here and classified. Read API
// neutral entries carry their rationale; mutation paths are recorded so the
// source inventory detects new public methods regardless of their name.
const ENTRYPOINT_TTL_CLASSIFICATIONS: &[(&str, TtlClassification)] = &[
    (
        "supports_interface",
        TtlClassification::Neutral("compile-time interface probe"),
    ),
    (
        "version",
        TtlClassification::Neutral("compile-time schema version"),
    ),
    (
        "get_capabilities",
        TtlClassification::Neutral("compile-time capability list"),
    ),
    ("init", TtlClassification::NotReader),
    (
        "is_initialized",
        TtlClassification::Neutral("initialization probe"),
    ),
    ("get_admin", TtlClassification::InstanceKeepAlive),
    ("get_threshold", TtlClassification::InstanceKeepAlive),
    ("get_governance", TtlClassification::InstanceKeepAlive),
    ("get_recovery_address", TtlClassification::InstanceKeepAlive),
    ("update_governance", TtlClassification::NotReader),
    ("initiate_recovery", TtlClassification::NotReader),
    ("cancel_recovery", TtlClassification::NotReader),
    ("update_recovery_address", TtlClassification::NotReader),
    ("execute_recovery", TtlClassification::NotReader),
    ("transfer_admin", TtlClassification::NotReader),
    ("change_threshold", TtlClassification::NotReader),
    ("upgrade", TtlClassification::NotReader),
    ("pause", TtlClassification::NotReader),
    ("unpause", TtlClassification::NotReader),
    (
        "is_paused",
        TtlClassification::Neutral("pause-state check does not keep the instance alive"),
    ),
    ("migrate", TtlClassification::NotReader),
    ("schedule", TtlClassification::NotReader),
    ("execute", TtlClassification::NotReader),
    ("cancel", TtlClassification::NotReader),
    ("register_merchant", TtlClassification::NotReader),
    ("unregister_merchant", TtlClassification::NotReader),
    (
        "is_merchant_registered",
        TtlClassification::Neutral("public registry probe does not extend marker TTL"),
    ),
    ("set_settlement_rule", TtlClassification::NotReader),
    ("clear_settlement_rule", TtlClassification::NotReader),
    ("set_default_rule", TtlClassification::NotReader),
    (
        "get_default_rule",
        TtlClassification::Neutral("configuration lookup is not keep-alive intent"),
    ),
    (
        "get_settlement_rule",
        TtlClassification::PersistentKeepAlive,
    ),
    (
        "get_settlement_rule_no_bump",
        TtlClassification::Neutral("explicit no-bump public lookup"),
    ),
    (
        "get_effective_rule",
        TtlClassification::Neutral("public rule resolution is TTL-neutral"),
    ),
    (
        "get_rule_provenance",
        TtlClassification::Neutral("public rule resolution is TTL-neutral"),
    ),
    ("store_payment_reference", TtlClassification::NotReader),
    (
        "calculate_fee_split",
        TtlClassification::Neutral("public fee quote is TTL-neutral"),
    ),
    ("prune_payment", TtlClassification::NotReader),
    ("prune_payments", TtlClassification::NotReader),
    (
        "get_payment_reference",
        TtlClassification::PersistentKeepAlive,
    ),
    ("get_payments", TtlClassification::PersistentKeepAlive),
];

#[test]
fn every_public_entrypoint_has_a_ttl_classification() {
    let mut classified = BTreeSet::new();
    for (name, classification) in ENTRYPOINT_TTL_CLASSIFICATIONS {
        assert!(
            classified.insert(*name),
            "duplicate TTL classification for {name}"
        );
        if let TtlClassification::Neutral(reason) = classification {
            assert!(
                !reason.is_empty(),
                "TTL-neutral entry point {name} needs a reason"
            );
        }
    }

    let source_entrypoints = [
        include_str!("../admin.rs"),
        include_str!("../merchant.rs"),
        include_str!("../settlement.rs"),
        include_str!("../payments.rs"),
    ]
    .into_iter()
    .flat_map(str::lines)
    .filter_map(|line| {
        line.trim_start()
            .strip_prefix("pub fn ")
            .and_then(|signature| signature.split('(').next())
    })
    .collect::<BTreeSet<_>>();

    assert_eq!(
        source_entrypoints, classified,
        "every public entry point must be added to ENTRYPOINT_TTL_CLASSIFICATIONS; \
         readers must declare keep-alive or a TTL-neutral exemption"
    );
}

#[test]
fn public_instance_readers_bump_and_neutral_probes_do_not() {
    let (env, client, _admins, merchant) = setup();

    for (name, read) in [
        ("get_admin", 0),
        ("get_threshold", 1),
        ("get_governance", 2),
        ("get_recovery_address", 3),
    ] {
        make_instance_ttl_stale(&env, &client.address);
        match read {
            0 => {
                client.get_admin();
            }
            1 => {
                client.get_threshold();
            }
            2 => {
                client.get_governance();
            }
            _ => {
                client.get_recovery_address();
            }
        }
        let ttl_after = env.as_contract(&client.address, || env.storage().instance().get_ttl());
        assert_eq!(
            ttl_after, READ_INSTANCE_TTL_BUMP,
            "{name} must bump the shared instance TTL"
        );
    }

    make_instance_ttl_stale(&env, &client.address);
    let ttl_before = env.as_contract(&client.address, || env.storage().instance().get_ttl());
    client.supports_interface(&1);
    client.version();
    client.get_capabilities();
    client.is_initialized();
    client.is_paused();
    client.get_default_rule();
    let ttl_after = env.as_contract(&client.address, || env.storage().instance().get_ttl());
    assert_eq!(
        ttl_after, ttl_before,
        "compile-time probes and public config reads must be TTL-neutral"
    );

    client.register_merchant(&_admins, &merchant);
    let marker_ttl_before = make_merchant_ttl_stale(&env, &client.address, &merchant);
    assert!(client.is_merchant_registered(&merchant));
    let marker_ttl_after = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Merchant(merchant.clone()))
    });
    assert_eq!(
        marker_ttl_after, marker_ttl_before,
        "is_merchant_registered must not extend the merchant marker TTL"
    );
}

#[test]
fn payment_read_entrypoints_bump_the_payment_ttl() {
    let (env, client, _admins, merchant) = setup();
    client.register_merchant(&_admins, &merchant);
    let reference = BytesN::from_array(&env, &[0x71; 32]);
    client.store_payment_reference(&merchant, &reference, &1_000);

    let before = make_payment_ttl_stale(&env, &client.address, &merchant, &reference);
    assert!(before < PAYMENT_TTL_THRESHOLD);
    assert!(client
        .get_payment_reference(&merchant, &reference, &soroban_sdk::Vec::new(&env))
        .is_some());
    let after_single = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Payment(merchant.clone(), reference.clone()))
    });
    assert_eq!(after_single, PAYMENT_TTL_BUMP);

    make_payment_ttl_stale(&env, &client.address, &merchant, &reference);
    let page = client.get_payments(
        &merchant,
        &soroban_sdk::vec![&env, reference.clone()],
        &0,
        &soroban_sdk::Vec::new(&env),
    );
    assert_eq!(page.records.len(), 1);
    let after_batch = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Payment(merchant, reference))
    });
    assert_eq!(after_batch, PAYMENT_TTL_BUMP);
}

#[test]
fn get_settlement_rule_no_bump_returns_same_rule_without_extending_ttl() {
    let (env, client, admins, merchant) = setup();

    let expected = client.get_settlement_rule(&merchant);
    assert!(expected.is_none(), "setup must not set a merchant rule");

    // No rule stored: both variants agree on `None` and nothing panics.
    let no_bump = client.get_settlement_rule_no_bump(&merchant);
    assert!(no_bump.is_none());
    assert!(expected.is_none());

    // With a rule stored: both variants return identical values.
    client.set_settlement_rule(&admins, &merchant, &make_rule(100, 200, 10));
    let stored = client
        .get_settlement_rule(&merchant)
        .expect("rule must be set");
    let via_no_bump = client
        .get_settlement_rule_no_bump(&merchant)
        .expect("rule must be set");
    assert_eq!(stored, via_no_bump);
}

#[test]
fn get_settlement_rule_no_bump_does_not_extend_rule_ttl() {
    let (env, client, admins, merchant) = setup();
    let contract_address = client.address.clone();

    client.set_settlement_rule(&admins, &merchant, &make_rule(100, 200, 10));

    let ttl_before = make_rule_ttl_stale(&env, &contract_address, &merchant);

    client.get_settlement_rule_no_bump(&merchant);

    let ttl_after = env.as_contract(&contract_address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Rule(merchant.clone()))
    });
    assert_eq!(
        ttl_after, ttl_before,
        "no-bump getter must not extend the rule's TTL"
    );
}

#[test]
fn get_settlement_rule_still_bumps_for_authenticated_keepalive() {
    let (env, client, admins, merchant) = setup();
    let contract_address = client.address.clone();

    client.set_settlement_rule(&admins, &merchant, &make_rule(100, 200, 10));

    let ttl_before = make_rule_ttl_stale(&env, &contract_address, &merchant);
    assert!(
        ttl_before < RULE_TTL_THRESHOLD,
        "test setup did not let the rule TTL decay below threshold: {ttl_before}"
    );

    client.get_settlement_rule(&merchant);

    let ttl_after = env.as_contract(&contract_address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Rule(merchant.clone()))
    });
    assert_eq!(
        ttl_after, RULE_TTL_BUMP,
        "bumping getter must keep extending the rule TTL to the keep-alive floor"
    );
}

#[test]
fn get_effective_rule_returns_identical_rule_without_extending_ttl() {
    let (env, client, admins, merchant) = setup();
    let contract_address = client.address.clone();

    client.set_settlement_rule(&admins, &merchant, &make_rule(150, 250, 20));

    let effective = client.get_effective_rule(&merchant);
    assert_eq!(effective.platform_fee_bps, 150);

    let ttl_before = make_rule_ttl_stale(&env, &contract_address, &merchant);

    let again = client.get_effective_rule(&merchant);
    assert_eq!(again, effective);

    let ttl_after = env.as_contract(&contract_address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Rule(merchant.clone()))
    });
    assert_eq!(
        ttl_after, ttl_before,
        "get_effective_rule must be TTL-neutral (issue #762)"
    );
}

#[test]
fn calculate_fee_split_is_ttl_neutral_for_the_merchant_rule() {
    let (env, client, admins, merchant) = setup();
    let contract_address = client.address.clone();

    client.register_merchant(&admins, &merchant);
    client.set_settlement_rule(&admins, &merchant, &make_rule(100, 200, 10));

    let split_before = client.calculate_fee_split(&merchant, &10_000);

    let ttl_before = make_rule_ttl_stale(&env, &contract_address, &merchant);

    let split_after = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(
        split_after.platform_fee_amount, split_before.platform_fee_amount,
        "platform fee must be unchanged by the TTL-neutral read (issue #760)"
    );
    assert_eq!(
        split_after.network_fee_amount, split_before.network_fee_amount,
        "network fee must be unchanged by the TTL-neutral read (issue #760)"
    );
    assert_eq!(
        split_after.merchant_payout_amount, split_before.merchant_payout_amount,
        "merchant amount must be unchanged by the TTL-neutral read (issue #760)"
    );

    let ttl_after = env.as_contract(&contract_address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Rule(merchant.clone()))
    });
    assert_eq!(
        ttl_after, ttl_before,
        "repeated calculate_fee_split calls must not extend the rule TTL"
    );
}

#[test]
fn calculate_fee_split_still_works_when_rule_ttl_is_stale() {
    let (env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);
    client.set_settlement_rule(&admins, &merchant, &make_rule(100, 200, 10));

    let _ = make_rule_ttl_stale(&env, &client.address, &merchant);

    // The stale (but unexpired) rule is still readable and resolves fees.
    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 200);
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

// Issue #777: verify DefaultRule stored in instance never expires independently.
#[test]
fn default_rule_survives_long_idle() {
    let (env, client, admins, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);

    // Advance the ledger by 60 days (well past RULE_TTL_THRESHOLD of 17280*14 = 241_920).
    // Each hop advances 100_000 ledgers, touching the contract via get_admin()
    // between hops so the instance's own TTL doesn't expire along the way.
    for _ in 0..15 {
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + 100_000);
        client.get_admin();
    }

    // After long idle, get_default_rule should still return the stored rule.
    let retrieved = client.get_default_rule();
    assert!(
        retrieved.is_some(),
        "DefaultRule should survive long idle and be retrievable via get_default_rule"
    );
    let retrieved = retrieved.unwrap();
    assert_eq!(
        retrieved.platform_fee_bps, rule.platform_fee_bps,
        "platform_fee_bps should match after long idle"
    );
    assert_eq!(
        retrieved.network_fee_bps, rule.network_fee_bps,
        "network_fee_bps should match after long idle"
    );
    assert_eq!(
        retrieved.auto_settle, rule.auto_settle,
        "auto_settle should match after long idle"
    );
}

// ---------------------------------------------------------------------------
// Issue #897: payment tombstone after unregister
// ---------------------------------------------------------------------------

/// Unregistering a merchant orphans its payment history without deleting it.
///
/// TTL window: `unregister_merchant` writes a `DataKey::ArchivedMerchant`
/// tombstone with the merchant TTL policy (`MERCHANT_TTL_BUMP`), while each
/// `DataKey::Payment` entry keeps its own, independent payment TTL. For as long
/// as the tombstone is live (i.e. until the merchant is re-registered), every
/// payment read for that merchant fails with `PaymentOrphaned` and — because
/// the read fails — never bumps the payment entry's TTL. The entry itself stays
/// in storage until an admin prunes it or its own TTL lapses.
/// Re-registration removes the tombstone (issue #685).
#[test]
fn unregister_orphans_payments_until_reregistration_clears_tombstone() {
    let (env, client, admins, merchant) = setup();
    let no_signers = soroban_sdk::vec![&env];
    client.register_merchant(&admins, &merchant);

    let kept = BytesN::<32>::from_array(&env, &[7u8; 32]);
    let pruned = BytesN::<32>::from_array(&env, &[8u8; 32]);
    client.store_payment_reference(&merchant, &kept, &1_000);
    client.store_payment_reference(&merchant, &pruned, &2_000);

    client.unregister_merchant(&admins, &merchant);

    let archived_key = DataKey::ArchivedMerchant(merchant.clone());
    let kept_key = DataKey::Payment(merchant.clone(), kept.clone());
    let pruned_key = DataKey::Payment(merchant.clone(), pruned.clone());
    let has =
        |key: &DataKey| env.as_contract(&client.address, || env.storage().persistent().has(key));
    let ttl = |key: &DataKey| {
        env.as_contract(&client.address, || env.storage().persistent().get_ttl(key))
    };

    // The tombstone is live for the merchant TTL window.
    assert!(has(&archived_key), "unregister must write the tombstone");
    assert_eq!(ttl(&archived_key), MERCHANT_TTL_BUMP);

    // Reads fail with PaymentOrphaned on both the merchant and admin paths.
    let orphaned = Error::from(SettlementError::PaymentOrphaned);
    let kept_ttl_before = ttl(&kept_key);
    assert_eq!(
        client
            .try_get_payment_reference(&merchant, &kept, &no_signers)
            .err(),
        Some(Ok(orphaned))
    );
    assert_eq!(
        client
            .try_get_payment_reference(&merchant, &kept, &admins)
            .err(),
        Some(Ok(orphaned))
    );
    let refs = soroban_sdk::vec![&env, kept.clone(), pruned.clone()];
    assert_eq!(
        client
            .try_get_payments(&merchant, &refs, &0, &no_signers)
            .err(),
        Some(Ok(orphaned))
    );
    assert_eq!(
        client.try_get_payments(&merchant, &refs, &0, &admins).err(),
        Some(Ok(orphaned))
    );

    // Auth still gates reads: without the merchant's authorization the call
    // fails on auth before the orphan check, so orphan status isn't leaked.
    env.set_auths(&[]);
    let unauthenticated = client
        .try_get_payment_reference(&merchant, &kept, &no_signers)
        .err()
        .expect("read without merchant auth must fail");
    assert_ne!(unauthenticated, Ok(orphaned));
    env.mock_all_auths();

    // The stored entries survive the unregister and the failed reads, with
    // their TTL untouched, until pruned.
    assert!(has(&kept_key), "unregister must not delete payment records");
    assert!(
        has(&pruned_key),
        "unregister must not delete payment records"
    );
    assert_eq!(ttl(&kept_key), kept_ttl_before);

    client.prune_payment(&admins, &merchant, &pruned);
    assert!(!has(&pruned_key), "prune removes the orphaned entry");
    assert!(has(&kept_key), "unpruned entries remain stored");

    // Re-registration clears the tombstone and new records are readable.
    client.register_merchant(&admins, &merchant);
    assert!(
        !has(&archived_key),
        "re-registration must clear the tombstone"
    );

    let fresh = BytesN::<32>::from_array(&env, &[9u8; 32]);
    client.store_payment_reference(&merchant, &fresh, &3_000);
    let record = client
        .get_payment_reference(&merchant, &fresh, &no_signers)
        .expect("new record must be readable after re-registration");
    assert_eq!(record.amount, 3_000);

    // The tombstone is per merchant, not per record: the unpruned entry from
    // the earlier registration is readable again, and the pruned one is gone.
    let refs = soroban_sdk::vec![&env, kept.clone(), pruned.clone(), fresh.clone()];
    let page = client.get_payments(&merchant, &refs, &0, &no_signers);
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records.get(0).unwrap().amount, 1_000);
    assert_eq!(page.records.get(1).unwrap().amount, 3_000);
}

// ---------------------------------------------------------------------------
// Issue #898: re-registration does not resurrect the merchant rule
// ---------------------------------------------------------------------------

/// `unregister_merchant` removes the merchant-specific rule and emits
/// `settlement_rule_cleared` with the canonical `(admin, removed, fallback)`
/// payload. Re-registering the merchant must not bring that rule back: the
/// effective rule resolves through the fallback chain to the global default.
#[test]
fn reregistration_falls_back_to_default_rule_not_old_merchant_rule() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // Distinct from both the merchant rule and BOOTSTRAP_DEFAULT_RULE so the
    // fallback is unambiguous.
    let default_rule = make_rule(150, 30, 10);
    let merchant_rule = make_rule(250, 50, 20);
    client.set_default_rule(&admins, &default_rule);
    client.set_settlement_rule(&admins, &merchant, &merchant_rule);

    let rule_key = DataKey::Rule(merchant.clone());
    let has_rule = || {
        env.as_contract(&client.address, || {
            env.storage().persistent().has(&rule_key)
        })
    };
    assert!(has_rule());
    assert_eq!(
        client.get_effective_rule(&merchant),
        merchant_rule
    );

    client.unregister_merchant(&admins, &merchant);
    assert!(!has_rule(), "unregister must remove DataKey::Rule");

    // settlement_rule_cleared: topics (name, merchant), data
    // (admin, removed, fallback) with the default rule as the fallback.
    let cleared_topic = Symbol::new(&env, events::SETTLEMENT_RULE_CLEARED_EVENT);
    let all = env.events().all();
    let mut cleared = None;
    for i in 0..all.len() {
        let (contract, topics, data) = all.get(i).unwrap();
        if contract == client.address
            && Symbol::from_val(&env, &topics.get(0).unwrap()) == cleared_topic
        {
            assert!(cleared.is_none(), "settlement_rule_cleared emitted once");
            assert_eq!(topics.len(), 2);
            assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
            cleared = Some(data);
        }
    }
    let data = cleared.expect("unregister must emit settlement_rule_cleared");
    let (admin, removed, fallback): (Address, SettlementRule, SettlementRule) =
        TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!(admin, admins.get(0).unwrap());
    assert_eq!(removed, merchant_rule);
    assert_eq!(fallback, default_rule);

    // Re-register: precedence is merchant rule > default rule > governance >
    // bootstrap. With no merchant rule stored, the default must win — never
    // the pre-unregister merchant rule.
    client.register_merchant(&admins, &merchant);
    assert!(
        !has_rule(),
        "re-registration must not restore DataKey::Rule"
    );

    let effective = client.get_effective_rule(&merchant);
    assert_eq!(effective, default_rule);
    assert_ne!(effective, merchant_rule);

    // New payments are priced with the default rule, not the old one.
    let reference = BytesN::<32>::from_array(&env, &[5u8; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);
    let record = client
        .get_payment_reference(&merchant, &reference, &soroban_sdk::vec![&env])
        .unwrap();
    assert_eq!(record.platform_fee_bps, default_rule.platform_fee_bps);
    assert_eq!(record.network_fee_bps, default_rule.network_fee_bps);
    assert!(!has_rule(), "no stale rule written by the payment path");
}
