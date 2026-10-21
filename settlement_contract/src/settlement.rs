use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS},
    events,
};

use crate::errors::SettlementError;
use crate::policy::{
    BOOTSTRAP_DEFAULT_RULE, MAX_SETTLEMENT_DELAY_LEDGER, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
};
use crate::storage::{
    assert_admin_and_not_paused, is_merchant_registered_and_bump_ttl, read_fallback_rule,
    read_rule_or_default, read_rule_or_default_no_bump, validate_fee_against_governance,
    validate_nonzero_address,
};
use crate::types::{DataKey, SettlementRule};
use crate::SettlementContract;

/// Field-level bounds shared by the rule setters: each fee within
/// `[MIN_FEE_BPS, MAX_FEE_BPS]` (and never above `BPS_DENOMINATOR`), the fee
/// sum no more than `BPS_DENOMINATOR` (overflow-checked), and the settlement
/// delay no more than `MAX_SETTLEMENT_DELAY_LEDGER`. Checks run in the same
/// order the setters used inline, so the first error reported is unchanged.
///
/// `MIN_FEE_BPS` and `MAX_FEE_BPS` come from `bettapay_common::constants`, the
/// same single source the governance contract's `set_fee_config` uses for
/// `FeeConfig`. Neither contract keeps its own copy, so the per-leg bounds
/// cannot drift apart; `fee_bounds_parity_tests` pins this at each boundary
/// (issue #987).
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
        assert_admin_and_not_paused(&env, &signers);
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
        assert_admin_and_not_paused(&env, &signers);
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
        assert_admin_and_not_paused(&env, &signers);
        let admin = signers.get(0).unwrap();

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

        // Local range checks run before the cross-contract governance ceiling
        // check (issue #803): they're free, so obviously invalid input is
        // rejected without paying for a round-trip into the governance
        // contract first.
        validate_fee_against_governance(&env, &new_rule);

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
    ///
    /// Keep-alive getter for authenticated maintenance flows. Read-only
    /// consumers that must not affect storage lifetime should call
    /// [`get_settlement_rule_no_bump`](SettlementContract::get_settlement_rule_no_bump)
    /// instead (issue #763).
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

    /// TTL-neutral variant of [`get_settlement_rule`](SettlementContract::get_settlement_rule)
    /// (issue #763): returns exactly the same value without extending the
    /// persistent storage TTL, so querying a rule never forces a rent write.
    pub fn get_settlement_rule_no_bump(env: Env, merchant: Address) -> Option<SettlementRule> {
        let key = DataKey::Rule(merchant);
        env.storage().persistent().get::<_, SettlementRule>(&key)
    }

    /// Returns the effective settlement rule for a merchant, applying the full
    /// resolution chain: merchant-specific rule → global default → governance
    /// fee config → bootstrap fallback.
    ///
    /// Unlike [`get_settlement_rule`](SettlementContract::get_settlement_rule) (which returns
    /// `None` when no merchant-specific rule is stored) and [`get_default_rule`](SettlementContract::get_default_rule) (which returns `None` when
    /// no global default is stored), this method always returns a rule — it
    /// follows the same resolution that the write and payment paths use
    /// internally.
    pub fn get_effective_rule(env: Env, merchant: Address) -> SettlementRule {
        // Issue #762 — public effective-rule query must be TTL-neutral so
        // third parties cannot keep a merchant's rule alive via repeated
        // queries. Returns the identical value through the no-bump reader.
        read_rule_or_default_no_bump(&env, merchant)
    }
}

#[cfg(test)]
mod rule_validation_tests {
    use crate::tests::{assert_rule_eq, setup};
    use crate::*;
    use bettapay_common::constants::{BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS};
    use bettapay_common::storage::zero_address;
    use proptest::prelude::*;

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

    fn rule_is_within_bounds(rule: &SettlementRule) -> bool {
        rule.platform_fee_bps >= MIN_FEE_BPS
            && rule.platform_fee_bps <= MAX_FEE_BPS
            && rule.network_fee_bps >= MIN_FEE_BPS
            && rule.network_fee_bps <= MAX_FEE_BPS
            && rule
                .platform_fee_bps
                .checked_add(rule.network_fee_bps)
                .is_some_and(|sum| sum <= BPS_DENOMINATOR)
            && rule.settlement_delay_ledger <= MAX_SETTLEMENT_DELAY_LEDGER
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Random fee pairs and delays cover both setter branches. Accepted
        /// rules are persisted by both direct and timelocked writes; rejected
        /// rules leave the prior value absent from storage.
        #[test]
        fn setter_paths_agree_on_rule_bounds(
            platform_fee_bps in prop_oneof![
                Just(MIN_FEE_BPS),
                Just(MAX_FEE_BPS),
                Just(BPS_DENOMINATOR),
                any::<u32>(),
            ],
            network_fee_bps in prop_oneof![
                Just(MIN_FEE_BPS),
                Just(MAX_FEE_BPS),
                Just(BPS_DENOMINATOR),
                any::<u32>(),
            ],
            settlement_delay_ledger in prop_oneof![
                Just(MAX_SETTLEMENT_DELAY_LEDGER),
                Just(MAX_SETTLEMENT_DELAY_LEDGER + 1),
                any::<u32>(),
            ],
            auto_settle in any::<bool>(),
        ) {
            let candidate = SettlementRule {
                platform_fee_bps,
                network_fee_bps,
                settlement_delay_ledger,
                auto_settle,
            };
            let accepted = rule_is_within_bounds(&candidate);

            let (_direct_env, direct_client, direct_admins, _merchant) = setup();
            let direct_result = direct_client.try_set_default_rule(&direct_admins, &candidate);
            prop_assert_eq!(direct_result.is_ok(), accepted);
            if accepted {
                assert_rule_eq(
                    &direct_client.get_default_rule().unwrap(),
                    &candidate,
                    "direct setter must persist every accepted rule",
                );
            } else {
                prop_assert!(direct_client.get_default_rule().is_none());
            }

            let (timelock_env, timelock_client, timelock_admins, _merchant) = setup();
            let operation = Operation::SetDefaultRule(candidate.clone());
            let scheduled = timelock_client.try_schedule(
                &timelock_admins,
                &operation,
                &DEFAULT_TIMELOCK_DELAY_SECONDS,
            );
            let executed = if scheduled.is_ok() {
                timelock_env.ledger().with_mut(|ledger| {
                    ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS;
                });
                timelock_client.try_execute(&timelock_admins.get(0).unwrap(), &operation).is_ok()
            } else {
                false
            };
            prop_assert_eq!(executed, accepted);
            if accepted {
                assert_rule_eq(
                    &timelock_client.get_default_rule().unwrap(),
                    &candidate,
                    "timelocked setter must persist every accepted rule",
                );
            } else {
                prop_assert!(timelock_client.get_default_rule().is_none());
            }

            let (merchant_env, merchant_client, merchant_admins, merchant) = setup();
            merchant_client.register_merchant(&merchant_admins, &merchant);
            let direct_result = merchant_client.try_set_settlement_rule(
                &merchant_admins,
                &merchant,
                &candidate,
            );
            prop_assert_eq!(direct_result.is_ok(), accepted);
            if accepted {
                assert_rule_eq(
                    &merchant_client.get_settlement_rule(&merchant).unwrap(),
                    &candidate,
                    "direct merchant setter must persist every accepted rule",
                );
            } else {
                prop_assert!(merchant_client.get_settlement_rule(&merchant).is_none());
            }

            let (merchant_timelock_env, merchant_timelock_client, merchant_timelock_admins, merchant) = setup();
            merchant_timelock_client.register_merchant(&merchant_timelock_admins, &merchant);
            let operation = Operation::SetSettlementRule(merchant.clone(), candidate.clone());
            let scheduled = merchant_timelock_client.try_schedule(
                &merchant_timelock_admins,
                &operation,
                &DEFAULT_TIMELOCK_DELAY_SECONDS,
            );
            let executed = if scheduled.is_ok() {
                merchant_timelock_env.ledger().with_mut(|ledger| {
                    ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS;
                });
                merchant_timelock_client
                    .try_execute(&merchant_timelock_admins.get(0).unwrap(), &operation)
                    .is_ok()
            } else {
                false
            };
            prop_assert_eq!(executed, accepted);
            if accepted {
                assert_rule_eq(
                    &merchant_timelock_client
                        .get_settlement_rule(&merchant)
                        .unwrap(),
                    &candidate,
                    "timelocked merchant setter must persist every accepted rule",
                );
            } else {
                prop_assert!(
                    merchant_timelock_client
                        .get_settlement_rule(&merchant)
                        .is_none()
                );
            }
        }
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
