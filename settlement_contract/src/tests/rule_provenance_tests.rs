//! Issue #997: the effective rule reports which source of the resolution chain
//! (merchant → default → governance → bootstrap) produced the fees, without
//! changing any fee value.

use super::{
    assert_rule_eq, register_governance_with_config, register_governance_with_mode, setup,
    MockGovMode,
};
use crate::types::DataKey;
use crate::{
    GovFeeConfig, RuleProvenance, SettlementContract, SettlementContractClient, SettlementRule,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

fn rule(platform_fee_bps: u32, network_fee_bps: u32) -> SettlementRule {
    SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger: 0,
        auto_settle: false,
    }
}

/// Registers a settlement contract wired to `governance` with one registered merchant.
fn setup_with_governance(
    governance_for: impl FnOnce(&Env) -> Address,
) -> (
    Env,
    SettlementContractClient<'static>,
    soroban_sdk::Vec<Address>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    let governance = governance_for(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &admins,
        &1,
        &governance,
        &Address::generate(&env),
    );
    let merchant = Address::generate(&env);
    client.register_merchant(&admins, &merchant);
    (env, client, admins, merchant)
}

#[test]
fn merchant_rule_reports_merchant_provenance() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    // A default rule exists too, but the merchant-specific rule must win.
    client.set_default_rule(&admins, &rule(300, 30));
    client.set_settlement_rule(&admins, &merchant, &rule(200, 20));

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Merchant);
    assert_rule_eq(&effective.rule, &rule(200, 20), "merchant rule values");
}

#[test]
fn default_rule_reports_default_provenance() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    client.set_default_rule(&admins, &rule(300, 30));

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Default);
    assert_rule_eq(&effective.rule, &rule(300, 30), "default rule values");
}

#[test]
fn governance_config_reports_governance_provenance() {
    let (_env, client, _admins, merchant) = setup_with_governance(|env| {
        register_governance_with_config(
            env,
            GovFeeConfig {
                platform_fee_bps: 150,
                network_fee_bps: 25,
            },
        )
    });

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Governance);
    assert_rule_eq(&effective.rule, &rule(150, 25), "governance rule values");
}

#[test]
fn governance_config_equal_to_bootstrap_values_is_still_governance() {
    // Provenance tracks the source, not the values: governance returning the
    // exact bootstrap numbers must not be mistaken for the bootstrap fallback.
    let bootstrap = crate::policy::BOOTSTRAP_DEFAULT_RULE;
    let (_env, client, _admins, merchant) = setup_with_governance(|env| {
        register_governance_with_config(
            env,
            GovFeeConfig {
                platform_fee_bps: bootstrap.platform_fee_bps,
                network_fee_bps: bootstrap.network_fee_bps,
            },
        )
    });

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Governance);
    assert_rule_eq(&effective.rule, &bootstrap, "values match bootstrap");
}

#[test]
fn cached_governance_rule_during_outage_still_reports_governance() {
    let (env, client, _admins, merchant) = setup_with_governance(|env| {
        register_governance_with_config(
            env,
            GovFeeConfig {
                platform_fee_bps: 150,
                network_fee_bps: 25,
            },
        )
    });
    // Prime the last-good governance cache.
    assert_eq!(
        client.get_rule_provenance(&merchant).provenance,
        RuleProvenance::Governance
    );

    // Governance now traps; the cached governance rule is served.
    let trapping = register_governance_with_mode(&env, MockGovMode::Trap);
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&DataKey::Governance, &trapping);
    });

    let effective = client.get_rule_provenance(&merchant);
    assert_eq!(effective.provenance, RuleProvenance::Governance);
    assert_rule_eq(&effective.rule, &rule(150, 25), "cached governance values");
}

#[test]
fn unconfigured_deployment_reports_bootstrap_provenance() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Bootstrap);
    assert_rule_eq(
        &effective.rule,
        &crate::policy::BOOTSTRAP_DEFAULT_RULE,
        "bootstrap rule values",
    );
}

#[test]
fn broken_governance_with_no_cache_reports_bootstrap_provenance() {
    let (_env, client, _admins, merchant) =
        setup_with_governance(|env| register_governance_with_mode(env, MockGovMode::Trap));

    let effective = client.get_rule_provenance(&merchant);

    assert_eq!(effective.provenance, RuleProvenance::Bootstrap);
}

#[test]
fn all_four_provenance_values_are_distinct() {
    let all = [
        RuleProvenance::Merchant,
        RuleProvenance::Default,
        RuleProvenance::Governance,
        RuleProvenance::Bootstrap,
    ];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            assert_eq!(i == j, a == b);
        }
    }
}

#[test]
fn provenance_getter_rule_matches_get_effective_rule() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    client.set_default_rule(&admins, &rule(300, 30));

    assert_rule_eq(
        &client.get_rule_provenance(&merchant).rule,
        &client.get_effective_rule(&merchant),
        "provenance must not change fee values",
    );
}
