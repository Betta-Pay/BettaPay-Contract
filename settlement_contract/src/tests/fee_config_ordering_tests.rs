//! Regression coverage for issue #572: the settlement contract decodes
//! governance's `FeeConfig` cross-contract return value into its own local
//! `GovFeeConfig` type. Both structs happen to declare `platform_fee_bps`
//! before `network_fee_bps` today, but nothing *guarantees* the two stay in
//! lockstep — a future reorder of one struct's fields must not silently swap
//! which value lands in which field.
//!
//! Soroban's `#[contracttype]` derive encodes/decodes named-field structs as
//! an `ScMap` keyed by field *name* (sorted alphabetically), not by
//! declaration order or positional index — see
//! `soroban-sdk-macros::derive_struct::derive_type_struct`. So decoding is
//! already order-independent by construction. This test proves that
//! property directly: it stands up a governance stub whose fee-config type
//! declares its fields in the *opposite* order from settlement's
//! `GovFeeConfig`, and asserts the values still decode into the correct
//! fields.

use crate::types::DataKey;
use crate::GovFeeConfig;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

/// A governance-side fee config with its fields declared in the reverse
/// order of settlement's `GovFeeConfig` (network before platform). If the
/// wire format were positional (index 0 / index 1) instead of name-keyed,
/// this reordering would cause settlement to decode `network_fee_bps` into
/// its `platform_fee_bps` field and vice versa.
mod reordered_gov {
    use soroban_sdk::{contract, contractimpl, contracttype, Env};

    #[derive(Clone)]
    #[contracttype]
    pub struct ReorderedFeeConfig {
        pub network_fee_bps: u32,
        pub platform_fee_bps: u32,
    }

    #[contract]
    pub struct ReorderedGovernance;

    #[contractimpl]
    impl ReorderedGovernance {
        pub fn get_fee_config(_env: Env) -> Option<ReorderedFeeConfig> {
            Some(ReorderedFeeConfig {
                platform_fee_bps: 777,
                network_fee_bps: 222,
            })
        }
    }
}

use reordered_gov::ReorderedGovernance;

fn inject_governance(env: &Env, contract_address: &Address, governance: &Address) {
    env.as_contract(contract_address, || {
        env.storage()
            .instance()
            .set(&DataKey::Governance, governance);
    });
}

/// Decoding the field-order-swapped governance struct into settlement's
/// `GovFeeConfig` must still land each bps value in the field with the
/// matching *name*, not the matching *position*.
#[test]
fn decode_is_order_independent_by_field_name() {
    let env = Env::default();
    env.mock_all_auths();

    let governance = env.register_contract(None, ReorderedGovernance);
    let contract_id = env.register_contract(None, crate::SettlementContract);
    inject_governance(&env, &contract_id, &governance);

    let outcome = env.as_contract(&contract_id, || {
        env.try_invoke_contract::<Option<GovFeeConfig>, crate::SettlementError>(
            &governance,
            &soroban_sdk::Symbol::new(&env, "get_fee_config"),
            soroban_sdk::Vec::new(&env),
        )
    });
    let result = match outcome {
        Ok(Ok(result)) => result,
        _ => panic!("get_fee_config call must succeed against a well-behaved stub"),
    };

    let config = result.expect("governance stub always returns Some");
    assert_eq!(
        config.platform_fee_bps, 777,
        "platform_fee_bps must decode by field name, not by declaration position"
    );
    assert_eq!(
        config.network_fee_bps, 222,
        "network_fee_bps must decode by field name, not by declaration position"
    );
}

/// End-to-end variant: drive the same reordered-fields governance stub
/// through the real `calculate_fee_split` entry point (no merchant-specific
/// or default rule set, so it falls through to governance) and confirm the
/// computed fee split reflects the correct platform/network bps.
#[test]
fn calculate_fee_split_uses_correctly_ordered_governance_fees() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let merchant = Address::generate(&env);
    let governance = env.register_contract(None, ReorderedGovernance);
    let contract_id = env.register_contract(None, crate::SettlementContract);
    let client = crate::SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery_address);
    client.register_merchant(&admins, &merchant);

    let amount: i128 = 1_000_000;
    let split = client.calculate_fee_split(&merchant, &amount);

    // 777 bps and 222 bps of 1_000_000 at BPS_DENOMINATOR = 10_000.
    assert_eq!(split.platform_fee_amount, amount * 777 / 10_000);
    assert_eq!(split.network_fee_amount, amount * 222 / 10_000);
}
