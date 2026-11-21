//! Tests for issues #1108, #1109, and #1111:
//! - clear_settlement_rule zero-address validation
//! - register_merchant storage check before auth
//! - get_deployer read-only getter

use super::setup;
use crate::errors::SettlementError;
use crate::SettlementContract;
use bettapay_common::storage::zero_address;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

#[test]
fn clear_settlement_rule_rejects_zero_address() {
    let (env, client, admins, _merchant) = setup();
    let zero = zero_address(&env);
    let res = client.try_clear_settlement_rule(&admins, &zero);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::ZeroAddress.into()
    );
}

#[test]
fn clear_settlement_rule_returns_merchant_rule_not_set_for_non_zero_without_rule() {
    let (_env, client, admins, merchant) = setup();
    let res = client.try_clear_settlement_rule(&admins, &merchant);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::MerchantRuleNotSet.into()
    );
}

#[test]
fn register_merchant_fails_duplicate_without_merchant_auth() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // Duplicate registration returns MerchantExists immediately
    let res = client.try_register_merchant(&admins, &merchant);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::MerchantExists.into()
    );
}

#[test]
fn settlement_get_deployer_returns_deployer_address() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = crate::SettlementContractClient::new(&env, &contract_id);

    // Before init, get_deployer returns NotInitialized
    assert_eq!(
        client.try_get_deployer().unwrap_err().unwrap(),
        SettlementError::NotInitialized.into()
    );

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin];
    let gov = Address::generate(&env);
    let recovery = Address::generate(&env);
    client.init(&deployer, &admins, &1u32, &gov, &recovery);

    assert_eq!(client.get_deployer(), deployer);
}

#[test]
fn read_schema_version_defaults_to_zero_and_migrate_advances() {
    use crate::types::DataKey;
    use crate::CURRENT_SCHEMA_VERSION;

    let (env, client, admins, _merchant) = setup();

    // Remove SchemaVersion marker to simulate missing schema version on an unmigrated contract
    env.as_contract(&client.address, || {
        env.storage().instance().remove(&DataKey::SchemaVersion);
    });

    // When marker is absent, read_schema_version returns 0
    env.as_contract(&client.address, || {
        assert_eq!(crate::storage::read_schema_version(&env), 0);
    });

    // Calling migrate advances schema version to CURRENT_SCHEMA_VERSION
    client.migrate(&admins);

    env.as_contract(&client.address, || {
        assert_eq!(
            crate::storage::read_schema_version(&env),
            CURRENT_SCHEMA_VERSION
        );
    });
}
