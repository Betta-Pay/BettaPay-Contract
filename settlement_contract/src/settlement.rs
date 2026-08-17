use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::constants::{BPS_DENOMINATOR, MIN_FEE_BPS};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_internal, persistent_get_safe, read_rule_or_default,
    read_threshold, validate_fee_against_governance, verify_admin_auth,
};
use crate::types::{DataKey, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, BOOTSTRAP_DEFAULT_RULE,
    MAX_SETTLEMENT_DELAY_LEDGER, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
};

#[contractimpl]
impl SettlementContract {
        pub fn set_settlement_rule(
            env: Env,
            signers: Vec<Address>,
            merchant: Address,
            rule: SettlementRule,
        ) {
            assert_not_paused(&env);
            verify_admin_auth(&env, &signers, read_threshold(&env));
            let admin = signers.get(0).unwrap();

            validate_fee_against_governance(&env, &rule);

            if !is_merchant_registered_internal(&env, merchant.clone()) {
                panic_with_error!(&env, SettlementError::MerchantMissing);
            }
            if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
                panic_with_error!(&env, SettlementError::InvalidFeeBps);
            }
            if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
                panic_with_error!(&env, SettlementError::InvalidFeeBps);
            }
            if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
                panic_with_error!(&env, SettlementError::InvalidFeeBps);
            }
            if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
                panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
            }

            let prev = persistent_get_safe::<_, SettlementRule>(
                &env,
                &DataKey::Rule(merchant.clone()),
            )
            .unwrap_or_else(|| read_rule_or_default(&env, merchant.clone()));

            let key = DataKey::Rule(merchant.clone());
            env.storage().persistent().set(&key, &rule);

            env.storage()
                .persistent()
                .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

            env.events().publish(
                (Symbol::new(&env, "settlement_rule_updated"), merchant),
                (admin, prev, rule),
            );
        }

        pub fn clear_settlement_rule(env: Env, signers: Vec<Address>, merchant: Address) {
            assert_not_paused(&env);
            verify_admin_auth(&env, &signers, read_threshold(&env));
            let admin = signers.get(0).unwrap();

            let key = DataKey::Rule(merchant.clone());
            let removed = persistent_get_safe::<_, SettlementRule>(&env, &key)
                .unwrap_or_else(|| panic_with_error!(&env, SettlementError::MerchantRuleNotSet));

            env.storage().persistent().remove(&key);

            let fallback = read_rule_or_default(&env, merchant.clone());

            env.events().publish(
                (Symbol::new(&env, "settlement_rule_cleared"), merchant),
                (admin, removed, fallback),
            );
        }

        pub fn set_default_rule(env: Env, signers: Vec<Address>, new_rule: SettlementRule) {
            assert_not_paused(&env);
            verify_admin_auth(&env, &signers, read_threshold(&env));
            let admin = signers.get(0).unwrap();

            validate_fee_against_governance(&env, &new_rule);

            if new_rule.platform_fee_bps > BPS_DENOMINATOR || new_rule.network_fee_bps > BPS_DENOMINATOR
            {
                panic_with_error!(&env, SettlementError::InvalidFeeBps);
            }
            if new_rule.platform_fee_bps < MIN_FEE_BPS || new_rule.network_fee_bps < MIN_FEE_BPS {
                panic_with_error!(&env, SettlementError::InvalidFeeBps);
            }
            if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
                panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
            }

            let prev = persistent_get_safe::<_, SettlementRule>(&env, &DataKey::DefaultRule)
                .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

            env.storage()
                .persistent()
                .set(&DataKey::DefaultRule, &new_rule);
            env.storage().persistent().extend_ttl(
                &DataKey::DefaultRule,
                RULE_TTL_THRESHOLD,
                RULE_TTL_BUMP,
            );

            env.events().publish(
                (Symbol::new(&env, "default_rule_updated"),),
                (admin, prev, new_rule),
            );
        }

        /// Returns the global default settlement rule, if one has been set.
        /// Automatically extends the persistent storage TTL to prevent archival
        /// during public read queries (clausal to TTL eviction).
        pub fn get_default_rule(env: Env) -> Option<SettlementRule> {
            let key = DataKey::DefaultRule;
            match persistent_get_safe::<_, SettlementRule>(&env, &key) {
                Some(rule) => {
                    env.storage()
                        .persistent()
                        .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
                    Some(rule)
                }
                None => None,
            }
        }

        /// Returns the merchant-specific settlement rule, if one has been set.
        /// Automatically extends the persistent storage TTL to prevent archival.
        pub fn get_settlement_rule(env: Env, merchant: Address) -> Option<SettlementRule> {
            let key = DataKey::Rule(merchant);

            if let Some(rule) = persistent_get_safe::<_, SettlementRule>(&env, &key) {
                // Extend the TTL using the same named constants as set_settlement_rule
                // so the read and write paths never drift apart if the policy changes.
                env.storage()
                    .persistent()
                    .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

                Some(rule)
            } else {
                None
            }
        }

}
