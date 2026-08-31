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
use governance_contract::{GovernanceContract, GovernanceContractClient};
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{Address, Env, FromVal, Symbol};

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

    let panicking_gov = env.register_contract(None, PanickingGovernance);
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

    let panicking_gov = env.register_contract(None, PanickingGovernance);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

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
    let panicking_gov = env.register_contract(None, PanickingGovernance);
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];

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
// Failure Variant Coverage for Governance Fee Rule Resolution
// ---------------------------------------------------------------------------

/// Verifies that when governance returns a valid `GovFeeConfig`, `read_governance_fee_rule`
/// applies the configured fee BPS directly.
#[test]
fn read_path_governance_valid_config_used() {
    let env = Env::default();
    env.mock_all_auths();

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
        &governance_contract::FeeConfig {
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
    assert_eq!(split.merchant_amount, 9_700);
}

/// Verifies that when governance has no config set (`Ok(Ok(None))`), the fallback
/// to bootstrap default emits `BOOTSTRAP_FALLBACK_EVENT`.
#[test]
fn read_path_governance_none_emits_bootstrap_fallback_event() {
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

    client.calculate_fee_split(&merchant, &10_000);

    let events = env.events().all();
    let mut fallback_event_emitted = false;
    for i in 0..events.len() {
        let (_contract, topics, _data) = events.get(i).unwrap();
        if !topics.is_empty() {
            let sym = Symbol::from_val(&env, &topics.get(0).unwrap());
            if sym == Symbol::new(&env, bettapay_common::events::BOOTSTRAP_FALLBACK_EVENT) {
                fallback_event_emitted = true;
            }
        }
    }
    assert!(
        fallback_event_emitted,
        "BOOTSTRAP_FALLBACK_EVENT must be emitted when degrading to bootstrap defaults"
    );
}
