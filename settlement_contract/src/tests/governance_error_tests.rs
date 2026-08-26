//! Tests verifying that governance cross-contract failures surface as the
//! typed `SettlementError::GovernanceCallFailed` (code 311) rather than an
//! untyped host panic or silently collapsing to `None`.
//!
//! Two paths are exercised:
//! - Read path: `read_governance_fee_rule` (reached via `calculate_fee_split`
//!   when no merchant-specific or default rule is set).
//! - Write path: `validate_fee_against_governance` (reached via
//!   `set_settlement_rule` / `set_default_rule`).
//!
//! Issue #483: Malformed governance configs (e.g. a 1-field config that omits
//! `network_fee_bps`) must be rejected rather than silently skipping the
//! network-fee ceiling.

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

// ---------------------------------------------------------------------------
// Malformed governance stub — returns a 1-field struct where GovFeeConfig
// expects 2 fields (issue #483).
// ---------------------------------------------------------------------------

mod malformed_gov {
    use soroban_sdk::{contract, contractimpl, contracttype, Env};

    /// A 1-field config struct: only `platform_fee_bps`, missing
    /// `network_fee_bps`. When the settlement contract attempts to
    /// deserialize this into `Option<GovFeeConfig>`, the missing field
    /// causes a deserialization failure that must surface as
    /// `GovernanceCallFailed`.
    #[derive(Clone)]
    #[contracttype]
    pub struct OneFieldFeeConfig {
        pub platform_fee_bps: u32,
    }

    #[contract]
    pub struct MalformedGovernance;

    #[contractimpl]
    impl MalformedGovernance {
        /// Returns a 1-field config where the settlement contract expects
        /// 2 fields (platform_fee_bps + network_fee_bps). This simulates
        /// a governance contract upgrade that forgot the network fee field.
        pub fn get_fee_config(_env: Env) -> Option<OneFieldFeeConfig> {
            Some(OneFieldFeeConfig {
                platform_fee_bps: 200,
            })
        }
    }
}

use malformed_gov::MalformedGovernance;

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
// Issue #483: Malformed governance config (1-field, missing network_fee_bps)
// ---------------------------------------------------------------------------

/// Governance stub that returns a 1-field config struct (only
/// `platform_fee_bps`). The settlement contract expects `Option<GovFeeConfig>`
/// with 2 fields (`platform_fee_bps` + `network_fee_bps`).
///
/// The cross-contract deserialization of a mismatched struct shape must fail,
/// surfacing as `GovernanceCallFailed` (code 311) rather than silently
/// accepting the config and skipping the network-fee ceiling.
///
/// This test proves issue #483: a 1-field governance config is rejected.
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn malformed_one_field_config_rejected_write_path() {
    let env = Env::default();
    env.mock_all_auths();

    let malformed_gov = env.register_contract(None, MalformedGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Inject governance that returns a 1-field config.
    inject_governance(&env, &contract_id, &malformed_gov);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // set_settlement_rule -> validate_fee_against_governance -> get_fee_config.
    // The 1-field response cannot deserialize into GovFeeConfig (2 fields),
    // so GovernanceCallFailed must be raised — the network-fee ceiling must
    // NOT be silently skipped.
    client.set_settlement_rule(&soroban_sdk::vec![&env, admin], &merchant, &rule);
}

/// Same malformed 1-field config exercised through `set_default_rule` to
/// confirm the read-through-validate path also rejects it.
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn malformed_one_field_config_rejected_default_rule() {
    let env = Env::default();
    env.mock_all_auths();

    let malformed_gov = env.register_contract(None, MalformedGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);

    // Inject governance that returns a 1-field config.
    inject_governance(&env, &contract_id, &malformed_gov);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // set_default_rule -> validate_fee_against_governance -> get_fee_config.
    // Must surface GovernanceCallFailed for malformed 1-field config.
    client.set_default_rule(&soroban_sdk::vec![&env, admin], &rule);
}

/// Malformed 1-field config also rejected on the read path (calculate_fee_split
/// falls through to read_governance_fee_rule when no merchant/default rule is set).
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn malformed_one_field_config_rejected_read_path() {
    let env = Env::default();
    env.mock_all_auths();

    let malformed_gov = env.register_contract(None, MalformedGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&soroban_sdk::vec![&env, admin.clone()], &1, &empty_gov, &recovery);
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Inject governance that returns a 1-field config.
    inject_governance(&env, &contract_id, &malformed_gov);

    // No rule set → resolution falls through to governance read path.
    // The 1-field response fails deserialization → GovernanceCallFailed.
    client.calculate_fee_split(&merchant, &10_000);
}
