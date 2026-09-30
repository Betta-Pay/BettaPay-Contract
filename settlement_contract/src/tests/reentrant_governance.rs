//! A malicious governance contract used to test init-reentrancy guard (#566).
//!
//! When its `get_fee_config` is invoked (during settlement's `validate_governance`
//! call inside `init`), it attempts to reenter `init` on the settlement contract.

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Symbol, Vec};

const TARGET_KEY: Symbol = symbol_short!("target");

/// Stores the settlement contract address in this governance's instance
/// storage so `get_fee_config` can reenter it.
pub fn stash_target(env: &Env, target: &Address) {
    env.storage().instance().set(&TARGET_KEY, target);
}

#[contract]
pub struct ReentrantGovernance;

#[contractimpl]
impl ReentrantGovernance {
    pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
        let settlement: Address = env.storage().instance().get(&TARGET_KEY).unwrap();

        let client = SettlementContractClient::new(&env, &settlement);
        // This must panic with AlreadyInitialized because the init-in-progress
        // marker is already set by the first (outer) init call.
        let deployer = Address::generate(&env);
        client.init(
            &deployer,
            &Vec::new(&env),
            &0,
            &settlement,
            &Address::generate(&env),
        );
        None
    }
}
