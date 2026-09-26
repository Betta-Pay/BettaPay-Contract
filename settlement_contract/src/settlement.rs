use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS},
    events,
};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_and_bump_ttl, read_fallback_rule,
    read_rule_or_default, read_threshold, validate_fee_against_governance,
    validate_nonzero_address, verify_admin_auth,
};
use crate::types::{DataKey, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, BOOTSTRAP_DEFAULT_RULE,
    MAX_SETTLEMENT_DELAY_LEDGER, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
};

/// Field-level bounds shared by the rule setters: each fee within
/// `[MIN_FEE_BPS, MAX_FEE_BPS]` (and never above `BPS_DENOMINATOR`), the fee
/// sum no more than `BPS_DENOMINATOR` (overflow-checked), and the settlement
/// delay no more than `MAX_SETTLEMENT_DELAY_LEDGER`. Checks run in the same
/// order the setters used inline, so the first error reported is unchanged.
fn validate_rule_bounds(env: &Env, rule: &SettlementRule) {
    if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
        panic_with_error!(env, SettlementError::InvalidFeeBps);
    }
    if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
        panic_with_error!(env, SettlementError::InvalidFeeBps);
    }
    if rule.platform_fee_bps > MAX_FEE_BPS || rule.network_fee_bps > MAX_FEE_BPS {
        panic_with_error!(env, SettlementError::InvalidFeeBps);
    }
    if rule
        .platform_fee_bps
        .checked_add(rule.network_fee_bps)
        .is_none_or(|sum| sum > BPS_DENOMINATOR)
    {
        panic_with_error!(env, SettlementError::InvalidFeeBps);
    }
    if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
        panic_with_error!(env, SettlementError::InvalidSettlementDelay);
    }
}

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

        // Explicit ZeroAddress diagnostic instead of falling through to
        // MerchantMissing on the registration lookup.
        validate_nonzero_address(&env, &merchant, SettlementError::ZeroAddress);
        if !is_merchant_registered_and_bump_ttl(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        validate_rule_bounds(&env, &rule);

        // Local range checks run before the cross-contract governance ceiling
        // check (issue #799): they're free, so obviously invalid input is
        // rejected without paying for a round-trip into the governance
        // contract first.
        validate_fee_against_governance(&env, &rule);

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
        if new_rule
            .platform_fee_bps
            .checked_add(new_rule.network_fee_bps)
            .is_none_or(|sum| sum > BPS_DENOMINATOR)
        {
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

    /// Returns the effective settlement rule for a merchant, applying the full
    /// resolution chain: merchant-specific rule → global default → governance
    /// fee config → bootstrap fallback.
    ///
    /// Unlike [`get_settlement_rule`](Self::get_settlement_rule) (which returns
    /// `None` when no merchant-specific rule is stored) and [`get_default_rule`](Self::get_default_rule) (which returns `None` when
    /// no global default is stored), this method always returns a rule — it
    /// follows the same resolution that the write and payment paths use
    /// internally.
    pub fn get_effective_rule(env: Env, merchant: Address) -> SettlementRule {
        read_rule_or_default(&env, merchant)
    }
}

#[cfg(test)]
mod rule_validation_tests {
    use crate::tests::setup;
    use crate::*;
    use bettapay_common::constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS};
    use soroban_sdk::{Address, Env, String};

    const ZERO_ADDRESS_STRKEY: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn zero_address(env: &Env) -> Address {
        Address::from_string(&String::from_str(env, ZERO_ADDRESS_STRKEY))
    }

    fn rule(platform_fee_bps: u32, network_fee_bps: u32, delay: u32) -> SettlementRule {
        SettlementRule {
            platform_fee_bps,
            network_fee_bps,
            settlement_delay_ledger: delay,
            auto_settle: true,
        }
    }

    fn fee_error() -> Result<(), Result<soroban_sdk::Error, ()>> {
        Err(Ok(soroban_sdk::Error::from_contract_error(
            SettlementError::InvalidFeeBps as u32,
        )))
    }

    // #806
    #[test]
    fn set_settlement_rule_rejects_zero_merchant_with_zero_address() {
        let (env, client, admins, _merchant) = setup();
        let res = client.try_set_settlement_rule(&admins, &zero_address(&env), &rule(250, 50, 7));
        assert_eq!(
            res.map(|_| ()).map_err(|e| e.map_err(|_| ())),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                SettlementError::ZeroAddress as u32
            )))
        );
    }

    #[test]
    fn set_settlement_rule_unregistered_merchant_still_merchant_missing() {
        let (_env, client, admins, merchant) = setup();
        let res = client.try_set_settlement_rule(&admins, &merchant, &rule(250, 50, 7));
        assert_eq!(
            res.map(|_| ()).map_err(|e| e.map_err(|_| ())),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                SettlementError::MerchantMissing as u32
            )))
        );
    }

    // #804 / #807: boundary unchanged — fees summing exactly to the
    // denominator are accepted, each bound is still enforced.
    #[test]
    fn set_settlement_rule_accepts_fee_sum_at_denominator() {
        let (_env, client, admins, merchant) = setup();
        client.register_merchant(&admins, &merchant);
        assert_eq!(MAX_FEE_BPS * 2, BPS_DENOMINATOR);
        let r = rule(MAX_FEE_BPS, MAX_FEE_BPS, 7);
        client.set_settlement_rule(&admins, &merchant, &r);
        let stored = client.get_settlement_rule(&merchant).unwrap();
        assert_eq!(stored.platform_fee_bps, MAX_FEE_BPS);
        assert_eq!(stored.network_fee_bps, MAX_FEE_BPS);
    }

    #[test]
    fn set_settlement_rule_bounds_unchanged() {
        let (_env, client, admins, merchant) = setup();
        client.register_merchant(&admins, &merchant);
        for bad in [
            rule(MIN_FEE_BPS - 1, 50, 7),
            rule(250, MIN_FEE_BPS - 1, 7),
            rule(MAX_FEE_BPS + 1, 50, 7),
            rule(250, MAX_FEE_BPS + 1, 7),
            rule(u32::MAX, u32::MAX, 7),
        ] {
            let res = client.try_set_settlement_rule(&admins, &merchant, &bad);
            assert_eq!(res.map(|_| ()).map_err(|e| e.map_err(|_| ())), fee_error());
        }
        let res = client.try_set_settlement_rule(
            &admins,
            &merchant,
            &rule(250, 50, MAX_SETTLEMENT_DELAY_LEDGER + 1),
        );
        assert_eq!(
            res.map(|_| ()).map_err(|e| e.map_err(|_| ())),
            Err(Ok(soroban_sdk::Error::from_contract_error(
                SettlementError::InvalidSettlementDelay as u32
            )))
        );
    }

    // #805
    #[test]
    fn set_default_rule_accepts_fee_sum_at_denominator() {
        let (_env, client, admins, _merchant) = setup();
        client.set_default_rule(&admins, &rule(MAX_FEE_BPS, MAX_FEE_BPS, 7));
    }

    #[test]
    fn set_default_rule_rejects_out_of_bound_fees() {
        let (_env, client, admins, _merchant) = setup();
        for bad in [rule(MAX_FEE_BPS + 1, 50, 7), rule(u32::MAX, u32::MAX, 7)] {
            let res = client.try_set_default_rule(&admins, &bad);
            assert_eq!(res.map(|_| ()).map_err(|e| e.map_err(|_| ())), fee_error());
        }
    }
}
