//! Shared test utilities for the settlement contract test suite.
//!
//! This module exposes the common `setup()` helper, the `MockGovernance` contract
//! used in most tests, and the `register_governance` helper. Feature-specific
//! test modules are declared here so Rust can discover them during `cargo test`.

pub mod admin_tests;
pub mod conformity_tests;
pub mod data_key_fuzz_tests;
pub mod event_topic_conformity_tests;
pub mod event_topic_registry_tests;
pub mod fee_bounds_parity_tests;
pub mod fee_config_ordering_tests;
pub mod governance_error_tests;
pub mod integration_tests;
pub mod interface_tests;
pub mod mock_governance;
#[path = "../../../tests/test_invariant_issue_723.rs"]
mod issue_invariant_tests;
pub mod operation_roundtrip_tests;
pub mod recovery_admin_set_tests;
pub mod reentrant_governance;
pub mod schedule_collision_tests;
pub mod timelock_tests;
pub mod ttl_tests;

pub use mock_governance::{
    register_governance, register_governance_with_config, register_governance_with_mode,
    MockGovMode, MockGovernance,
};

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Vec};

/// Creates a fully initialised settlement contract environment.
///
/// Returns `(env, client, admins, merchant)` where:
/// - `env`      — default Soroban test environment with `mock_all_auths` enabled.
/// - `client`   — a `SettlementContractClient` bound to the registered contract.
/// - `admins`   — the signer set (single admin, threshold 1) used for admin calls.
/// - `merchant` — a freshly generated address that has **not** been registered;
///   individual tests must call `client.register_merchant(&admins, &merchant)`
///   when they need a registered merchant.
pub fn setup() -> (
    Env,
    SettlementContractClient<'static>,
    Vec<Address>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let merchant = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery_address);
    (env, client, admins, merchant)
}

/// Asserts two rules are equal field by field. `SettlementRule` derives
/// neither `PartialEq` nor `Debug`, so the fields are compared as a tuple to
/// get a readable diff on failure.
pub fn assert_rule_eq(actual: &SettlementRule, expected: &SettlementRule, context: &str) {
    let fields = |r: &SettlementRule| {
        (
            r.platform_fee_bps,
            r.network_fee_bps,
            r.settlement_delay_ledger,
            r.auto_settle,
        )
    };
    assert_eq!(fields(actual), fields(expected), "{context}");
}
