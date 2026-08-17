//! Shared test utilities for the settlement contract test suite.
//!
//! This module exposes the common `setup()` helper, the `MockGovernance` contract
//! used in most tests, and the `register_governance` helper. Feature-specific
//! test modules are declared here so Rust can discover them during `cargo test`.

pub mod admin_tests;
pub mod conformity_tests;

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

/// Issue #583 (settlement counterpart): storage operations on expired/archived
/// persistent entries must surface typed contract errors rather than raw host
/// errors. The following tests cover merchant, rule, payment, and default-rule
/// expiry paths.
#[cfg(test)]
mod storage_ttl_expiry_tests {
    use super::*;
    use crate::types::DataKey;
    use soroban_sdk::testutils::{Ledger, Storage as _};

    /// unregister_merchant on an expired merchant marker must raise the typed
    /// `MerchantMissing` (301) error instead of a raw host error.
    #[test]
    #[should_panic(expected = "Error(Contract, #301)")]
    fn unregister_merchant_on_expired_returns_typed_merchant_missing() {
        let (env, client, admins, merchant) = setup();

        client.register_merchant(&admins, &merchant);
        assert!(client.is_merchant_registered(&merchant));

        env.as_contract(&client.address, || {
            let key = DataKey::Merchant(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        client.unregister_merchant(&admins, &merchant);
    }

    /// register_merchant on an expired merchant marker treats the entry as
    /// missing and succeeds (no duplicate-exists panic).
    #[test]
    fn register_merchant_on_expired_marker_treats_as_missing_and_succeeds() {
        let (env, client, admins, merchant) = setup();

        client.register_merchant(&admins, &merchant);
        assert!(client.is_merchant_registered(&merchant));

        env.as_contract(&client.address, || {
            let key = DataKey::Merchant(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        client.register_merchant(&admins, &merchant);
        assert!(client.is_merchant_registered(&merchant));
    }

    /// is_merchant_registered on an expired entry returns `false` instead of
    /// panicking with a raw host error.
    #[test]
    fn is_merchant_registered_on_expired_returns_false() {
        let (env, client, admins, merchant) = setup();

        client.register_merchant(&admins, &merchant);
        assert!(client.is_merchant_registered(&merchant));

        env.as_contract(&client.address, || {
            let key = DataKey::Merchant(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert!(!client.is_merchant_registered(&merchant));
    }

    /// get_default_rule on an expired entry returns `None` instead of
    /// panicking with a raw host error.
    #[test]
    fn get_default_rule_on_expired_returns_none() {
        let (env, client, admins, _) = setup();
        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        client.set_default_rule(&admins, &rule);
        assert!(client.get_default_rule().is_some());

        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_default_rule(), None);
    }

    /// get_settlement_rule on an expired merchant rule returns `None`
    /// instead of panicking with a raw host error.
    #[test]
    fn get_settlement_rule_on_expired_returns_none() {
        let (env, client, admins, merchant) = setup();
        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };

        client.register_merchant(&admins, &merchant);
        client.set_settlement_rule(&admins, &merchant, &rule);
        assert!(client.get_settlement_rule(&merchant).is_some());

        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_settlement_rule(&merchant), None);
    }

    /// clear_settlement_rule on an expired rule raises the typed
    /// `MerchantRuleNotSet` (304) error instead of a raw host error.
    #[test]
    #[should_panic(expected = "Error(Contract, #304)")]
    fn clear_settlement_rule_on_expired_returns_typed_rule_not_set() {
        let (env, client, admins, merchant) = setup();
        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };

        client.register_merchant(&admins, &merchant);
        client.set_settlement_rule(&admins, &merchant, &rule);
        assert!(client.get_settlement_rule(&merchant).is_some());

        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        client.clear_settlement_rule(&admins, &merchant);
    }

    /// get_payment_reference on an expired payment returns `None` instead of
    /// panicking with a raw host error.
    #[test]
    fn get_payment_reference_on_expired_returns_none() {
        let (env, client, admins, merchant) = setup();

        client.register_merchant(&admins, &merchant);
        let reference = BytesN::from_array(&env, &[1u8; 32]);
        let amount: i128 = 1_000_000;
        client.store_payment_reference(&merchant, &reference, &amount);
        assert!(client.get_payment_reference(&reference).is_some());

        env.as_contract(&client.address, || {
            let key = DataKey::Payment(reference.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_payment_reference(&reference), None);
    }

    /// read_rule_or_default falls through to the default when the merchant
    /// rule is archived, and then to bootstrap when the default is also
    /// archived. No raw host error is raised on either path.
    #[test]
    fn calculate_fee_split_survives_archived_rule_and_default_entries() {
        let (env, client, admins, merchant) = setup();
        let custom_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 100,
            settlement_delay_ledger: 5,
            auto_settle: false,
        };

        client.register_merchant(&admins, &merchant);
        client.set_settlement_rule(&admins, &merchant, &custom_rule);
        client.set_default_rule(
            &admins,
            &SettlementRule {
                platform_fee_bps: 150,
                network_fee_bps: 75,
                settlement_delay_ledger: 0,
                auto_settle: false,
            },
        );

        // Expire the merchant rule first: fallback should be the default.
        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        // Should not panic; fee split should come from the default rule now.
        let split = client.calculate_fee_split(&merchant, &10_000i128);
        assert_eq!(split.platform_fee_bps, 150);
        assert_eq!(split.network_fee_bps, 75);

        // Now expire the default rule too: fallback should be bootstrap.
        env.as_contract(&client.address, || {
            let key = DataKey::DefaultRule;
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        // Should still not panic; bootstrap fallback applies (100/0 bps).
        let split = client.calculate_fee_split(&merchant, &10_000i128);
        assert_eq!(split.platform_fee_bps, 100);
        assert_eq!(split.network_fee_bps, 0);
    }
}
