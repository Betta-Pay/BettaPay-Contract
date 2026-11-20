//! Tests for governance cross-contract failure handling.
//!
//! Two paths are exercised, and they deliberately differ (issues #741/#742):
//! - Read path: `read_governance_fee_rule` (reached via `calculate_fee_split`
//!   when no merchant-specific or default rule is set). A trap or typed error
//!   must **degrade** to the bootstrap default so fallback payments keep
//!   working (issue #741).
//! - Write path: `validate_fee_against_governance` (reached via
//!   `set_settlement_rule` / `set_default_rule`). A trap must **hard-fail**
//!   with the typed `SettlementError::GovernanceCallFailed` (issue #742), so
//!   admin writes never silently accept stale ceilings.
//!
//! Issue #743 adds the `bypass_gov_fees` circuit-breaker: when governance sets
//! it to 1, the read path skips `get_fee_config` entirely.
//!
//! Issue #483: Malformed governance configs (e.g. a 1-field config that omits
//! `network_fee_bps`) must be rejected rather than silently skipping the
//! network-fee ceiling.

use crate::types::DataKey;
use crate::*;
use governance_contract::{
    FeeConfig as GovFeeConfig, GovernanceContract, GovernanceContractClient,
};
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{Address, Env, FromVal, Symbol};

// ---------------------------------------------------------------------------
// Failing governance stub — lives in its own module to avoid symbol collisions
// with the `MockGovernance` stub in tests::mod, which also exposes
// `get_fee_config`.
// ---------------------------------------------------------------------------

pub mod panicking_gov {
    pub use super::super::mock_governance::{
        register_governance_with_mode, MockGovMode, MockGovernance as PanickingGovernance,
    };
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
/// injecting the address into storage, then resolves the effective rule for a
/// merchant (which hits the governance read path when no merchant-specific or
/// default rule is set).
///
/// Issue #741: the trap must degrade to the bootstrap default, not brick the
/// payment path with `GovernanceCallFailed`.
#[test]
fn read_path_governance_trap_falls_back_to_bootstrap() {
    let env = Env::default();
    env.mock_all_auths();

    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );

    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Directly inject the panicking governance address, bypassing validate_governance.
    inject_governance(&env, &contract_id, &panicking_gov);

    // No merchant rule or default rule is set, so resolution falls through to
    // the governance read path. Governance traps; the payment still succeeds
    // against the bootstrap default (100 bps platform, 5 bps network).
    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_payout_amount, 9_895);
}

/// Same as above but governance returns a *typed error* rather than trapping,
/// exercising the `Ok(Err(_))` branch of the read path (issue #741).
#[test]
fn read_path_governance_typed_error_falls_back_to_bootstrap() {
    let env = Env::default();
    env.mock_all_auths();

    let erroring_gov = env.register_contract(None, ErroringGovernance);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);

    inject_governance(&env, &contract_id, &erroring_gov);

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_payout_amount, 9_895);
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
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
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
    assert_eq!(split.merchant_payout_amount, 9_895);
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

    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
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

    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let empty_gov = super::register_governance(&env);

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
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

// ---------------------------------------------------------------------------
// Issue #124: Init and update_governance succeed without cross-contract calls
// ---------------------------------------------------------------------------

mod reentrant_gov {
    use crate::{GovFeeConfig, SettlementContractClient};
    use soroban_sdk::{contract, contractimpl, Address, Env, Symbol};

    /// A governance stub that attempts to call back into SettlementContract
    /// during `get_fee_config` (simulates reentrancy).
    #[contract]
    pub struct ReentrantInitGovernance;

    #[contractimpl]
    impl ReentrantInitGovernance {
        pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
            if let Some(settle_addr) = env
                .storage()
                .instance()
                .get::<_, Address>(&Symbol::new(&env, "target_settle"))
            {
                let client = SettlementContractClient::new(&env, &settle_addr);
                let _ = client.is_initialized();
            }
            None
        }

        pub fn set_target(env: Env, target_settle: Address) {
            env.storage()
                .instance()
                .set(&Symbol::new(&env, "target_settle"), &target_settle);
        }
    }
}

use reentrant_gov::ReentrantInitGovernance;

/// `init` must succeed regardless of governance's behavior, because `init`
/// does not invoke cross-contract calls on `governance` (Issue #124).
#[test]
fn init_succeeds_with_panicking_governance() {
    let env = Env::default();
    env.mock_all_auths();

    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);

    // init must succeed directly with panicking_gov without cross-calling it
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &panicking_gov,
        &recovery,
    );

    assert_eq!(client.get_governance(), panicking_gov);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, admin]);
}

/// `update_governance` must also succeed directly with a panicking governance
/// contract without making cross-contract calls during update (Issue #124).
#[test]
fn update_governance_succeeds_with_panicking_governance() {
    let env = Env::default();
    env.mock_all_auths();

    let empty_gov = super::register_governance(&env);
    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    let deployer = Address::generate(&env);

    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &empty_gov, &recovery);
    client.update_governance(&admins, &panicking_gov);

    assert_eq!(client.get_governance(), panicking_gov);
}

/// `init` succeeds with a reentrant governance contract and guards against
/// double-initialization reentrancy (Issue #124).
#[test]
fn init_succeeds_with_reentrant_governance_and_prevents_double_init() {
    let env = Env::default();
    env.mock_all_auths();

    let reentrant_gov_id = env.register_contract(None, ReentrantInitGovernance);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);

    // Configure target for potential reentrancy callback
    env.invoke_contract::<()>(
        &reentrant_gov_id,
        &soroban_sdk::Symbol::new(&env, "set_target"),
        soroban_sdk::vec![&env, contract_id.to_val()],
    );

    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &reentrant_gov_id,
        &recovery,
    );

    assert!(client.is_initialized());
    assert_eq!(client.get_governance(), reentrant_gov_id);

    // Reentry / second initialization must panic with AlreadyInitialized
    let res = client.try_init(
        &deployer,
        &soroban_sdk::vec![&env, admin],
        &1,
        &reentrant_gov_id,
        &recovery,
    );
    assert!(res.is_err());
}

// ---------------------------------------------------------------------------
// Failure Variant Coverage for Governance Fee Rule Resolution
// ---------------------------------------------------------------------------

/// Verifies that when governance returns a valid `GovFeeConfig`, `read_governance_fee_rule`
/// applies the configured fee BPS directly.
#[test]
fn read_path_governance_valid_config_used() {
    let env = Env::default();
    env.mock_all_auths();

    use governance_contract::{FeeConfig, GovernanceContract, GovernanceContractClient};

    let gov_id = env.register_contract(None, GovernanceContract);
    let gov_client = GovernanceContractClient::new(&env, &gov_id);
    let gov_admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let gov_deployer = Address::generate(&env);
    gov_client.init(
        &gov_deployer,
        &soroban_sdk::vec![&env, gov_admin.clone()],
        &1,
        &recovery,
    );

    // Set governance fee config: 250 platform bps, 50 network bps
    gov_client.set_fee_config(
        &soroban_sdk::vec![&env, gov_admin],
        &FeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        },
    );

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &gov_id,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_payout_amount, 9_700);
}

/// Verifies that when governance has no config set (`Ok(Ok(None))`), the
/// fallback chain resolves to the protocol bootstrap defaults without
/// surfacing `GovernanceCallFailed` and without emitting a
/// `BOOTSTRAP_FALLBACK_EVENT` on the hot path (issue #691).
/// Verifies that when governance has no config set (`Ok(Ok(None))`), the read
/// path falls back to the bootstrap default and does not emit a fallback event
/// (issue #691).
/// Verifies that when governance has no config set (`Ok(Ok(None))`), the fallback
/// to bootstrap default applies the bootstrap fee values.
///
/// Note: `calculate_fee_split` is a read-only path and does not emit events
/// (issue #691), so we verify the fee values directly.
#[test]
fn read_path_governance_none_falls_back_to_bootstrap_defaults() {
    let env = Env::default();
    env.mock_all_auths();

    let empty_gov = super::register_governance(&env);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);

    let amount = 10_000i128;
    let rule = crate::BOOTSTRAP_DEFAULT_RULE;
    let denom = 10_000i128;
    let expected_platform = (amount * rule.platform_fee_bps as i128 + denom - 1) / denom;
    let expected_network = (amount * rule.network_fee_bps as i128 + denom - 1) / denom;
    let expected_merchant = (amount - expected_platform - expected_network).max(0);

    let split = client.calculate_fee_split(&merchant, &amount);
    assert_eq!(split.gross_amount, amount);
    assert_eq!(split.platform_fee_amount, expected_platform);
    assert_eq!(split.network_fee_amount, expected_network);
    assert_eq!(split.merchant_payout_amount, expected_merchant);

    // No BOOTSTRAP_FALLBACK_EVENT on the hot path (issue #691).
    let events = env.events().all();
    for i in 0..events.len() {
        let (_contract, topics, _data) = events.get(i).unwrap();
        if !topics.is_empty() {
            let sym = Symbol::from_val(&env, &topics.get(0).unwrap());
            assert_ne!(
                sym,
                Symbol::new(&env, bettapay_common::events::BOOTSTRAP_FALLBACK_EVENT),
                "bootstrap_fallback must not be emitted when falling back on the hot path"
            );
        }
    }
    // Bootstrap default: 100 bps platform, 5 bps network.
    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_payout_amount, 9_895);
}

// ---------------------------------------------------------------------------
// Issue #743: bypass_gov_fees circuit-breaker
// ---------------------------------------------------------------------------

/// Governance stub with the circuit-breaker enabled and a `get_fee_config`
/// that traps if it is ever consulted. Proves the read path short-circuits
/// *before* the fee-config call, not after catching its failure.
mod bypass_gov {
    use crate::GovFeeConfig;
    use soroban_sdk::{contract, contractimpl, Env, Symbol};

    #[contract]
    pub struct BypassGovernance;

    #[contractimpl]
    impl BypassGovernance {
        pub fn get_system_param(env: Env, key: Symbol) -> Option<i128> {
            if key == Symbol::new(&env, "bypass_gov_fees") {
                Some(1)
            } else {
                None
            }
        }

        #[allow(unused_variables)]
        pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
            panic!("get_fee_config must not be called when bypass_gov_fees is 1")
        }
    }
}

use bypass_gov::BypassGovernance;

/// Builds a real governance contract with a fee config and (optionally) the
/// bypass flag, plus a settlement contract wired to it and a registered
/// merchant.
fn setup_with_bypass(
    env: &Env,
    platform_fee_bps: u32,
    network_fee_bps: u32,
    bypass: Option<i128>,
) -> (SettlementContractClient<'_>, Address) {
    let gov_id = env.register_contract(None, GovernanceContract);
    let gov_client = GovernanceContractClient::new(env, &gov_id);
    let gov_admin = Address::generate(env);
    let recovery = Address::generate(env);
    let gov_deployer = Address::generate(env);
    gov_client.init(
        &gov_deployer,
        &soroban_sdk::vec![env, gov_admin.clone()],
        &1,
        &recovery,
    );
    gov_client.set_fee_config(
        &soroban_sdk::vec![env, gov_admin.clone()],
        &GovFeeConfig {
            platform_fee_bps,
            network_fee_bps,
        },
    );
    if let Some(value) = bypass {
        gov_client.update_system_param(
            &soroban_sdk::vec![env, gov_admin],
            &Symbol::new(env, "bypass_gov_fees"),
            &value,
        );
    }

    let admin = Address::generate(env);
    let merchant = Address::generate(env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(env, &contract_id);
    client.init(
        &gov_deployer,
        &soroban_sdk::vec![env, admin.clone()],
        &1,
        &gov_id,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![env, admin], &merchant);
    (client, merchant)
}

#[test]
fn bypass_gov_fees_one_uses_bootstrap_instead_of_governance_config() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, merchant) = setup_with_bypass(&env, 250, 50, Some(1));

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_payout_amount, 9_895);
}

#[test]
fn bypass_gov_fees_zero_keeps_governance_config() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, merchant) = setup_with_bypass(&env, 250, 50, Some(0));

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_payout_amount, 9_700);
}

#[test]
fn bypass_gov_fees_unset_keeps_governance_config() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, merchant) = setup_with_bypass(&env, 250, 50, None);

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
}

/// The strongest form of the check: governance's `get_fee_config` traps, but
/// because the bypass flag reads as 1 first, the payment path never calls it.
#[test]
fn bypass_gov_fees_skips_a_trapping_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let gov_id = env.register_contract(None, BypassGovernance);
    let empty_gov = super::register_governance(&env);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);
    inject_governance(&env, &contract_id, &gov_id);

    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
}

/// The bypass is a **read-path** circuit-breaker. Admin writes still validate
/// against governance (issue #742), so this traps with `GovernanceCallFailed`
/// even though reads would bypass.
#[test]
#[should_panic(expected = "Error(Contract, #311)")]
fn bypass_gov_fees_does_not_skip_admin_write_validation() {
    let env = Env::default();
    env.mock_all_auths();

    let gov_id = env.register_contract(None, BypassGovernance);
    let empty_gov = super::register_governance(&env);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);
    inject_governance(&env, &contract_id, &gov_id);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&soroban_sdk::vec![&env, admin], &merchant, &rule);
}

// ---------------------------------------------------------------------------
// Issue #973: Governance fee config with u32::MAX
// ---------------------------------------------------------------------------

/// A settlement rule that exceeds the governance fee ceiling must be rejected
/// with `FeeExceedsGovernanceConfig` (#312) rather than trapping or overflowing,
/// even when governance's configured ceilings are extreme.
#[test]
#[should_panic(expected = "Error(Contract, #312)")]
fn settlement_rule_exceeding_governance_ceiling_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    use governance_contract::{FeeConfig, GovernanceContract, GovernanceContractClient};

    let gov_id = env.register_contract(None, GovernanceContract);
    let gov_client = GovernanceContractClient::new(&env, &gov_id);
    let gov_admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let gov_deployer = Address::generate(&env);
    gov_client.init(
        &gov_deployer,
        &soroban_sdk::vec![&env, gov_admin.clone()],
        &1,
        &recovery,
    );

    // Governance ceiling: 100 platform bps, 50 network bps.
    gov_client.set_fee_config(
        &soroban_sdk::vec![&env, gov_admin],
        &FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 50,
        },
    );

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &gov_id,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin.clone()], &merchant);

    // Rule exceeding the governance ceiling (200 > 100 platform).
    let over_ceiling_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&soroban_sdk::vec![&env, admin], &merchant, &over_ceiling_rule);
}

/// When governance has no fee config set, the settlement read path falls back
/// to [`BOOTSTRAP_DEFAULT_RULE`] (100 bps platform, 5 bps network) instead of
/// surfacing `GovernanceCallFailed`.
#[test]
fn settlement_read_path_falls_back_to_bootstrap_default_rule() {
    let env = Env::default();
    env.mock_all_auths();

    let empty_gov = super::register_governance(&env);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &empty_gov,
        &recovery,
    );
    client.register_merchant(&soroban_sdk::vec![&env, admin], &merchant);

    let amount = 10_000i128;
    let rule = crate::BOOTSTRAP_DEFAULT_RULE;
    let denom = 10_000i128;
    let expected_platform = (amount * rule.platform_fee_bps as i128 + denom - 1) / denom;
    let expected_network = (amount * rule.network_fee_bps as i128 + denom - 1) / denom;
    let expected_merchant = (amount - expected_platform - expected_network).max(0);

    let split = client.calculate_fee_split(&merchant, &amount);
    assert_eq!(split.gross_amount, amount);
    assert_eq!(split.platform_fee_amount, expected_platform);
    assert_eq!(split.network_fee_amount, expected_network);
    assert_eq!(split.merchant_payout_amount, expected_merchant);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_payout_amount, 9_895);
}

// ---------------------------------------------------------------------------
// Scheduling liveness precheck (issue #994)
// ---------------------------------------------------------------------------

/// With an unreachable governance, `schedule` fails with the typed
/// `GovernanceCallFailed` error and writes no `ScheduledOperation` entry.
#[test]
fn schedule_rejects_unreachable_governance_without_writing_storage() {
    use soroban_sdk::xdr::ToXdr;

    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin];
    let merchant = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(
        &Address::generate(&env),
        &admins,
        &1,
        &super::register_governance(&env),
        &Address::generate(&env),
    );

    let operation = Operation::RegisterMerchant(merchant);
    let key =
        DataKey::ScheduledOperation(env.crypto().sha256(&operation.clone().to_xdr(&env)).into());

    // Reachable governance: scheduling succeeds and the entry is written.
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(env.as_contract(&contract_id, || env.storage().persistent().has(&key)));
    client.cancel(&admins, &operation);
    assert!(!env.as_contract(&contract_id, || env.storage().persistent().has(&key)));

    // Unreachable governance: typed error, nothing written.
    let panicking_gov = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    inject_governance(&env, &contract_id, &panicking_gov);

    assert_eq!(
        client.try_schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS),
        Err(Ok(SettlementError::GovernanceCallFailed))
    );
    assert!(!env.as_contract(&contract_id, || env.storage().persistent().has(&key)));
}

// ---------------------------------------------------------------------------
// Issue #901: CachedGovRule cache invalidation and fail-open fallback
// ---------------------------------------------------------------------------

/// Reads `DataKey::CachedGovRule`, returning `None` when the key is absent.
fn cached_gov_rule(env: &Env, contract_id: &Address) -> Option<Option<SettlementRule>> {
    env.as_contract(contract_id, || {
        env.storage()
            .instance()
            .get::<_, Option<SettlementRule>>(&DataKey::CachedGovRule)
    })
}

/// Asserts the cache holds `Some(expected)`.
fn assert_cached(env: &Env, contract_id: &Address, expected: &SettlementRule) {
    let cached = cached_gov_rule(env, contract_id)
        .expect("CachedGovRule must be written")
        .expect("CachedGovRule must hold Some(rule)");
    super::assert_rule_eq(&cached, expected, "CachedGovRule");
}

fn gov_rule(platform_fee_bps: u32, network_fee_bps: u32) -> SettlementRule {
    SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger: 0,
        auto_settle: false,
    }
}

/// Every successful governance read overwrites the cache, so a governance
/// config change is never shadowed by a stale value. Once governance traps,
/// the payment path fails open onto the last good cached rule, while admin
/// writes still fail loud with `GovernanceCallFailed`.
#[test]
fn governance_fee_cache_tracks_updates_and_survives_trap() {
    use super::mock_governance::MockGovernanceClient;

    let env = Env::default();
    env.mock_all_auths();

    let governance = super::register_governance_with_config(
        &env,
        crate::GovFeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        },
    );
    let gov = MockGovernanceClient::new(&env, &governance);
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let merchant = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(
        &Address::generate(&env),
        &admins,
        &1,
        &governance,
        &Address::generate(&env),
    );
    client.register_merchant(&admins, &merchant);

    // A successful read populates the cache.
    assert!(cached_gov_rule(&env, &contract_id).is_none());
    super::assert_rule_eq(
        &client.get_effective_rule(&merchant),
        &gov_rule(250, 50),
        "effective rule",
    );
    assert_cached(&env, &contract_id, &gov_rule(250, 50));

    // Governance changes its config: the next read replaces the cached value.
    gov.set_fee_config(&crate::GovFeeConfig {
        platform_fee_bps: 300,
        network_fee_bps: 40,
    });
    super::assert_rule_eq(
        &client.get_effective_rule(&merchant),
        &gov_rule(300, 40),
        "effective rule",
    );
    assert_cached(&env, &contract_id, &gov_rule(300, 40));

    // Governance now traps: the payment uses the cached bps, not bootstrap.
    gov.set_mode(&super::MockGovMode::Trap);
    let reference = soroban_sdk::BytesN::<32>::from_array(&env, &[0x91; 32]);
    let split = client.store_payment_reference(&merchant, &reference, &10_000);
    assert_eq!(split.platform_fee_amount, 300);
    assert_eq!(split.network_fee_amount, 40);
    let record = client
        .get_payment_reference(&merchant, &reference, &soroban_sdk::vec![&env])
        .expect("payment record must exist despite governance trap");
    assert_eq!(record.platform_fee_bps, 300);
    assert_eq!(record.network_fee_bps, 40);
    assert_ne!(
        record.platform_fee_bps,
        BOOTSTRAP_DEFAULT_RULE.platform_fee_bps
    );
    // A failed read leaves the last good value in place.
    assert_cached(&env, &contract_id, &gov_rule(300, 40));

    // Reads fail open, writes fail loud: the cache never satisfies an admin write.
    let res = client.try_set_default_rule(&admins, &gov_rule(200, 30));
    assert_eq!(
        res.map(|_| ()).map_err(|e| e.map_err(|_| ())),
        Err(Ok(soroban_sdk::Error::from_contract_error(
            SettlementError::GovernanceCallFailed as u32
        )))
    );
    assert!(client.get_default_rule().is_none());
}

/// With no successful governance read yet, a trap has nothing cached to fall
/// back on, so the payment resolves to `BOOTSTRAP_DEFAULT_RULE`.
#[test]
fn governance_trap_without_cache_uses_bootstrap_rule() {
    let env = Env::default();
    env.mock_all_auths();

    let governance = super::register_governance_with_mode(&env, super::MockGovMode::Trap);
    let admins = soroban_sdk::vec![&env, Address::generate(&env)];
    let merchant = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(
        &Address::generate(&env),
        &admins,
        &1,
        &governance,
        &Address::generate(&env),
    );
    client.register_merchant(&admins, &merchant);

    let reference = soroban_sdk::BytesN::<32>::from_array(&env, &[0x92; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);
    let record = client
        .get_payment_reference(&merchant, &reference, &soroban_sdk::vec![&env])
        .expect("payment record must exist despite governance trap");
    assert_eq!(
        record.platform_fee_bps,
        BOOTSTRAP_DEFAULT_RULE.platform_fee_bps
    );
    assert_eq!(
        record.network_fee_bps,
        BOOTSTRAP_DEFAULT_RULE.network_fee_bps
    );
    assert_eq!(
        record.settlement_delay_ledger,
        BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
    );
    assert_eq!(record.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
    assert!(cached_gov_rule(&env, &contract_id).is_none());
}
