use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{vec, Address};

use super::*;

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
    assert_eq!(
        env.events().all().len(),
        prev,
        "unauthorized upsert must not emit events"
    );
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "unauthorized remove must not emit events"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #200)")]
fn remove_anchor_emits_no_event_when_missing() {
    let (env, client, admins) = setup();
    let missing_asset = Address::generate(&env);

    let prev = env.events().all().len();
    client.remove_anchor(&admins, &missing_asset);
    assert_eq!(
        env.events().all().len(),
        prev,
        "remove missing anchor must not emit events"
    );
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "upsert while paused must not emit events"
    );
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "remove while paused must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "set_fee_config while paused must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "set_fee_config unauthorized must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "set_fee_config with invalid bps must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "set_fee_config with below-min bps must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "update_system_param with negative value must not emit events"
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
    assert_eq!(
        env.events().all().len(),
        prev,
        "unauthorized update_system_param must not emit events"
    );
}

// ---------------------------------------------------------------------------
// Change-threshold error paths — must emit zero events
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn change_threshold_emits_no_event_when_insufficient_signatures() {
    // Manual setup: 3 admins, threshold 2.  change_threshold requires 3 signers
    // (current_threshold + 1), but we only provide 1 → Unauthorized (#3).
    // The new_threshold (2) is within bounds so the bound check passes first.
    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let recovery = Address::generate(&env);
    let deployer = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    client.init(&deployer, &admins, &2, &recovery);

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
#[should_panic(expected = "Error(Contract, #202)")]
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
#[should_panic(expected = "Error(Contract, #203)")]
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
