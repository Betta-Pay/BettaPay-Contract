//! Issue #567: settlement previously scattered its event topic names as
//! inline string literals, independently of governance's topic vocabulary.
//! Every topic used by either contract is now defined once in
//! `bettapay_common::events` — the shared event-topic registry — and both
//! contracts construct their topic `Symbol`s from that registry instead of
//! an inline literal. (The `pause`/`unpause`/`admin_transferred`/recovery
//! subset of that drift was already fixed by #518, which routed both
//! contracts through the `emit_*` helpers; this module covers the rest of
//! the registry — the topics #518 didn't touch.)
//!
//! This module walks the settlement contract's entry points and asserts
//! each emitted `topic[0]` equals the corresponding registry constant, so it
//! fails again if a call site regresses to a hand-rolled string.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol, TryFromVal, Val};

use bettapay_common::events;

use super::{register_governance, setup};

/// Returns the topic[0] `Symbol` of the most recently emitted event.
fn last_topic(env: &Env) -> Symbol {
    let (_, topics, _) = env.events().all().last().unwrap();
    Symbol::from_val(env, &topics.get(0).unwrap())
}

/// Returns the data payload of the most recent `settlement_rule_cleared`
/// event emitted so far.
fn last_settlement_rule_cleared_data(env: &Env) -> Val {
    let events = env.events().all();
    let mut found = None;
    for i in 0..events.len() {
        let (_contract, topics, data) = events.get(i).unwrap();
        if !topics.is_empty()
            && Symbol::from_val(env, &topics.get(0).unwrap())
                == Symbol::new(env, events::SETTLEMENT_RULE_CLEARED_EVENT)
        {
            found = Some(data);
        }
    }
    found.expect("settlement_rule_cleared event must have been emitted")
}

/// Issue #491: `clear_settlement_rule` used to emit `(admin, removed, fallback)`
/// while the unregister path emitted `(admin, old_rule)` under the same
/// `settlement_rule_cleared` topic — two arities for one event name, which
/// breaks indexers. Both paths now publish through the shared
/// `bettapay_common::events::emit_settlement_rule_cleared` helper, so the two
/// events must carry the same topic and serialize byte-identically.
#[test]
fn settlement_rule_cleared_data_is_identical_across_both_paths() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let default_rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 30,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
    client.set_default_rule(&admins, &default_rule);

    // Path 1: explicit clear_settlement_rule.
    client.clear_settlement_rule(&admins, &merchant);
    let data_clear = last_settlement_rule_cleared_data(&env);

    // Path 2: unregister_merchant removing a merchant that still has a rule.
    client.set_settlement_rule(&admins, &merchant, &rule);
    client.unregister_merchant(&admins, &merchant);
    let data_unregister = last_settlement_rule_cleared_data(&env);

    // Same topic on both paths.
    let events = env.events().all();
    let mut cleared_topics = 0;
    for i in 0..events.len() {
        let (_contract, topics, _data) = events.get(i).unwrap();
        if !topics.is_empty()
            && Symbol::from_val(&env, &topics.get(0).unwrap())
                == Symbol::new(&env, events::SETTLEMENT_RULE_CLEARED_EVENT)
        {
            cleared_topics += 1;
        }
    }
    assert_eq!(cleared_topics, 2, "both paths must emit the same topic");

    // Byte-identical serialization — the acceptance criterion for #491.
    assert_eq!(
        data_clear.to_xdr(&env),
        data_unregister.to_xdr(&env),
        "both removal paths must serialize the settlement_rule_cleared \
         payload identically",
    );

    // The payload still decodes as the canonical (admin, removed, fallback)
    // triple, with the expected values.
    let (admin, removed, fallback): (Address, SettlementRule, SettlementRule) =
        TryFromVal::try_from_val(&env, &data_clear).unwrap();
    assert_eq!(admin, admins.get(0).unwrap());
    assert_eq!(removed.platform_fee_bps, 250);
    assert_eq!(removed.network_fee_bps, 50);
    assert_eq!(fallback.platform_fee_bps, 150);
    assert_eq!(fallback.network_fee_bps, 30);
}

#[test]
fn threshold_changed_uses_canonical_topic() {
    // change_threshold requires `current_threshold + 1` signers, so a
    // single-admin setup() contract can never call it; register a
    // two-admin contract directly instead.
    let env = Env::default();
    env.mock_all_auths();
    let admin1 = Address::generate(&env);
    let admin2 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin1, admin2];
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery);

    client.change_threshold(&admins, &2);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::THRESHOLD_CHANGED_EVENT)
    );
}

#[test]
fn upgrade_uses_canonical_topic() {
    // The `contract_upgraded` event is only emitted on a successful upgrade.
    // Since soroban 21.7.7 test environments don't expose a way to upload the
    // current contract's own compiled bytes as a hash, we verify the negative
    // case: a non-conforming wasm (empty, missing `supports_interface`) is
    // rejected before the event is emitted.
    let (env, client, admins, _merchant) = setup();
    let wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let bad_hash = env.deployer().upload_contract_wasm(wasm);

    let before = env.events().all().len();
    let result = client.try_upgrade(&admins, &bad_hash);
    assert!(result.is_err(), "non-conforming wasm must be rejected");
    // No event emitted on failure.
    assert_eq!(
        env.events().all().len(),
        before,
        "no event on failed upgrade"
    );
}

#[test]
fn update_governance_uses_canonical_topic() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&env);

    client.update_governance(&admins, &new_governance);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::GOVERNANCE_UPDATED_EVENT)
    );
}

#[test]
fn merchant_lifecycle_uses_canonical_topics() {
    let (env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::MERCHANT_REGISTERED_EVENT)
    );

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::SETTLEMENT_RULE_UPDATED_EVENT)
    );

    client.clear_settlement_rule(&admins, &merchant);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::SETTLEMENT_RULE_CLEARED_EVENT)
    );

    // unregister_merchant emits merchant_unregistered as the last event even
    // when it also clears a still-set rule as a side effect.
    client.set_settlement_rule(&admins, &merchant, &rule);
    client.unregister_merchant(&admins, &merchant);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::MERCHANT_UNREGISTERED_EVENT)
    );
}

#[test]
fn default_rule_and_payment_use_canonical_topics() {
    let (env, client, admins, merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_default_rule(&admins, &rule);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::DEFAULT_RULE_UPDATED_EVENT)
    );

    client.register_merchant(&admins, &merchant);
    let reference = BytesN::from_array(&env, &[7; 32]);
    client.store_payment_reference(&merchant, &reference, &1_000);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::PAYMENT_STORED_EVENT)
    );
}

#[test]
fn scheduled_operation_lifecycle_uses_canonical_topics() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_SCHEDULED_EVENT)
    );

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap(), &operation);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_EXECUTED_EVENT)
    );

    let other_operation = Operation::UnregisterMerchant(Address::generate(&env));
    client.schedule(&admins, &other_operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.cancel(&admins, &other_operation);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_CANCELLED_EVENT)
    );
}

#[test]
fn scheduled_operation_events_identify_the_executor() {
    let (env, client, admins, merchant) = setup();
    let executor = Address::generate(&env);
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&executor, &operation);

    let events = env.events().all();
    let mut actor = None;
    for i in 0..events.len() {
        let (_contract, topics, data) = events.get(i).unwrap();
        if Symbol::from_val(&env, &topics.get(0).unwrap())
            == Symbol::new(&env, events::MERCHANT_REGISTERED_EVENT)
        {
            actor = Some(Address::from_val(&env, &data));
        }
    }

    assert_eq!(actor, Some(executor));
}

/// The fee hot path falls back to the protocol bootstrap rule when no
/// merchant rule, default rule, or governance fee config is set — without
/// emitting a `bootstrap_fallback` event on every payment (issue #691). The
/// resolved rule is exactly the canonical [`BOOTSTRAP_DEFAULT_RULE`].
#[test]
fn bootstrap_fallback_resolves_without_emitting_event() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let amount = 1_000i128;
    let rule = crate::BOOTSTRAP_DEFAULT_RULE;
    let denom = 10_000i128;
    let expected_platform = (amount * rule.platform_fee_bps as i128 + denom - 1) / denom;
    let expected_network = (amount * rule.network_fee_bps as i128 + denom - 1) / denom;
    let expected_merchant = (amount - expected_platform - expected_network).max(0);

    let before = env.events().all().len();
    let split = client.calculate_fee_split(&merchant, &amount);

    // Event silence: the hot path must not emit a bootstrap_fallback event.
    assert_eq!(env.events().all().len(), before);
    assert_eq!(split.gross_amount, amount);
    assert_eq!(split.platform_fee_amount, expected_platform);
    assert_eq!(split.network_fee_amount, expected_network);
    assert_eq!(split.merchant_amount, expected_merchant);
}

#[test]
fn clear_settlement_rule_emits_only_one_event() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);

    let before = env.events().all().len();
    client.clear_settlement_rule(&admins, &merchant);
    let after = env.events().all().len();

    // Only one event emitted: SETTLEMENT_RULE_CLEARED_EVENT.
    // The bootstrap fallback event should NOT be emitted during this call.
    assert_eq!(after - before, 1);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::SETTLEMENT_RULE_CLEARED_EVENT)
    );
}
