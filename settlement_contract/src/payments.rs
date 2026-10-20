use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env, Symbol, Vec};

use bettapay_common::{constants::BPS_DENOMINATOR, events};

use crate::errors::SettlementError;
use crate::policy::{
    BOOTSTRAP_DEFAULT_RULE, MAX_PAYMENTS_BATCH, MAX_PAYMENT_REFERENCE_LEN, PAYMENT_TTL_BUMP,
    PAYMENT_TTL_THRESHOLD,
};
use crate::storage::{
    assert_not_paused, assert_payments_readable, decrement_payment_count,
    increment_payment_count, is_merchant_registered_and_bump_ttl, is_merchant_registered_internal,
    read_min_payment_amount, read_payment_count, read_rule_or_default,
    read_rule_or_default_no_bump, read_threshold, verify_admin_auth,
};
use crate::types::{Bps, DataKey, FeeSplit, PaymentRecord, PaymentsPage, SettlementRule};
use crate::{SettlementContract, SettlementContractClient};

/// Computes the platform, network, and merchant fee amounts for an amount using ceil-based rounding.
///
/// # Known edge case: clamping merchant payout
///
/// Ceiling rounding of both fees independently can make
/// `platform_fee_amount + network_fee_amount > amount` for small gross amounts
/// (e.g. `amount = 1`, `platform_fee_bps = 5000`, `network_fee_bps = 5000`).
/// This yields a negative subtraction remainder. The policy is to clamp the
/// `merchant_payout_amount` to zero, ensuring fees are not under-collected but the
/// merchant never owes a negative balance for a settlement.
fn calculate_split(env: &Env, amount: i128, rule: &SettlementRule) -> FeeSplit {
    let denom = BPS_DENOMINATOR as i128;
    let platform_bps = rule.platform_bps();
    let network_bps = rule.network_bps();

    // Guard against `amount * bps + (denom - 1)` overflowing i128 before it is attempted below,
    // so callers get a readable AmountOverflow error instead of a raw arithmetic-overflow panic.
    // Checked arithmetic keeps the guard unconditional, including when both fee legs are zero,
    // and checks the ceil-rounding adjustment as well as the multiplication at the boundary.
    let max_bps = core::cmp::max(platform_bps.as_i128(), network_bps.as_i128());
    if amount
        .checked_mul(max_bps)
        .and_then(|numerator| numerator.checked_add(denom - 1))
        .is_none()
    {
        panic_with_error!(env, SettlementError::AmountOverflow);
    }
    // The combined fee rate must also fit, so an overflow of the fee sum maps
    // deterministically to AmountOverflow rather than a later arithmetic trap.
    let sum_bps = platform_bps
        .as_i128()
        .checked_add(network_bps.as_i128())
        .unwrap_or(i128::MAX);
    if amount
        .checked_mul(sum_bps)
        .and_then(|numerator| numerator.checked_add(denom - 1))
        .is_none()
    {
        panic_with_error!(env, SettlementError::AmountOverflow);
    }

    // Integer arithmetic is used instead of floats to ensure deterministic, reproducible smart contract execution.
    // Standard integer division (`/`) truncates fractions toward zero, causing precision loss and under-collecting fees.
    // To prevent fee under-collection, ceiling division is simulated by adding `BPS_DENOMINATOR - 1` to the numerator.
    // Edge case: For small amounts, ceil rounding can force fees to 1 unit even when the basis points represent a tiny fraction.
    let platform_fee_amount = platform_bps
        .calculate_fee_ceil(amount)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::AmountOverflow));
    let mut network_fee_amount = network_bps
        .calculate_fee_ceil(amount)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::AmountOverflow));

    // Ceil-rounded fees can sum to more than the gross for tiny amounts with
    // high fee configs. Clamp the network leg so total fees never exceed the
    // gross, keeping the accounting equation balanced (issue #683). An
    // overflowing sum is treated as exceeding the gross.
    let fees_exceed_gross = platform_fee_amount
        .checked_add(network_fee_amount)
        .is_none_or(|total| total > amount);
    if fees_exceed_gross {
        network_fee_amount = amount.checked_sub(platform_fee_amount).unwrap_or(0).max(0);
    }

    let merchant_payout_amount = amount
        .checked_sub(platform_fee_amount)
        .and_then(|remainder| remainder.checked_sub(network_fee_amount))
        .unwrap_or(0)
        .max(0);
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_payout_amount,
    }
}

/// Returns true if `rule` matches the bootstrap default settlement rule across all fields.
fn is_bootstrap_rule(rule: &SettlementRule) -> bool {
    rule.platform_fee_bps == BOOTSTRAP_DEFAULT_RULE.platform_fee_bps
        && rule.network_fee_bps == BOOTSTRAP_DEFAULT_RULE.network_fee_bps
        && rule.settlement_delay_ledger == BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
        && rule.auto_settle == BOOTSTRAP_DEFAULT_RULE.auto_settle
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::{prop_assert, prop_assert_eq, prop_assume, proptest};

    fn fee_test_amounts() -> impl proptest::strategy::Strategy<Value = i128> {
        prop_oneof![
            Just(0i128),
            Just(1i128),
            Just(crate::MIN_PAYMENT_AMOUNT),
            Just(i128::MIN),
            Just(i128::MAX),
            1i128..=1_000_000_000i128,
        ]
    }

    #[test]
    fn is_bootstrap_rule_identifies_matching_and_divergent_rules() {
        assert!(is_bootstrap_rule(&BOOTSTRAP_DEFAULT_RULE));

        let mut modified = BOOTSTRAP_DEFAULT_RULE;
        modified.platform_fee_bps += 1;
        assert!(!is_bootstrap_rule(&modified));

        let mut modified = BOOTSTRAP_DEFAULT_RULE;
        modified.network_fee_bps += 1;
        assert!(!is_bootstrap_rule(&modified));

        let mut modified = BOOTSTRAP_DEFAULT_RULE;
        modified.settlement_delay_ledger += 1;
        assert!(!is_bootstrap_rule(&modified));

        let mut modified = BOOTSTRAP_DEFAULT_RULE;
        modified.auto_settle = !BOOTSTRAP_DEFAULT_RULE.auto_settle;
        assert!(!is_bootstrap_rule(&modified));
    }

    #[test]
    fn zero_fee_split_handles_maximum_amount() {
        let env = Env::default();
        let amount = i128::MAX;
        let rule = SettlementRule {
            platform_fee_bps: 0,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        let split = calculate_split(&env, amount, &rule);

        assert_eq!(split.gross_amount, amount);
        assert_eq!(split.platform_fee_amount, 0);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_payout_amount, amount);
        assert_eq!(
            split.platform_fee_amount + split.network_fee_amount + split.merchant_payout_amount,
            split.gross_amount,
        );
    }

    fn rule(platform_fee_bps: u32, network_fee_bps: u32) -> SettlementRule {
        SettlementRule {
            platform_fee_bps,
            network_fee_bps,
            settlement_delay_ledger: 0,
            auto_settle: false,
        }
    }

    #[test]
    // See FEE_ROUNDING.md for the documented ceil direction and per-leg bound.
    fn calculate_fee_ceil_matches_expected_values() {
        for (amount, bps, expected) in [
            (1i128, 1u32, 1i128),
            (199, 100, 2),
            (10_000, 250, 250),
            (1_000_000_007, 9_999, 999_900_007),
        ] {
            assert_eq!(Bps::new(bps).calculate_fee_ceil(amount), Some(expected));
        }
    }

    #[test]
    fn zero_bps_fee_is_some_zero() {
        assert_eq!(Bps::new(0).calculate_fee_ceil(0), Some(0));
        assert_eq!(Bps::new(0).calculate_fee_ceil(i128::MAX), Some(0));
    }

    #[test]
    fn calculate_fee_ceil_returns_none_on_overflow() {
        assert_eq!(
            Bps::new(BPS_DENOMINATOR).calculate_fee_ceil(i128::MAX),
            None
        );
        assert_eq!(Bps::new(1).calculate_fee_ceil(i128::MAX), None);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #310)")]
    fn full_bps_fee_leg_overflow_panics_with_amount_overflow() {
        let env = Env::default();
        calculate_split(&env, i128::MAX, &rule(BPS_DENOMINATOR, 0));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #310)")]
    fn fee_sum_overflow_panics_even_when_each_leg_fits() {
        let env = Env::default();
        // 6000 bps alone fits, but the combined 10000 bps does not.
        let amount = i128::MAX / 8_000;
        calculate_split(&env, amount, &rule(6_000, 4_000));
    }

    #[test]
    fn largest_amount_passing_sum_guard_splits_without_trapping() {
        let env = Env::default();
        let denom = BPS_DENOMINATOR as i128;
        let amount = (i128::MAX - (denom - 1)) / denom;

        let split = calculate_split(&env, amount, &rule(6_000, 4_000));

        assert_eq!(split.gross_amount, amount);
        assert_eq!(
            split.platform_fee_amount + split.network_fee_amount + split.merchant_payout_amount,
            amount,
        );
        assert!(split.merchant_payout_amount >= 0);
    }

    #[test]
    // See FEE_ROUNDING.md for the network clamp and non-negative merchant rule.
    fn tiny_amount_with_full_fees_clamps_network_and_merchant_to_zero() {
        let env = Env::default();
        let split = calculate_split(&env, 1, &rule(5_000, 5_000));

        assert_eq!(split.platform_fee_amount, 1);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_payout_amount, 0);
    }

    #[test]
    fn balanced_split_is_unchanged() {
        let env = Env::default();
        let split = calculate_split(&env, 10_000, &rule(250, 50));

        assert_eq!(split.platform_fee_amount, 250);
        assert_eq!(split.network_fee_amount, 50);
        assert_eq!(split.merchant_payout_amount, 9_700);
    }

    proptest! {
        #[test]
        fn split_matches_ceil_arithmetic_and_never_negative_merchant(
            amount in 1i128..=1_000_000_000,
            platform_fee_bps in 0u32..=10_000,
            network_fee_bps in 0u32..=10_000,
        ) {
            let env = Env::default();
            let rule = SettlementRule {
                platform_fee_bps,
                network_fee_bps,
                settlement_delay_ledger: 0,
                auto_settle: false,
            };

            let split = calculate_split(&env, amount, &rule);
            let denom = BPS_DENOMINATOR as i128;
            let expected_platform =
                (amount * platform_fee_bps as i128 + denom - 1) / denom;
            let mut expected_network =
                (amount * network_fee_bps as i128 + denom - 1) / denom;
            // Mirror the implementation's clamp (issue #683): when the two
            // ceil-rounded fee legs would exceed the gross, the network leg is
            // clamped so total fees never exceed the amount.
            if expected_platform + expected_network > amount {
                expected_network = amount - expected_platform;
            }
            let _expected_merchant =
                (amount - expected_platform - expected_network).max(0);

            prop_assert_eq!(split.gross_amount, amount);
            // Clamp network leg so total fees never exceed gross (issue #683).
            let clamped_network = if expected_platform + expected_network > amount {
                (amount - expected_platform).max(0)
            } else {
                expected_network
            };
            let clamped_merchant = (amount - expected_platform - clamped_network).max(0);

            prop_assert_eq!(split.platform_fee_amount, expected_platform);
            prop_assert_eq!(split.network_fee_amount, clamped_network);
            prop_assert_eq!(split.merchant_payout_amount, clamped_merchant);
            prop_assert_eq!(
                split.platform_fee_amount
                    + split.network_fee_amount
                    + split.merchant_payout_amount,
                split.gross_amount,
            );
            prop_assert!(split.merchant_payout_amount >= 0);
        }

        #[test]
        fn fee_ceiling_is_monotonic_for_platform_and_network_legs(
            platform_rate in 0u32..=BPS_DENOMINATOR,
            network_rate in 0u32..=BPS_DENOMINATOR,
            amount_a in fee_test_amounts(),
            amount_b in fee_test_amounts(),
        ) {
            let (amount_a, amount_b) = if amount_a <= amount_b {
                (amount_a, amount_b)
            } else {
                (amount_b, amount_a)
            };
            for rate in [platform_rate, network_rate] {
                let fee_a = Bps::new(rate).calculate_fee_ceil(amount_a);
                let fee_b = Bps::new(rate).calculate_fee_ceil(amount_b);
                prop_assume!(fee_a.is_some() && fee_b.is_some());
                prop_assert!(fee_a.unwrap() <= fee_b.unwrap());
                if rate == 0 {
                    prop_assert_eq!(fee_a, Some(0));
                    prop_assert_eq!(fee_b, Some(0));
                }
            }
        }

        #[test]
        fn clamped_network_leg_is_monotonic_in_gross_amount(
            platform_fee_bps in 0u32..=BPS_DENOMINATOR,
            network_fee_bps in 0u32..=BPS_DENOMINATOR,
            amount_a in 1i128..=1_000_000_000i128,
            amount_b in 1i128..=1_000_000_000i128,
        ) {
            let (amount_a, amount_b) = if amount_a <= amount_b {
                (amount_a, amount_b)
            } else {
                (amount_b, amount_a)
            };
            let env = Env::default();
            let rule = rule(platform_fee_bps, network_fee_bps);
            let split_a = calculate_split(&env, amount_a, &rule);
            let split_b = calculate_split(&env, amount_b, &rule);

            prop_assert!(split_a.network_fee_amount <= split_b.network_fee_amount);
        }

        #[test]
        fn zero_fee_legs_preserve_the_gross_amount(
            amount in 0i128..=i128::MAX,
        ) {
            let env = Env::default();
            let rule = SettlementRule {
                platform_fee_bps: 0,
                network_fee_bps: 0,
                settlement_delay_ledger: 0,
                auto_settle: false,
            };

            let split = calculate_split(&env, amount, &rule);

            prop_assert_eq!(split.platform_fee_amount, 0);
            prop_assert_eq!(split.network_fee_amount, 0);
            prop_assert_eq!(split.merchant_payout_amount, amount);
            prop_assert_eq!(
                split.platform_fee_amount + split.network_fee_amount + split.merchant_payout_amount,
                amount,
            );
        }

        #[test]
        fn extreme_fees_clamp_merchant_payout_to_zero(
            amount in 1i128..=10,
        ) {
            let env = Env::default();
            let rule = SettlementRule {
                platform_fee_bps: 5000,
                network_fee_bps: 5000,
                settlement_delay_ledger: 0,
                auto_settle: false,
            };

            let split = calculate_split(&env, amount, &rule);

            prop_assert!(split.platform_fee_amount > 0);
            prop_assert_eq!(
                split.platform_fee_amount + split.network_fee_amount,
                amount,
                "extreme fees must collect the full gross (issue #683)"
            );
            // network_fee may be clamped to 0 when total fees exceed gross
            // (issue #683), but merchant must always be non-negative.
            prop_assert!(split.network_fee_amount >= 0);
            prop_assert_eq!(split.merchant_payout_amount, 0);
        }
    }
}

/// Enforces the uniform payment-read auth policy (issue #559): the caller
/// must be either the owning merchant or an admin.
///
/// The SDK version pinned by this workspace has no `env.caller()`, so caller
/// identity is proven the same way every other privileged path in this
/// contract proves it:
///
/// * an **empty** `signers` list means the caller is acting as the merchant,
///   and [`Address::require_auth`] on the merchant proves the caller controls
///   it;
/// * a **non-empty** `signers` list means the caller is acting as an admin,
///   and [`verify_admin_auth`] proves those signers satisfy the admin
///   threshold.
///
/// There is deliberately no `no check` path: a caller who is neither the
/// merchant nor an admin is rejected either by the auth framework (owner
/// path) or by `verify_admin_auth`'s admin-membership check (admin path).
fn assert_read_authorized(env: &Env, merchant: &Address, signers: &Vec<Address>) {
    if signers.is_empty() {
        merchant.require_auth();
    } else {
        verify_admin_auth(env, signers, read_threshold(env));
    }
}

#[contractimpl]
impl SettlementContract {
    /// Store a payment reference for a merchant and calculate the fee split.
    ///
    /// `reference` is fixed at [`MAX_PAYMENT_REFERENCE_LEN`] bytes (`BytesN<32>`).
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`InvalidPaymentReference`](SettlementError::InvalidPaymentReference) — if `reference` is all zeros.
    /// * [`AmountTooSmall`](SettlementError::AmountTooSmall) — if `amount` is below the minimum.
    /// * [`DuplicatePaymentReference`](SettlementError::DuplicatePaymentReference) — if the reference already exists for this merchant.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`
    ///   for either fee leg or for the combined platform + network fee rate.
    ///
    /// ## Emitted Event: `payment_stored`
    ///
    /// **Topics**: `(Symbol("payment_stored"), Address merchant, BytesN<32> reference)`
    /// **Data**: `PaymentRecord`
    ///
    /// The full fee split snapshot (gross amount, platform fee, network fee,
    /// merchant amount) is carried directly on the `PaymentRecord` in this
    /// event's data, so indexers do not need a separate state read to see
    /// the split; no separate split event is emitted.
    pub fn store_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        amount: i128,
    ) -> FeeSplit {
        // Validate cheap, non-storage input first (issue #774): an all-zero
        // reference is always invalid, so reject it before any storage reads
        // (merchant lookup) or auth checks to avoid wasting gas on invalid input.
        if reference == BytesN::from_array(&env, &[0; MAX_PAYMENT_REFERENCE_LEN as usize]) {
            panic_with_error!(&env, SettlementError::InvalidPaymentReference);
        }
        assert_not_paused(&env);

        merchant.require_auth();
        if !is_merchant_registered_and_bump_ttl(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        let min_amount = read_min_payment_amount(&env);
        if amount < min_amount {
            panic_with_error!(&env, SettlementError::AmountTooSmall);
        }

        // Reference uniqueness is scoped to the merchant: the same reference
        // may be used by two different merchants, so the key carries the
        // merchant alongside the reference (issue #493). A duplicate is only
        // a duplicate for the same merchant.
        let payment_key = DataKey::Payment(merchant.clone(), reference.clone());
        if env.storage().persistent().has(&payment_key) {
            panic_with_error!(&env, SettlementError::DuplicatePaymentReference);
        }

        // ISSUE 495: Reentrancy guard.
        // We write a dummy record to storage immediately so that if the external
        // read_governance_fee_rule call results in a reentrant call back to this
        // contract, the `has` check above will catch it. This dummy record is
        // overwritten by the actual record at the end of this function.
        let dummy_record = PaymentRecord {
            merchant: merchant.clone(),
            amount: 0,
            platform_fee_amount: 0,
            network_fee_amount: 0,
            merchant_payout_amount: 0,
            platform_fee_bps: 0,
            network_fee_bps: 0,
            ledger: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
            version: 1,
        };
        env.storage().persistent().set(&payment_key, &dummy_record);

        let rule = read_rule_or_default(&env, merchant.clone());
        if is_bootstrap_rule(&rule) {
            env.events().publish(
                (Symbol::new(&env, events::BOOTSTRAP_FALLBACK_EVENT),),
                BOOTSTRAP_DEFAULT_RULE,
            );
        }
        let split = calculate_split(&env, amount, &rule);
        let record = PaymentRecord {
            merchant: merchant.clone(),
            amount,
            platform_fee_amount: split.platform_fee_amount,
            network_fee_amount: split.network_fee_amount,
            merchant_payout_amount: split.merchant_payout_amount,
            platform_fee_bps: rule.platform_fee_bps,
            network_fee_bps: rule.network_fee_bps,
            ledger: env.ledger().sequence(),
            settlement_delay_ledger: rule.settlement_delay_ledger,
            auto_settle: rule.auto_settle,
            version: 1,
        };

        env.storage().persistent().set(&payment_key, &record);
        env.storage().persistent().extend_ttl(
            &payment_key,
            PAYMENT_TTL_THRESHOLD,
            PAYMENT_TTL_BUMP,
        );
        increment_payment_count(&env, &merchant);

        env.events().publish(
            (
                Symbol::new(&env, events::PAYMENT_STORED_EVENT),
                merchant.clone(),
                reference.clone(),
            ),
            record,
        );

        split
    }

    /// Calculate the fee split for a given merchant and amount without storing a payment reference.
    ///
    /// # Panics
    ///
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`AmountTooSmall`](SettlementError::AmountTooSmall) — if `amount` is below the minimum.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`
    ///   for either fee leg or for the combined platform + network fee rate.
    pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        let min_amount = read_min_payment_amount(&env);
        if amount < min_amount {
            panic_with_error!(env, SettlementError::AmountTooSmall);
        }
        // Issue #760 — TTL-neutral rule read: this entry point is callable by
        // anyone, so the rule must not be kept alive by unauthenticated fee
        // queries. Fee values are unchanged — the no-bump reader resolves the
        // identical rule through the same fallback chain.
        let rule = read_rule_or_default_no_bump(&env, merchant);
        calculate_split(&env, amount, &rule)
    }

    /// Remove a payment reference after the configured admin threshold has
    /// authorized the operation. Removing a missing record is intentionally a
    /// no-op so maintenance callers can safely retry cleanup work.
    pub fn prune_payment(
        env: Env,
        signers: Vec<Address>,
        merchant: Address,
        reference: BytesN<32>,
    ) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let key = DataKey::Payment(merchant.clone(), reference);
        if env.storage().persistent().has(&key) {
            env.storage().persistent().remove(&key);
            decrement_payment_count(&env, &merchant, 1);
        }
    }

    /// Remove several of one merchant's payment references in a single call
    /// after the configured admin threshold has authorized the operation.
    ///
    /// Issue #983: the bounded batch form of [`prune_payment`](Self::prune_payment).
    /// References that are not stored (never existed, already pruned, or
    /// repeated within `refs`) are skipped as no-ops, so a cleanup can be
    /// retried safely. One `payments_pruned` event is emitted per call with
    /// topics `(Symbol("payments_pruned"), merchant)` and data
    /// `(admin, removed_count)`, where `removed_count` is the number of
    /// records actually deleted. There are no accounting or refund side effects.
    ///
    /// # Panics
    ///
    /// * [`BatchTooLarge`](SettlementError::BatchTooLarge) — if `refs` exceeds
    ///   [`MAX_PAYMENTS_BATCH`].
    /// * Auth failure — if the signers do not satisfy the admin threshold.
    pub fn prune_payments(
        env: Env,
        signers: Vec<Address>,
        merchant: Address,
        refs: Vec<BytesN<32>>,
    ) -> u32 {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if refs.len() > MAX_PAYMENTS_BATCH {
            panic_with_error!(&env, SettlementError::BatchTooLarge);
        }

        let mut removed: u32 = 0;
        for reference in refs.iter() {
            let key = DataKey::Payment(merchant.clone(), reference);
            if env.storage().persistent().has(&key) {
                env.storage().persistent().remove(&key);
                removed += 1;
            }
        }

        decrement_payment_count(&env, &merchant, removed);

        let admin = signers.get(0).unwrap();
        env.events().publish(
            (
                Symbol::new(&env, events::PAYMENTS_PRUNED_EVENT),
                merchant,
            ),
            (admin, removed),
        );
        removed
    }

    /// Retrieve a payment record for a merchant by its reference, extending
    /// the storage TTL if found.
    ///
    /// The reference is resolved within the merchant's own namespace, so the
    /// same reference held by a different merchant is not returned.
    /// Pass an empty `signers` vector to authorize as the merchant, or a
    /// non-empty vector of admin signers to authorize through the configured
    /// admin threshold. The reference identifies the record but does not grant
    /// access by itself.
    ///
    /// # Panics
    ///
    /// * Auth failure — if the caller is not the merchant who owns the
    ///   record. Reads are gated behind the merchant's own authorization so
    ///   the gross/fee/net amounts cannot be probed by anyone who can guess
    ///   a reference (issue #492).
    ///
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is
    ///   neither the merchant nor an authorized admin signer set.
    /// * [`PaymentOrphaned`](SettlementError::PaymentOrphaned) — if the
    ///   merchant was unregistered, its payment records are orphaned and no
    ///   longer readable (issue #490).
    pub fn get_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        signers: Vec<Address>,
    ) -> Option<PaymentRecord> {
        assert_read_authorized(&env, &merchant, &signers);
        assert_payments_readable(&env, &merchant);
        let key = DataKey::Payment(merchant, reference);
        let record: Option<PaymentRecord> = env.storage().persistent().get(&key);
        if record.is_some() {
            // `extend_ttl` only writes when the current TTL is below
            // `threshold`, so this has the same externally observable
            // behavior as a manual get_ttl-then-extend check, without
            // depending on `get_ttl`, which is test-only in production code.
            env.storage()
                .persistent()
                .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
        }
        record
    }

    /// Retrieve multiple payment records for a merchant by a vector of references.
    ///
    /// `get_payments` cannot list keys; pass refs observed from `payment_stored` events.
    ///
    /// Soroban cannot enumerate persistent storage keys, so this method does not
    /// iterate over all stored payments. Callers **must supply references**
    /// observed from `payment_stored` events (or other indexer-sourced sources).
    /// Each reference is then looked up individually within the merchant's own
    /// namespace, and the returned vector contains only records that exist.
    ///
    /// Example indexer flow:
    /// 1. Listen for `payment_stored` events with topics
    ///    `(Symbol("payment_stored"), merchant, reference)`.
    /// 2. Collect the `reference: BytesN<32>` values for the merchant of interest.
    /// 3. Call `get_payments(merchant, refs, signers)` with those references to
    ///    fetch the corresponding `PaymentRecord`s.
    ///
    /// References are resolved within the merchant's own namespace and the
    /// returned page's `records` contains only references that exist.
    /// Pass an empty `signers` vector to authorize as the merchant, or a
    /// non-empty vector of admin signers to authorize through the configured
    /// admin threshold.
    ///
    /// ## Pagination (issue #982)
    ///
    /// Soroban cannot enumerate storage keys, so pagination is driven by the
    /// caller's own `refs` vector rather than server-side merchant state:
    /// `cursor` is an opaque position the caller echoes back on the next
    /// call, and the cap is enforced on `refs` itself, exactly as before.
    /// `PaymentsPage::next_cursor` is `Some(cursor + refs.len())` whenever
    /// this call's `refs` filled a full page (length == `MAX_PAYMENTS_BATCH`,
    /// signalling the caller's own reference set may extend further), and
    /// `None` once a shorter page confirms there is nothing left to resolve.
    /// Pass `cursor = 0` for a single, non-paginated call.
    ///
    /// # Panics
    ///
    /// * Auth failure — if the caller is not the merchant who owns the
    ///   records (issue #492).
    ///
    ///   Since issue #559 the same entry point also allows an admin: pass a
    ///   non-empty `signers` list to authenticate as an admin.
    /// * [`PaymentOrphaned`](SettlementError::PaymentOrphaned) — if the
    ///   merchant was unregistered, its payment records are orphaned and no
    ///   longer readable (issue #490).
    /// * [`BatchTooLarge`](SettlementError::BatchTooLarge) — if `refs` exceeds
    ///   [`MAX_PAYMENTS_BATCH`].
    pub fn get_payments(
        env: Env,
        merchant: Address,
        refs: Vec<BytesN<32>>,
        cursor: u32,
        signers: Vec<Address>,
    ) -> PaymentsPage {
        assert_read_authorized(&env, &merchant, &signers);
        assert_payments_readable(&env, &merchant);
        if refs.len() > MAX_PAYMENTS_BATCH {
            panic_with_error!(env, SettlementError::BatchTooLarge);
        }

        let capacity = refs.len().min(MAX_PAYMENTS_BATCH) as usize;
        let mut records = alloc::vec::Vec::with_capacity(capacity);
        for reference in refs.iter() {
            let key = DataKey::Payment(merchant.clone(), reference);
            if let Some(payment) = env.storage().persistent().get::<_, PaymentRecord>(&key) {
                // Match `get_payment_reference`'s TTL maintenance so indexers
                // that exclusively use the batch API don't have their
                // payments silently expire (issue #703). `extend_ttl` only
                // writes when the current TTL is below `threshold`.
                env.storage().persistent().extend_ttl(
                    &key,
                    PAYMENT_TTL_THRESHOLD,
                    PAYMENT_TTL_BUMP,
                );
                records.push(payment);
            }
        }

        let next_cursor = if refs.len() == MAX_PAYMENTS_BATCH {
            Some(cursor + refs.len())
        } else {
            None
        };

        PaymentsPage {
            records: Vec::from_slice(&env, &records),
            next_cursor,
        }
    }

    /// Returns the number of payment records currently stored for `merchant`,
    /// matching [`MAX_PAYMENTS_PER_MERCHANT`](crate::policy::MAX_PAYMENTS_PER_MERCHANT)
    /// usage (issue #980).
    ///
    /// Enforces the same read authorization as [`Self::get_payments`]: pass an
    /// empty `signers` vector to authorize as the merchant, or a non-empty
    /// vector of admin signers to authorize through the configured admin
    /// threshold. A registered merchant that has never stored a payment
    /// returns zero.
    ///
    /// # Panics
    ///
    /// * Auth failure — if the caller is neither the merchant nor an
    ///   authorized admin signer set.
    /// * [`PaymentOrphaned`](SettlementError::PaymentOrphaned) — if the
    ///   merchant was unregistered, its payment records are orphaned and no
    ///   longer readable (issue #490).
    pub fn get_payment_count(env: Env, merchant: Address, signers: Vec<Address>) -> u32 {
        assert_read_authorized(&env, &merchant, &signers);
        assert_payments_readable(&env, &merchant);
        read_payment_count(&env, &merchant)
    }
}

#[cfg(test)]
mod read_authorization_tests {
    use super::SettlementContractClient;
    use crate::tests::setup;
    use soroban_sdk::{BytesN, Error};

    #[test]
    fn duplicate_admin_signers_rejected_on_read() {
        let (env, client, admins, merchant) = setup();
        client.register_merchant(&admins, &merchant);
        let reference = BytesN::from_array(&env, &[1; 32]);
        client.store_payment_reference(&merchant, &reference, &1_000);
        let admin = admins.get(0).unwrap();
        let duplicate_signers = soroban_sdk::vec![&env, admin.clone(), admin];

        assert!(matches!(
            client.try_get_payment_reference(&merchant, &reference, &duplicate_signers),
            Err(Ok(e)) if e == Error::from_contract_error(3)
        ));
    }

    #[test]
    fn calculate_fee_split_rejects_fee_sum_overflow_with_amount_overflow() {
        let (_env, client, admins, merchant) = setup();
        client.register_merchant(&admins, &merchant);
        client.set_settlement_rule(
            &admins,
            &merchant,
            &crate::types::SettlementRule {
                platform_fee_bps: 5_000,
                network_fee_bps: 5_000,
                settlement_delay_ledger: 0,
                auto_settle: false,
            },
        );

        // Each 5000 bps leg fits in i128, but the combined 10000 bps does not.
        let amount = i128::MAX / 7_500;
        assert!(matches!(
            client.try_calculate_fee_split(&merchant, &amount),
            Err(Ok(e)) if e == Error::from_contract_error(310)
        ));
    }

    #[test]
    fn one_admin_signer_satisfies_threshold_one_on_read() {
        let (env, client, admins, merchant) = setup();
        client.register_merchant(&admins, &merchant);
        let reference = BytesN::from_array(&env, &[2; 32]);
        client.store_payment_reference(&merchant, &reference, &1_000);

        assert!(client
            .get_payment_reference(&merchant, &reference, &admins)
            .is_some());
    }
}
