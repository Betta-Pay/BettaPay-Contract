//! Issue #990: the event topic set is stable.
//!
//! `bettapay_common::events::ALL_EVENT_TOPICS` is the single registry of
//! every topic name any BettaPay contract may emit. This module pins that
//! complete set and exercises settlement entry points, asserting every
//! emitted `topic[0]` matches a registry constant.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol};

use bettapay_common::events;

use super::setup;

const EXPECTED_EVENT_TOPICS: &[&str] = &[
    "admin_transferred",
    "paused",
    "unpaused",
    "recovery_initiated",
    "recovery_cancelled",
    "recovery_executed",
    "contract_upgraded",
    "threshold_changed",
    "recovery_address_updated",
    "initialized",
    "migrated",
    "sys_param_updated",
    "fee_config_updated",
    "anchor_upserted",
    "anchor_removed",
    "governance_updated",
    "merchant_registered",
    "merchant_consent",
    "merchant_unregistered",
    "settlement_rule_updated",
    "settlement_rule_cleared",
    "default_rule_updated",
    "bootstrap_fallback",
    "payment_stored",
    "payments_pruned",
    "payment_pruned",
    "op_scheduled",
    "op_executed",
    "op_cancelled",
    "fee_ceiling_rejected",
];

/// Builds the registry as `Symbol`s once per `Env`.
fn registry(env: &Env) -> soroban_sdk::Vec<Symbol> {
    let mut out = soroban_sdk::Vec::new(env);
    for name in events::ALL_EVENT_TOPICS {
        out.push_back(Symbol::new(env, name));
    }
    out
}

/// Asserts every event emitted so far has a `topic[0]` that matches a
/// registry constant, failing with the offending topic's position if not.
fn assert_all_topics_registered(env: &Env) {
    let known = registry(env);
    let all = env.events().all();
    for i in 0..all.len() {
        let (_contract, topics, _data) = all.get(i).unwrap();
        assert!(!topics.is_empty(), "event {i} has no topics at all");
        let topic0 = Symbol::from_val(env, &topics.get(0).unwrap());
        assert!(
            known.contains(&topic0),
            "event {i} emitted an unregistered topic: {topic0:?}"
        );
    }
}

/// Exercises a broad lifecycle — merchant/rule/payment management, recovery,
/// governance, pause/unpause, and the scheduled-operation queue — and
/// asserts every topic emitted along the way is a registered member of
/// `ALL_EVENT_TOPICS`. This is the "fails if an unexpected topic appears"
/// guard: an inline-literal or typo'd topic anywhere in this path would
/// surface here as an assertion failure rather than passing unnoticed.
#[test]
fn lifecycle_events_use_only_registered_topics() {
    let (env, client, admins, merchant) = setup();

    // Merchant + rule + payment lifecycle.
    client.register_merchant(&admins, &merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 20,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
    client.set_default_rule(&admins, &rule);

    let reference = BytesN::from_array(&env, &[3; 32]);
    client.store_payment_reference(&merchant, &reference, &1_000);
    client.prune_payments(&admins, &merchant, &soroban_sdk::vec![&env, reference]);

    let single_reference = BytesN::from_array(&env, &[4; 32]);
    client.store_payment_reference(&merchant, &single_reference, &1_000);
    client.prune_payment(&admins, &merchant, &single_reference);

    client.clear_settlement_rule(&admins, &merchant);
    client.unregister_merchant(&admins, &merchant);

    // Governance + recovery administration.
    let new_governance = super::register_governance(&env);
    client.update_governance(&admins, &new_governance);
    client.update_recovery_address(&admins, &Address::generate(&env));

    client.initiate_recovery(&Address::generate(&env));
    client.cancel_recovery(&admins);

    // Pause / unpause (issue #991: pause now carries a bounded reason code).
    client.pause(&admins, &1u32);
    client.unpause(&admins);

    // Scheduled-operation queue: schedule, execute, and cancel.
    let scheduled_merchant = Address::generate(&env);
    let op = Operation::RegisterMerchant(scheduled_merchant);
    client.schedule(&admins, &op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap(), &op);

    let other_merchant = Address::generate(&env);
    let other_op = Operation::RegisterMerchant(other_merchant);
    client.schedule(&admins, &other_op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.cancel(&admins, &other_op);

    assert_all_topics_registered(&env);
}

/// `change_threshold` requires `current_threshold + 1` signers, so it needs
/// its own multi-admin setup independent of `setup()`'s single admin.
#[test]
fn threshold_changed_topic_is_registered() {
    let env = Env::default();
    env.mock_all_auths();
    let admin1 = Address::generate(&env);
    let admin2 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin1, admin2];
    let recovery = Address::generate(&env);
    let governance = super::register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery);

    client.change_threshold(&admins, &2);

    assert_all_topics_registered(&env);
}

/// Cross-checks that the topic constants `event_topic_conformity_tests`
/// asserts against are themselves registered members of `ALL_EVENT_TOPICS`
/// (issue #990 scope: cross-check the settlement conformity tests against
/// the registry). If a future edit added a new conformity assertion against
/// a topic that was never added to the shared registry, this test would
/// catch the gap.
#[test]
fn conformity_tested_topics_are_all_registered() {
    let env = Env::default();
    let known = registry(&env);

    let conformity_topics: &[&str] = &[
        events::THRESHOLD_CHANGED_EVENT,
        events::GOVERNANCE_UPDATED_EVENT,
        events::MERCHANT_REGISTERED_EVENT,
        events::MERCHANT_CONSENT_EVENT,
        events::MERCHANT_UNREGISTERED_EVENT,
        events::SETTLEMENT_RULE_UPDATED_EVENT,
        events::SETTLEMENT_RULE_CLEARED_EVENT,
        events::DEFAULT_RULE_UPDATED_EVENT,
        events::PAYMENT_STORED_EVENT,
        events::PAYMENT_PRUNED_EVENT,
        events::OP_SCHEDULED_EVENT,
        events::OP_EXECUTED_EVENT,
        events::OP_CANCELLED_EVENT,
    ];

    for name in conformity_topics {
        let topic = Symbol::new(&env, name);
        assert!(
            known.contains(&topic),
            "conformity-tested topic `{name}` is missing from ALL_EVENT_TOPICS"
        );
    }
}

#[test]
fn complete_event_topic_set_is_stable() {
    assert_eq!(
        events::ALL_EVENT_TOPICS,
        EXPECTED_EVENT_TOPICS,
        "event topic additions, removals, or reordering must be deliberate"
    );
}

/// Proves the membership check used above actually has teeth: a topic name
/// that was never added to the registry must NOT be reported as a member.
#[test]
fn unregistered_topic_name_is_not_a_member() {
    let env = Env::default();
    let known = registry(&env);
    let bogus = Symbol::new(&env, "not_a_real_topic");
    assert!(
        !known.contains(&bogus),
        "a made-up topic name must not be found in the registry"
    );
}
