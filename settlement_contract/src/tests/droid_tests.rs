//! Tests for issues #1125, #1128, and #1130:
//! - get_capabilities matching CAPABILITY_FLAGS (#1125)
//! - SettlementContract::migrate idempotency and paused rejection (#1128)
//! - store_payment_reference rejecting all-zero reference before auth and when paused (#1130)

use super::setup;
use crate::errors::SettlementError;
use crate::policy::CAPABILITY_FLAGS;
use crate::CURRENT_SCHEMA_VERSION;
use bettapay_common::events::MIGRATED_EVENT;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol};

#[test]
fn get_capabilities_matches_capability_flags_exactly() {
    let env = Env::default();
    let contract_id = env.register_contract(None, crate::SettlementContract);
    let client = crate::SettlementContractClient::new(&env, &contract_id);

    // Callable without initialization or auth
    let capabilities = client.get_capabilities();

    // Verify lengths match exactly
    assert_eq!(capabilities.len(), CAPABILITY_FLAGS.len() as u32);

    // Verify every capability flag maps to true
    for flag in CAPABILITY_FLAGS.iter() {
        let key = Symbol::new(&env, flag);
        assert_eq!(
            capabilities.get(key),
            Some(true),
            "Capability flag '{}' must be present and set to true",
            flag
        );
    }
}

#[test]
fn settlement_migrate_idempotency_and_paused_rejection() {
    let (env, client, admins, _merchant) = setup();

    // 1. Paused contract rejects migrate with Paused
    client.pause(&admins, &0);
    let res_paused = client.try_migrate(&admins);
    assert_eq!(
        res_paused.unwrap_err().unwrap(),
        SettlementError::Paused.into()
    );
    client.unpause(&admins);

    // 2. Unpaused migrate succeeds and emits MIGRATED_EVENT
    let events_before_first = env.events().all().len();
    client.migrate(&admins);
    assert_eq!(env.events().all().len(), events_before_first + 1);

    let (_, topics, data) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, MIGRATED_EVENT)
    );
    let (admin, schema_ver): (Address, u32) = FromVal::from_val(&env, &data);
    assert_eq!(admin, admins.get(0).unwrap());
    assert_eq!(schema_ver, CURRENT_SCHEMA_VERSION);

    // 3. Repeated call is idempotent and succeeds
    let events_before_second = env.events().all().len();
    client.migrate(&admins);
    assert_eq!(env.events().all().len(), events_before_second + 1);
}

#[test]
fn store_payment_reference_rejects_zero_reference_before_auth_and_paused() {
    let env = Env::default();
    let contract_id = env.register_contract(None, crate::SettlementContract);
    let client = crate::SettlementContractClient::new(&env, &contract_id);

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin];
    let gov = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    env.mock_all_auths();
    client.init(&deployer, &admins, &1u32, &gov, &recovery);
    client.register_merchant(&admins, &merchant);

    // Pause contract
    client.pause(&admins, &0);

    // Even while paused, all-zero reference returns InvalidPaymentReference
    let zero_ref = BytesN::from_array(&env, &[0u8; 32]);
    let res = client.try_store_payment_reference(&merchant, &zero_ref, &1000);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::InvalidPaymentReference.into()
    );
}
