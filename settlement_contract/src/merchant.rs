use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_internal, persistent_get_safe, persistent_has_safe,
    read_threshold, validate_nonzero_address, verify_admin_auth,
};
use crate::types::{DataKey, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, MERCHANT_TTL_BUMP, MERCHANT_TTL_THRESHOLD,
};

#[contractimpl]
impl SettlementContract {
        pub fn register_merchant(env: Env, signers: Vec<Address>, merchant: Address) {
            assert_not_paused(&env);

            validate_nonzero_address(
                &env,
                &merchant,
                SettlementError::EmptyAddress,
                SettlementError::ZeroAddress,
            );

            verify_admin_auth(&env, &signers, read_threshold(&env));
            let admin = signers.get(0).unwrap();

            let key = DataKey::Merchant(merchant.clone());
            if persistent_has_safe(&env, &key) {
                panic_with_error!(&env, SettlementError::MerchantExists);
            }

            env.storage().persistent().set(&key, &());
            env.storage()
                .persistent()
                .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
            env.events()
                .publish((Symbol::new(&env, "merchant_registered"), merchant), admin);
        }

        pub fn unregister_merchant(env: Env, signers: Vec<Address>, merchant: Address) {
            assert_not_paused(&env);
            verify_admin_auth(&env, &signers, read_threshold(&env));
            let admin = signers.get(0).unwrap();

            let key = DataKey::Merchant(merchant.clone());
            if !persistent_has_safe(&env, &key) {
                panic_with_error!(&env, SettlementError::MerchantMissing);
            }

            env.storage().persistent().remove(&key);

            let rule_key = DataKey::Rule(merchant.clone());
            let old_rule: Option<SettlementRule> = persistent_get_safe(&env, &rule_key);
            if let Some(old_rule) = old_rule {
                env.storage().persistent().remove(&rule_key);
                env.events().publish(
                    (
                        Symbol::new(&env, "settlement_rule_cleared"),
                        merchant.clone(),
                    ),
                    (admin.clone(), old_rule),
                );
            }

            env.events().publish(
                (Symbol::new(&env, "merchant_unregistered"), merchant),
                admin,
            );
        }

        /// Returns `true` if the given address is a registered merchant, `false` otherwise.
        ///
        /// # Panics
        ///
        /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
        pub fn is_merchant_registered(env: Env, merchant: Address) -> bool {
            if !env.storage().instance().has(&DataKey::Admin) {
                panic_with_error!(&env, SettlementError::NotInitialized);
            }
            is_merchant_registered_internal(&env, merchant)
        }

}
