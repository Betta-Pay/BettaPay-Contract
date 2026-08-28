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
    use crate::GovFeeConfig;
    use soroban_sdk::{contract, contractimpl, Env};

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

// A second failing-governance stub that returns a *typed error* rather than
// trapping. This exercises the `Ok(Err(_))` branch of `try_invoke_contract`
// (a contract that deliberately rejects the read), as opposed to the trap
// branch exercised by `PanickingGovernance`.
mod erroring_gov {
    use crate::errors::SettlementError;
    use crate::GovFeeConfig;
    use soroban_sdk::{contract, contractimpl, Env};

    /// A governance stub whose `get_fee_config` returns a typed error.
    #[contract]
    pub struct ErroringGovernance;

    #[contractimpl]
    impl ErroringGovernance {
        #[allow(unused_variables)]
        pub fn get_fee_config(env: Env) -> Result<Option<GovFeeConfig>, SettlementError> {
            Err(SettlementError::GovernanceCallFailed)
        }
    }
}

use erroring_gov::ErroringGovernance;

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
    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );

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
    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
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
    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
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
    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );

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

/// Focused variant of the write-path test: governance returns a typed error
/// (not a trap). This drives `validate_fee_against_governance` through the
/// `Ok(Err(_))` branch of `try_invoke_contract`.
///
/// Expected: the typed `GovernanceCallFailed` error (code 311) — a deliberate
/// governance rejection must not surface as an untyped host panic.
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn write_path_governance_error_surfaces_typed_error() {
    let env = Env::default();
    env.mock_all_auths();

    let erroring_gov = env.register_contract(None, ErroringGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Directly inject the error-returning governance address.
    inject_governance(&env, &contract_id, &erroring_gov);

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
