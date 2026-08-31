use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS},
    events,
};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_and_bump_ttl, read_fallback_rule,
    read_rule_or_default, read_threshold, validate_fee_against_governance, verify_admin_auth,
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

        if !is_merchant_registered_and_bump_ttl(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps > MAX_FEE_BPS || rule.network_fee_bps > MAX_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(&env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);

        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (
                Symbol::new(&env, events::SETTLEMENT_RULE_UPDATED_EVENT),
                merchant,
            ),
            (admin, prev, rule),
        );
    }

    pub fn clear_settlement_rule(env: Env, signers: Vec<Address>, merchant: Address) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        let key = DataKey::Rule(merchant.clone());
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::MerchantRuleNotSet));

        env.storage().persistent().remove(&key);

        // Use the shared fallback chain (default → governance → bootstrap)
        // without emitting a bootstrap_fallback event, so the event payload
        // matches the rule that will actually govern the next payment (issue #689).
        let fallback = read_fallback_rule(&env);

        // Canonical event shape shared with the unregister path (issue #491).
        events::emit_settlement_rule_cleared(&env, &merchant, &admin, &removed, &fallback);
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
        if new_rule.platform_fee_bps > MAX_FEE_BPS || new_rule.network_fee_bps > MAX_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps + new_rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .instance()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .instance()
            .set(&DataKey::DefaultRule, &new_rule);

        env.events().publish(
            (Symbol::new(&env, events::DEFAULT_RULE_UPDATED_EVENT),),
            (admin, prev, new_rule),
        );
    }

    /// Returns the global default settlement rule, if one has been set.
    /// Stored in instance storage so it cannot expire independently of the
    /// contract instance.
    pub fn get_default_rule(env: Env) -> Option<SettlementRule> {
        let key = DataKey::DefaultRule;
        env.storage().instance().get::<_, SettlementRule>(&key)
    }

    /// Returns the merchant-specific settlement rule, if one has been set.
    /// Automatically extends the persistent storage TTL to prevent archival.
    pub fn get_settlement_rule(env: Env, merchant: Address) -> Option<SettlementRule> {
        let key = DataKey::Rule(merchant);

        if let Some(rule) = env.storage().persistent().get(&key) {
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
