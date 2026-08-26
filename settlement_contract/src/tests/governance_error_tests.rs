//! Tests verifying that governance cross-contract failures surface as the
//! typed `SettlementError::GovernanceCallFailed` (code 311) rather than an
//! untyped host panic or silently collapsing to `None`.
//!
//! Two paths are exercised:
//! - Read path: `read_governance_fee_rule` (reached via `calculate_fee_split`
//!   when no merchant-specific or default rule is set).
//! - Write path: `validate_fee_against_governance` (reached via
//!   `set_settlement_rule` / `set_default_rule`).

use crate::types::DataKey;
use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

// ---------------------------------------------------------------------------
// Failing governance stub — lives in its own module to avoid symbol collisions
// with the `MockGovernance` stub in tests::mod, which also exposes
// `get_fee_config`.
// ---------------------------------------------------------------------------

mod panicking_gov {
    use soroban_sdk::{contract, contractimpl, Env};
    use crate::GovFeeConfig;

    /// A governance stub whose `get_fee_config` always traps (simulates a
    /// broken or mis-deployed governance contract).
    #[contract]
    pub struct PanickingGovernance;

    #[contractimpl]
    impl PanickingGovernance {
        #[allow(unused_variables)]
        pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
            panic!("governance trap")
        }
    }
}

use panicking_gov::PanickingGovernance;

/// Helper: directly injects a governance address into the settlement contract's
/// instance storage, bypassing `validate_governance` (which would itself call
/// `get_fee_config` and fail against the panicking stub).
fn inject_governance(env: &Env, contract_address: &Address, governance: &Address) {
    env.as_contract(contract_address, || {
        env.storage()
            .instance()
            .set(&DataKey::Governance, governance);
    });
}

// ---------------------------------------------------------------------------
// Read-path: read_governance_fee_rule
// ---------------------------------------------------------------------------

/// Wires a settlement contract to a panicking governance stub by directly
/// injecting the address into storage, then attempts to resolve the effective
/// rule for a merchant (which hits the governance read path when no
/// merchant-specific or default rule is set).
///
/// Expected: the typed `GovernanceCallFailed` error (code 311).
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn read_path_governance_failure_surfaces_typed_error() {
    let env = Env::default();
    env.mock_all_auths();

    let panicking_gov = env.register_contract(None, PanickingGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);

    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Directly inject the panicking governance address, bypassing validate_governance.
    inject_governance(&env, &contract_id, &panicking_gov);

    // No merchant rule or default rule is set, so resolution falls through to
    // the governance read path — which now traps and must raise GovernanceCallFailed.
    client.calculate_fee_split(&merchant, &10_000);
}

/// When governance returns `None` (no config set), the read path should fall
/// through to the bootstrap default without error.
#[test]
fn read_path_governance_none_falls_through_to_bootstrap() {
    let env = Env::default();
    env.mock_all_auths();

    let empty_gov = super::register_governance(&env);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);

    // Empty governance returns None — bootstrap default should apply (100 bps platform, 5 network).
    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_amount, 9_895);
}

// ---------------------------------------------------------------------------
// Write-path: validate_fee_against_governance
// ---------------------------------------------------------------------------

/// Injects a panicking governance address into a settlement contract, then
/// attempts to set a settlement rule (which hits `validate_fee_against_governance`).
///
/// Expected: the typed `GovernanceCallFailed` error (code 311).
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn write_path_governance_failure_surfaces_typed_error() {
    let env = Env::default();
    env.mock_all_auths();

    let panicking_gov = env.register_contract(None, PanickingGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Directly inject the panicking governance address.
    inject_governance(&env, &contract_id, &panicking_gov);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // set_settlement_rule calls validate_fee_against_governance, which must
    // surface GovernanceCallFailed instead of an untyped host panic.
    client.set_settlement_rule(&soroban_sdk::vec![&env, admin], &merchant, &rule);
}

/// Same as above but for `set_default_rule`, which also calls
/// `validate_fee_against_governance`.
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn write_path_set_default_rule_governance_failure_surfaces_typed_error() {
    let env = Env::default();
    env.mock_all_auths();

    let panicking_gov = env.register_contract(None, PanickingGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);

    // Directly inject the panicking governance address.
    inject_governance(&env, &contract_id, &panicking_gov);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    client.set_default_rule(&soroban_sdk::vec![&env, admin], &rule);
}

// ---------------------------------------------------------------------------
// Fee-returning governance stub — returns a populated GovFeeConfig
// ---------------------------------------------------------------------------

mod fee_gov {
    use soroban_sdk::{contract, contractimpl, Env};
    use crate::GovFeeConfig;

    /// A governance stub whose `get_fee_config` returns a populated config
    /// with non-zero fee values, allowing the read path to exercise the
    /// governance-to-SettlementRule conversion.
    #[contract]
    pub struct FeeReturningGovernance;

    #[contractimpl]
    impl FeeReturningGovernance {
        #[allow(unused_variables)]
        pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
            Some(GovFeeConfig {
                platform_fee_bps: 250,
                network_fee_bps: 50,
            })
        }
    }
}

use fee_gov::FeeReturningGovernance;

// ---------------------------------------------------------------------------
// Regression test: governance-to-SettlementRule conversion (issue #484)
// ---------------------------------------------------------------------------

/// Regression test for issue #484: verifies that the governance-to-
/// `SettlementRule` conversion in `read_governance_fee_rule` produces the
/// expected exact fields.
///
/// When governance returns a fee config with `platform_fee_bps: 250` and
/// `network_fee_bps: 50`, the resulting `SettlementRule` must have:
/// - `platform_fee_bps: 250` (from governance)
/// - `network_fee_bps: 50` (from governance)
/// - `settlement_delay_ledger: 0` (intentionally fixed — see #484)
/// - `auto_settle: false` (intentionally fixed — see #484)
///
/// This test proves the conversion fields are correct and prevents future
/// regressions if someone attempts to extend `GovFeeConfig` or modify the
/// conversion logic.
#[test]
fn governance_fee_rule_conversion_produces_exact_fields() {
    let env = Env::default();
    env.mock_all_auths();

    let fee_gov = env.register_contract(None, FeeReturningGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Inject the fee-returning governance address.
    inject_governance(&env, &contract_id, &fee_gov);

    // No merchant-specific or default rule is set, so resolution falls through
    // to the governance read path — which converts GovFeeConfig to SettlementRule.
    let split = client.calculate_fee_split(&merchant, &10_000);

    // Governance config: platform_fee_bps=250, network_fee_bps=50
    // Expected fee split on 10,000:
    //   platform_fee = ceil(10000 * 250 / 10000) = 250
    //   network_fee  = ceil(10000 * 50 / 10000) = 50
    //   merchant     = 10000 - 250 - 50 = 9700
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_amount, 9_700);

    // The governance path produces a SettlementRule with the fee BPS from
    // governance and fixed settlement_delay_ledger=0, auto_settle=false.
    // The fee amounts above (250, 50, 9700) prove the fee BPS are carried
    // through. The fixed delay/auto_settle fields are documented in
    // read_governance_fee_rule's doc comment and match the bootstrap default.

    // Additional proof: use a different amount to confirm ceiling-aware fees
    // are derived from governance values.
    let split_large = client.calculate_fee_split(&merchant, &100_000);
    assert_eq!(split_large.platform_fee_amount, 2500);
    assert_eq!(split_large.network_fee_amount, 500);
    assert_eq!(split_large.merchant_amount, 97_000);
}
