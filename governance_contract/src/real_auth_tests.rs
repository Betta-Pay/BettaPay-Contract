use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Bytes, Env, Symbol};

fn valid_fee_config() -> FeeConfig {
    FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    }
}

#[test]
fn fee_anchor_and_system_param_writes_require_real_authorization() {
    let (env, client, admins) = super::setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    let key = Symbol::new(&env, "real_auth");
    env.mock_auths(&[]);

    assert!(client.try_set_fee_config(&admins, &valid_fee_config()).is_err());
    assert!(client.try_upsert_anchor(&admins, &asset, &anchor).is_err());
    assert!(client.try_update_system_param(&admins, &key, &1).is_err());
}

#[test]
fn anchor_removal_requires_real_authorization() {
    let (env, client, admins) = super::setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admins, &asset, &anchor);
    env.mock_auths(&[]);

    assert!(client.try_remove_anchor(&admins, &asset).is_err());
}

#[test]
fn admin_transfer_and_threshold_change_require_real_authorization() {
    let (env, client, admins) = super::setup();
    let replacement_admin = Address::generate(&env);
    env.mock_auths(&[]);

    assert!(client
        .try_transfer_admin(&admins, &soroban_sdk::vec![&env, replacement_admin], &1,)
        .is_err());

    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, a1, a2, a3];
    let recovery = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    client.init(&admins, &1, &recovery);
    env.mock_auths(&[]);

    assert!(client.try_change_threshold(&admins, &2).is_err());
}

#[test]
fn pause_and_unpause_require_real_authorization() {
    let (env, client, admins) = super::setup();
    env.mock_auths(&[]);

    assert!(client.try_pause(&admins).is_err());

    let (env, client, admins) = super::setup();
    client.pause(&admins);
    env.mock_auths(&[]);
    assert!(client.try_unpause(&admins).is_err());
}

#[test]
fn recovery_initiation_and_cancellation_require_real_authorization() {
    let (env, client, _admins) = super::setup();
    let target = Address::generate(&env);
    env.mock_auths(&[]);
    assert!(client.try_initiate_recovery(&target).is_err());

    let (env, client, admins) = super::setup();
    let target = Address::generate(&env);
    client.initiate_recovery(&target);
    env.mock_auths(&[]);
    assert!(client.try_cancel_recovery(&admins).is_err());
}

#[test]
fn initialization_and_upgrade_require_real_authorization() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    env.mock_auths(&[]);
    assert!(client.try_init(&admins, &1, &recovery).is_err());

    let (env, client, admins) = super::setup();
    let wasm_hash = env
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(&env, &[]));
    env.mock_auths(&[]);
    assert!(client.try_upgrade(&admins, &wasm_hash).is_err());
}
