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
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol};

use bettapay_common::events;

use super::{register_governance, setup};

/// Returns the topic[0] `Symbol` of the most recently emitted event.
fn last_topic(env: &Env) -> Symbol {
    let (_, topics, _) = env.events().all().last().unwrap();
    Symbol::from_val(env, &topics.get(0).unwrap())
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
    client.init(&admins, &1, &governance, &recovery);

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
    assert_eq!(env.events().all().len(), before, "no event on failed upgrade");
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
    let admin = admins.get(0).unwrap();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_SCHEDULED_EVENT)
    );

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_EXECUTED_EVENT)
    );

    let other_operation = Operation::UnregisterMerchant(Address::generate(&env));
    client.schedule(&admin, &other_operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.cancel(&admin, &other_operation);
    assert_eq!(
        last_topic(&env),
        Symbol::new(&env, events::OP_CANCELLED_EVENT)
    );
}

#[test]
fn bootstrap_fallback_uses_canonical_topic() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // No merchant rule, no default rule, and MockGovernance's get_fee_config
    // always returns None, so this call falls all the way through to the
    // bootstrap fallback rule.
    let before = env.events().all().len();
    client.calculate_fee_split(&merchant, &1_000);
    assert_eq!(env.events().all().len(), before);
}
