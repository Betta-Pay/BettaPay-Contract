//! Fee bounds parity between governance and settlement — issue #987.
//!
//! Governance's `FeeConfig` (`set_fee_config`) and settlement's
//! `SettlementRule` (`set_settlement_rule`, `set_default_rule`) both take
//! `MIN_FEE_BPS` / `MAX_FEE_BPS` from `bettapay_common::constants`. These
//! tests drive the same boundary values through every entry point and assert
//! the three agree, so governance can never accept a config settlement would
//! reject (or the reverse).

use bettapay_common::constants::{MAX_FEE_BPS, MIN_FEE_BPS};
use bettapay_common::error_codes::INVALID_FEE_BPS;
use governance_contract::{FeeConfig, GovernanceContract, GovernanceContractClient};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Vec};

use super::setup;
use crate::types::SettlementRule;

/// `(platform_fee_bps, network_fee_bps, expected_accept)` for every boundary.
/// Each leg is pushed through both edges independently while the other leg
/// sits at a valid value, so a bound enforced on only one leg is caught.
fn boundary_cases() -> [(u32, u32, bool); 8] {
    let mid = 250;
    [
        (MIN_FEE_BPS - 1, mid, false),
        (MIN_FEE_BPS, mid, true),
        (MAX_FEE_BPS, mid, true),
        (MAX_FEE_BPS + 1, mid, false),
        (mid, MIN_FEE_BPS - 1, false),
        (mid, MIN_FEE_BPS, true),
        (mid, MAX_FEE_BPS, true),
        (mid, MAX_FEE_BPS + 1, false),
    ]
}

fn rule(platform_fee_bps: u32, network_fee_bps: u32) -> SettlementRule {
    SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger: 0,
        auto_settle: false,
    }
}

/// Registers an initialised governance contract; returns its client and admins.
fn governance(env: &Env) -> (GovernanceContractClient<'static>, Vec<Address>) {
    let id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(env, &id);
    let admins = soroban_sdk::vec![env, Address::generate(env)];
    client.init(
        &Address::generate(env),
        &admins,
        &1,
        &Address::generate(env),
    );
    (client, admins)
}

/// Outcome of a `try_*` call: `Ok(())` if accepted, `Err(code)` with the
/// contract error code if rejected. Any other failure fails the test.
fn outcome<T, C: core::fmt::Debug, I: core::fmt::Debug>(
    result: Result<Result<T, C>, Result<soroban_sdk::Error, I>>,
) -> Result<(), u32> {
    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => panic!("unexpected conversion failure: {e:?}"),
        Err(Ok(e)) => {
            if e.is_type(soroban_sdk::xdr::ScErrorType::Contract) {
                Err(e.get_code())
            } else {
                panic!("rejection must be a contract error: {e:?}")
            }
        }
        Err(Err(e)) => panic!("unexpected invocation failure: {e:?}"),
    }
}

fn governance_outcome(platform: u32, network: u32) -> Result<(), u32> {
    let env = Env::default();
    env.mock_all_auths();
    let (gov, admins) = governance(&env);
    outcome(gov.try_set_fee_config(
        &admins,
        &FeeConfig {
            platform_fee_bps: platform,
            network_fee_bps: network,
        },
    ))
}

fn settlement_rule_outcome(platform: u32, network: u32) -> Result<(), u32> {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    outcome(client.try_set_settlement_rule(&admins, &merchant, &rule(platform, network)))
}

fn settlement_default_outcome(platform: u32, network: u32) -> Result<(), u32> {
    let (_env, client, admins, _merchant) = setup();
    outcome(client.try_set_default_rule(&admins, &rule(platform, network)))
}

/// Both contracts reject `MIN_FEE_BPS - 1` and `MAX_FEE_BPS + 1` with
/// `InvalidFeeBps`, and accept `MIN_FEE_BPS` and `MAX_FEE_BPS`.
#[test]
fn governance_and_settlement_enforce_the_same_bounds() {
    for (platform, network, accept) in boundary_cases() {
        let expected = if accept { Ok(()) } else { Err(INVALID_FEE_BPS) };

        assert_eq!(
            governance_outcome(platform, network),
            expected,
            "set_fee_config({platform}, {network})",
        );
        assert_eq!(
            settlement_rule_outcome(platform, network),
            expected,
            "set_settlement_rule({platform}, {network})",
        );
        assert_eq!(
            settlement_default_outcome(platform, network),
            expected,
            "set_default_rule({platform}, {network})",
        );
    }
}

/// Parity itself: for every boundary value the governance verdict equals the
/// settlement verdict, whatever that verdict is.
#[test]
fn governance_and_settlement_verdicts_match_at_each_boundary() {
    for (platform, network, _) in boundary_cases() {
        let gov = governance_outcome(platform, network);
        assert_eq!(
            gov,
            settlement_rule_outcome(platform, network),
            "governance vs set_settlement_rule at ({platform}, {network})",
        );
        assert_eq!(
            gov,
            settlement_default_outcome(platform, network),
            "governance vs set_default_rule at ({platform}, {network})",
        );
    }
}
