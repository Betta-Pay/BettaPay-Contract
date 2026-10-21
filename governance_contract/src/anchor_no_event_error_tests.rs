use bettapay_common::constants::MIN_FEE_BPS;
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{vec, Address};

use super::*;
use super::{setup, setup_with};

/// Captures the event count before `call`, runs `call` (expected to panic),
/// and asserts no new events were emitted. The panic from `call` is expected
/// and is caught by the `#[should_panic]` attribute on the test.
fn assert_no_new_events(env: &Env, prev: u32, msg: &str) {
    assert_eq!(env.events().all().len(), prev, "{}", msg);
}

// ---------------------------------------------------------------------------
// Anchor error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn upsert_anchor_emits_no_event_when_unauthorized() {
    let (env, client, _admins) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    let non_admin = Address::generate(&env);

    let prev = env.events().all().len();
    client.upsert_anchor(&vec![&env, non_admin], &asset, &anchor);
    assert_no_new_events(&env, prev, "unauthorized upsert must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn upsert_anchor_emits_no_event_when_zero_asset() {
    let (env, client, admins) = setup();
    let anchor = Address::generate(&env);
    let zero = bettapay_common::storage::zero_address(&env);

    let prev = env.events().all().len();
    client.upsert_anchor(&admins, &zero, &anchor);
    assert_no_new_events(&env, prev, "upsert with zero asset must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn upsert_anchor_emits_no_event_when_zero_anchor() {
    let (env, client, admins) = setup();
    let asset = Address::generate(&env);
    let zero = bettapay_common::storage::zero_address(&env);

    let prev = env.events().all().len();
    client.upsert_anchor(&admins, &asset, &zero);
    assert_no_new_events(&env, prev, "upsert with zero anchor must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn remove_anchor_emits_no_event_when_unauthorized() {
    let (env, client, admins) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admins, &asset, &anchor);

    let non_admin = Address::generate(&env);
    let prev = env.events().all().len();
    client.remove_anchor(&vec![&env, non_admin], &asset);
    assert_no_new_events(&env, prev, "unauthorized remove must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #200)")]
fn remove_anchor_emits_no_event_when_missing() {
    let (env, client, admins) = setup();
    let missing_asset = Address::generate(&env);

    let prev = env.events().all().len();
    client.remove_anchor(&admins, &missing_asset);
    assert_no_new_events(&env, prev, "remove missing anchor must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn upsert_anchor_emits_no_event_when_paused() {
    let (env, client, admins) = setup();
    client.pause(&admins);

    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    let prev = env.events().all().len();
    client.upsert_anchor(&admins, &asset, &anchor);
    assert_no_new_events(&env, prev, "upsert while paused must not emit events");
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn remove_anchor_emits_no_event_when_paused() {
    let (env, client, admins) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admins, &asset, &anchor);

    client.pause(&admins);
    let prev = env.events().all().len();
    client.remove_anchor(&admins, &asset);
    assert_no_new_events(&env, prev, "remove while paused must not emit events");
}

// ---------------------------------------------------------------------------
// Anchor success path — must still emit its event
// ---------------------------------------------------------------------------

#[test]
fn upsert_anchor_success_emits_anchor_upserted_event() {
    let (env, client, admins) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    let prev = env.events().all().len();
    client.upsert_anchor(&admins, &asset, &anchor);
    assert_eq!(
        env.events().all().len(),
        prev + 1,
        "successful upsert must emit anchor_upserted"
    );
}

// ---------------------------------------------------------------------------
// Fee-config error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_fee_config_emits_no_event_when_paused() {
    let (env, client, admins) = setup();
    client.pause(&admins);

    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    let prev = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config while paused must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn set_fee_config_emits_no_event_when_unauthorized() {
    let (env, client, _admins) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    let non_admin = Address::generate(&env);

    let prev = env.events().all().len();
    client.set_fee_config(&vec![&env, non_admin], &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config unauthorized must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn set_fee_config_emits_no_event_when_insufficient_signers() {
    // Setup: 2 admins, threshold 2. set_fee_config requires 2 signers
    // but we provide only 1 -> Unauthorized (#3).
    let (env, client, admins, _recovery) = setup_with(2, 2);
    let a1 = admins.get(0).unwrap();

    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    let one_signer = vec![&env, a1.clone()];
    let prev = env.events().all().len();
    client.set_fee_config(&one_signer, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config with insufficient signers must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_fee_config_emits_no_event_when_bps_above_max() {
    let (env, client, admins) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 5_001,
        network_fee_bps: 100,
    };

    let prev = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config with invalid bps must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_fee_config_emits_no_event_when_bps_below_min() {
    let (env, client, admins) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 100,
        network_fee_bps: 4,
    };

    let prev = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config with below-min bps must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_fee_config_rejects_u32_max_platform_fee() {
    let (env, client, admins) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: u32::MAX,
        network_fee_bps: MIN_FEE_BPS,
    };

    let prev = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config with u32::MAX platform fee must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_fee_config_rejects_u32_max_network_fee() {
    let (env, client, admins) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: MIN_FEE_BPS,
        network_fee_bps: u32::MAX,
    };

    let prev = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    assert_no_new_events(
        &env,
        prev,
        "set_fee_config with u32::MAX network fee must not emit events",
    );
}

// ---------------------------------------------------------------------------
// System-parameter error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #201)")]
fn update_system_param_emits_no_event_when_negative() {
    let (env, client, admins) = setup();
    let key = Symbol::new(&env, "negative_param");

    let prev = env.events().all().len();
    client.update_system_param(&admins, &key, &-1);
    assert_no_new_events(
        &env,
        prev,
        "update_system_param with negative value must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #201)")]
fn update_system_param_emits_no_event_when_bypass_above_one() {
    let (env, client, admins) = setup();
    let key = Symbol::new(&env, "bypass_gov_fees");

    let prev = env.events().all().len();
    client.update_system_param(&admins, &key, &2);
    assert_no_new_events(
        &env,
        prev,
        "update_system_param with bypass_gov_fees > 1 must not emit events",
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn update_system_param_emits_no_event_when_unauthorized() {
    let (env, client, _admins) = setup();
    let key = Symbol::new(&env, "some_key");
    let non_admin = Address::generate(&env);

    let prev = env.events().all().len();
    client.update_system_param(&vec![&env, non_admin], &key, &100);
    assert_no_new_events(
        &env,
        prev,
        "unauthorized update_system_param must not emit events",
    );
}

// ---------------------------------------------------------------------------
// Change-threshold error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn change_threshold_emits_no_event_when_insufficient_signatures() {
    // Setup: 3 admins, threshold 2. change_threshold requires 3 signers
    // (current_threshold + 1), but we only provide 1 → Unauthorized (#3).
    // The new_threshold (2) is within bounds so the bound check passes first.
    let (env, client, admins, _recovery) = setup_with(3, 2);
    let a1 = admins.get(0).unwrap();

    let single_signer = vec![&env, a1.clone()];
    let prev = env.events().all().len();
    client.change_threshold(&single_signer, &2);
    assert_eq!(
        env.events().all().len(),
        prev,
        "change_threshold with insufficient sigs must not emit events"
    );
}

// ---------------------------------------------------------------------------
// Pause / unpause error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn pause_emits_no_event_when_already_paused() {
    let (env, client, admins) = setup();
    client.pause(&admins);

    let prev = env.events().all().len();
    client.pause(&admins);
    assert_eq!(
        env.events().all().len(),
        prev,
        "double pause must not emit events"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn unpause_emits_no_event_when_already_unpaused() {
    let (env, client, admins) = setup();

    let prev = env.events().all().len();
    client.unpause(&admins);
    assert_eq!(
        env.events().all().len(),
        prev,
        "unpause when not paused must not emit events"
    );
}

// ---------------------------------------------------------------------------
// Recovery error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn cancel_recovery_emits_no_event_when_nothing_pending() {
    let (env, client, admins) = setup();

    let prev = env.events().all().len();
    client.cancel_recovery(&admins);
    assert_eq!(
        env.events().all().len(),
        prev,
        "cancel_recovery with nothing pending must not emit events"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn execute_recovery_emits_no_event_when_delay_active() {
    let (env, client, _admins) = setup();
    let new_admin = Address::generate(&env);

    client.initiate_recovery(&new_admin);

    // Do NOT advance time — delay should still be active.
    let prev = env.events().all().len();
    client.execute_recovery();
    assert_eq!(
        env.events().all().len(),
        prev,
        "execute_recovery before delay must not emit events"
    );
}
