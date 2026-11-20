//! Shared configurable mock governance fixture for the settlement contract test suite.
//!
//! Provides a single mock governance contract covering four behavioral modes:
//! - [`MockGovMode::NoConfig`]: `get_fee_config` returns `None` (`Void`), simulating governance with no config set.
//! - [`MockGovMode::Valid`]: `get_fee_config` returns a valid 2-field fee config map.
//! - [`MockGovMode::Malformed`]: `get_fee_config` returns a malformed 1-field map (issue #483).
//! - [`MockGovMode::Trap`]: `get_fee_config` panics/traps during invocation.

use crate::GovFeeConfig;
use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, IntoVal, Map, Symbol, Val};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum MockGovMode {
    NoConfig = 0,
    Valid = 1,
    Malformed = 2,
    Trap = 3,
}

#[contract]
pub struct MockGovernance;

#[contractimpl]
impl MockGovernance {
    pub fn set_mode(env: Env, mode: MockGovMode) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "mode"), &mode);
    }

    pub fn set_fee_config(env: Env, config: GovFeeConfig) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "mode"), &MockGovMode::Valid);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "config"), &config);
    }

    pub fn get_fee_config(env: Env) -> Val {
        let mode: MockGovMode = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "mode"))
            .unwrap_or(MockGovMode::NoConfig);

        match mode {
            MockGovMode::NoConfig => ().into_val(&env),
            MockGovMode::Valid => {
                let config: GovFeeConfig = env
                    .storage()
                    .instance()
                    .get(&Symbol::new(&env, "config"))
                    .unwrap_or(GovFeeConfig {
                        platform_fee_bps: 100,
                        network_fee_bps: 50,
                    });
                config.into_val(&env)
            }
            MockGovMode::Malformed => {
                let mut map: Map<Symbol, Val> = Map::new(&env);
                map.set(Symbol::new(&env, "platform_fee_bps"), 100u32.into_val(&env));
                map.into_val(&env)
            }
            MockGovMode::Trap => {
                panic!("governance trap");
            }
        }
    }
}

/// Registers a fresh `MockGovernance` contract in default `NoConfig` mode and returns its address.
pub fn register_governance(env: &Env) -> Address {
    register_governance_with_mode(env, MockGovMode::NoConfig)
}

/// Registers a fresh `MockGovernance` contract in the specified mode and returns its address.
pub fn register_governance_with_mode(env: &Env, mode: MockGovMode) -> Address {
    let address = env.register_contract(None, MockGovernance);
    env.as_contract(&address, || {
        env.storage()
            .instance()
            .set(&Symbol::new(env, "mode"), &mode);
    });
    address
}

/// Registers a fresh `MockGovernance` contract configured with a valid `GovFeeConfig`.
pub fn register_governance_with_config(env: &Env, config: GovFeeConfig) -> Address {
    let address = env.register_contract(None, MockGovernance);
    env.as_contract(&address, || {
        env.storage()
            .instance()
            .set(&Symbol::new(env, "mode"), &MockGovMode::Valid);
        env.storage()
            .instance()
            .set(&Symbol::new(env, "config"), &config);
    });
    address
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::SettlementError;

    #[test]
    fn mock_governance_no_config_mode() {
        let env = Env::default();
        let gov = register_governance(&env);
        let res = env.try_invoke_contract::<Val, SettlementError>(
            &gov,
            &Symbol::new(&env, "get_fee_config"),
            soroban_sdk::Vec::new(&env),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn mock_governance_valid_config_mode() {
        let env = Env::default();
        let cfg = GovFeeConfig {
            platform_fee_bps: 300,
            network_fee_bps: 100,
        };
        let gov = register_governance_with_config(&env, cfg.clone());
        let outcome = env.try_invoke_contract::<Option<GovFeeConfig>, SettlementError>(
            &gov,
            &Symbol::new(&env, "get_fee_config"),
            soroban_sdk::Vec::new(&env),
        );
        match outcome {
            Ok(Ok(Some(decoded))) => {
                assert_eq!(decoded.platform_fee_bps, 300);
                assert_eq!(decoded.network_fee_bps, 100);
            }
            _ => panic!("expected valid decoded fee config"),
        }
    }

    #[test]
    fn mock_governance_malformed_mode() {
        let env = Env::default();
        let gov = register_governance_with_mode(&env, MockGovMode::Malformed);
        let res = env.try_invoke_contract::<Val, SettlementError>(
            &gov,
            &Symbol::new(&env, "get_fee_config"),
            soroban_sdk::Vec::new(&env),
        );
        assert!(res.is_ok());
        let val = res.unwrap().unwrap();
        // Malformed map only has 1 field instead of 2
        let map: Map<Symbol, Val> = Map::try_from_val(&env, &val).expect("expected map");
        assert_eq!(map.len(), 1);
    }

    #[test]
    #[should_panic(expected = "governance trap")]
    fn mock_governance_trap_mode() {
        let env = Env::default();
        let gov = register_governance_with_mode(&env, MockGovMode::Trap);
        let _ = env.invoke_contract::<Val>(
            &gov,
            &Symbol::new(&env, "get_fee_config"),
            soroban_sdk::Vec::new(&env),
        );
    }
}
