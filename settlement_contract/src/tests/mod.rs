//! Shared test utilities for the settlement contract test suite.
//!
//! This module exposes the common `setup()` helper, the `MockGovernance` contract
//! used in most tests, and the `register_governance` helper. Feature-specific
//! test modules are declared here so Rust can discover them during `cargo test`.

pub mod admin_tests;
pub mod conformity_tests;
pub mod payment_tests;

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, Address, Env, Vec};

/// A minimal governance stub that returns `None` from `get_fee_config`.
/// This satisfies `validate_governance` in `init`, which requires the
/// governance address to be a deployed contract exposing that function.
#[contract]
pub struct MockGovernance;

#[contractimpl]
impl MockGovernance {
    pub fn get_fee_config(_env: Env) -> Option<FeeConfig> {
        None
    }
}

/// Registers a fresh `MockGovernance` contract and returns its address.
pub fn register_governance(env: &Env) -> Address {
    env.register_contract(None, MockGovernance)
}

/// Creates a fully initialised settlement contract environment.
///
/// Returns `(env, client, admins, merchant)` where:
/// - `env`      — default Soroban test environment with `mock_all_auths` enabled.
/// - `client`   — a `SettlementContractClient` bound to the registered contract.
/// - `admins`   — the signer set (single admin, threshold 1) used for admin calls.
/// - `merchant` — a freshly generated address that has **not** been registered;
///                individual tests must call `client.register_merchant(&admins, &merchant)`
///                when they need a registered merchant.
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
    client.init(&admins, &1, &governance, &recovery_address);
    (env, client, admins, merchant)
}
