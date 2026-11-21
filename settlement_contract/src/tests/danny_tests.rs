//! Tests for issues #1110, #1112, and #1113:
//! - Guard update_recovery_address against setting same address or during active recovery (#1110)
//! - Check governance_bypass_enabled in validate_fee_against_governance (#1112)
//! - Instance TTL extension in cancel_recovery and execute_recovery (#1113)

use super::setup;
use crate::errors::SettlementError;
use crate::policy::{READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD};
use crate::types::SettlementRule;
use bettapay_common::constants::{BYPASS_GOV_FEES_PARAM, RECOVERY_DELAY_SECONDS};
use soroban_sdk::testutils::storage::Instance as _;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Symbol};

#[test]
fn update_recovery_address_rejects_same_address_and_active_recovery() {
    let (env, client, admins, _merchant) = setup();
    let current_recovery = client.get_recovery_address();

    // 1. Setting new_recovery equal to current_recovery fails with InvalidRecoveryAddress
    let res_same = client.try_update_recovery_address(&admins, &current_recovery);
    assert_eq!(
        res_same.unwrap_err().unwrap(),
        SettlementError::InvalidRecoveryAddress.into()
    );

    // 2. Setting new_recovery while recovery is pending fails with RecoveryDelayActive
    let new_recovery1 = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initiate_recovery(&new_admin);

    let res_pending = client.try_update_recovery_address(&admins, &new_recovery1);
    assert_eq!(
        res_pending.unwrap_err().unwrap(),
        SettlementError::RecoveryDelayActive.into()
    );

    // Cancel recovery allows updating recovery address
    client.cancel_recovery(&admins);
    client.update_recovery_address(&admins, &new_recovery1);
    assert_eq!(client.get_recovery_address(), new_recovery1);
}

#[test]
fn set_settlement_rule_honors_governance_bypass_fees() {
    let (env, client, admins, merchant) = setup();
    let gov_id = client.get_governance();
    let gov_client = governance_contract::GovernanceContractClient::new(&env, &gov_id);

    // Configure governance fee ceiling low (e.g. 500 bps)
    let gov_admins = gov_client.get_admin();
    gov_client.set_fee_config(
        &gov_admins,
        &governance_contract::FeeConfig {
            platform_fee_bps: 500,
            network_fee_bps: 500,
        },
    );

    let rule = SettlementRule {
        platform_fee_bps: 1_000,
        network_fee_bps: 1_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // Before bypass: setting 1000 bps fails against 500 bps governance ceiling
    let res = client.try_set_settlement_rule(&admins, &merchant, &rule);
    assert_eq!(
        res.unwrap_err().unwrap(),
        SettlementError::FeeExceedsGovernanceConfig.into()
    );

    // Enable emergency bypass in governance: bypass_gov_fees = 1
    gov_client.set_system_param(
        &gov_admins,
        &Symbol::new(&env, BYPASS_GOV_FEES_PARAM),
        &1i128,
    );

    // With bypass enabled: ceiling check is skipped, local rule is accepted
    client.set_settlement_rule(&admins, &merchant, &rule);
    assert_eq!(client.get_settlement_rule(&merchant), rule);

    // Invalid local rule bounds (> MAX_FEE_BPS) are still rejected
    let invalid_rule = SettlementRule {
        platform_fee_bps: 20_000,
        network_fee_bps: 1_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let res_invalid = client.try_set_settlement_rule(&admins, &merchant, &invalid_rule);
    assert_eq!(
        res_invalid.unwrap_err().unwrap(),
        SettlementError::InvalidFeeBps.into()
    );
}

#[test]
fn execute_and_cancel_recovery_extend_instance_ttl() {
    let (env, client, admins, _merchant) = setup();
    let contract_address = client.address.clone();

    // 1. Test cancel_recovery extends instance TTL
    let new_admin = Address::generate(&env);
    client.initiate_recovery(&new_admin);

    // Decay instance TTL
    env.as_contract(&contract_address, || {
        env.storage()
            .instance()
            .extend_ttl(READ_INSTANCE_TTL_THRESHOLD / 2, READ_INSTANCE_TTL_THRESHOLD / 2);
    });

    client.cancel_recovery(&admins);

    let ttl_after_cancel = env.as_contract(&contract_address, || {
        env.storage().instance().get_ttl()
    });
    assert_eq!(ttl_after_cancel, READ_INSTANCE_TTL_BUMP);

    // 2. Test execute_recovery extends instance TTL and new admin survives
    client.initiate_recovery(&new_admin);
    env.ledger().with_mut(|l| {
        l.timestamp += RECOVERY_DELAY_SECONDS;
    });

    // Decay instance TTL before executing recovery
    env.as_contract(&contract_address, || {
        env.storage()
            .instance()
            .extend_ttl(READ_INSTANCE_TTL_THRESHOLD / 2, READ_INSTANCE_TTL_THRESHOLD / 2);
    });

    client.execute_recovery();

    let ttl_after_execute = env.as_contract(&contract_address, || {
        env.storage().instance().get_ttl()
    });
    assert_eq!(ttl_after_execute, READ_INSTANCE_TTL_BUMP);

    // Verify new admin state is active and survives
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
    assert_eq!(client.get_threshold(), 1);
}
