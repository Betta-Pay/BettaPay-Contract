use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env, Symbol, Vec};

use bettapay_common::{constants::BPS_DENOMINATOR, events};

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, assert_payments_readable, is_merchant_registered_and_bump_ttl,
    is_merchant_registered_internal, read_min_payment_amount, read_rule_or_default,
};
use crate::types::{DataKey, FeeSplit, PaymentRecord, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, MAX_PAYMENTS_BATCH,
    PAYMENT_TTL_BUMP, PAYMENT_TTL_THRESHOLD,
};

/// Computes the platform, network, and merchant fee amounts for an amount using ceil-based rounding.
///
/// # Known edge case: clamping merchant amount
///
/// Ceiling rounding of both fees independently can make
/// `platform_fee_amount + network_fee_amount > amount` for small gross amounts
/// (e.g. `amount = 1`, `platform_fee_bps = 5000`, `network_fee_bps = 5000`).
/// This yields a negative subtraction remainder. The policy is to clamp the
/// `merchant_amount` to zero, ensuring fees are not under-collected but the
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

    // Integer arithmetic is used instead of floats to ensure deterministic, reproducible smart contract execution.
    // Standard integer division (`/`) truncates fractions toward zero, causing precision loss and under-collecting fees.
    // To prevent fee under-collection, ceiling division is simulated by adding `BPS_DENOMINATOR - 1` to the numerator.
    // Edge case: For small amounts, ceil rounding can force fees to 1 unit even when the basis points represent a tiny fraction.
    let platform_fee_amount = platform_bps.calculate_fee_ceil(amount);
    let mut network_fee_amount = network_bps.calculate_fee_ceil(amount);

    // Ceil-rounded fees can sum to more than the gross for tiny amounts with
    // high fee configs. Clamp the network leg so total fees never exceed the
    // gross, keeping the accounting equation balanced (issue #683).
    if platform_fee_amount + network_fee_amount > amount {
        network_fee_amount = amount - platform_fee_amount;
    }

    let merchant_amount = (amount - platform_fee_amount - network_fee_amount).max(0);
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
        assert_eq!(split.merchant_amount, amount);
        assert_eq!(
            split.platform_fee_amount + split.network_fee_amount + split.merchant_amount,
            split.gross_amount,
        );
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
            let expected_network =
                (amount * network_fee_bps as i128 + denom - 1) / denom;
            let expected_merchant =
                (amount - expected_platform - expected_network).max(0);

            prop_assert_eq!(split.gross_amount, amount);
            prop_assert_eq!(split.platform_fee_amount, expected_platform);
            prop_assert_eq!(split.network_fee_amount, expected_network);
            prop_assert_eq!(split.merchant_amount, expected_merchant);
            prop_assert!(split.merchant_amount >= 0);
        }

        #[test]
        fn zero_fee_legs_preserve_the_gross_amount(
            amount in 1i128..=i128::MAX,
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
            prop_assert_eq!(split.merchant_amount, amount);
            prop_assert_eq!(
                split.platform_fee_amount + split.network_fee_amount + split.merchant_amount,
                amount,
            );
        }

        #[test]
        fn extreme_fees_clamp_merchant_amount_to_zero(
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
            prop_assert!(split.network_fee_amount > 0);
            prop_assert_eq!(split.merchant_amount, 0);
        }
    }
}

#[contractimpl]
impl SettlementContract {
    /// Store a payment reference for a merchant and calculate the fee split.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`InvalidPaymentReference`](SettlementError::InvalidPaymentReference) — if `reference` is all zeros.
    /// * [`AmountTooSmall`](SettlementError::AmountTooSmall) — if `amount` is below the minimum.
    /// * [`DuplicatePaymentReference`](SettlementError::DuplicatePaymentReference) — if the reference already exists for this merchant.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    ///
    /// ## Emitted Event: `payment_stored`
    ///
    /// **Topics**: `(Symbol("payment_stored"), Address merchant, BytesN<32> reference)`
    /// **Data**: `()`
    ///
    /// The fee split (platform fee, network fee, merchant amount, gross amount)
    /// is available on the `PaymentRecord` in this event's data; no separate
    /// split event is emitted.
    pub fn store_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        amount: i128,
    ) -> FeeSplit {
        assert_not_paused(&env);

        // This whole call only ever commits if `merchant.require_auth()` below
        // succeeds (a panic reverts every storage change made in this
        // invocation, this TTL bump included), so bumping here — ahead of the
        // auth check — cannot be abused by a non-merchant caller to keep the
        // marker warm: their call fails auth and nothing persists.
        if !is_merchant_registered_and_bump_ttl(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        merchant.require_auth();
        if reference == BytesN::from_array(&env, &[0; 32]) {
            panic_with_error!(&env, SettlementError::InvalidPaymentReference);
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
            merchant_amount: 0,
            platform_fee_bps: 0,
            network_fee_bps: 0,
            ledger: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        env.storage().persistent().set(&payment_key, &dummy_record);

        let rule = read_rule_or_default(&env, merchant.clone());
        let split = calculate_split(&env, amount, &rule);
        let record = PaymentRecord {
            merchant: merchant.clone(),
            amount,
            platform_fee_amount: split.platform_fee_amount,
            network_fee_amount: split.network_fee_amount,
            merchant_amount: split.merchant_amount,
            platform_fee_bps: rule.platform_fee_bps,
            network_fee_bps: rule.network_fee_bps,
            ledger: env.ledger().sequence(),
            settlement_delay_ledger: rule.settlement_delay_ledger,
            auto_settle: rule.auto_settle,
        };

        env.storage().persistent().set(&payment_key, &record);
        env.storage().persistent().extend_ttl(
            &payment_key,
            PAYMENT_TTL_THRESHOLD,
            PAYMENT_TTL_BUMP,
        );

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
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        let min_amount = read_min_payment_amount(&env);
        if amount < min_amount {
            panic_with_error!(env, SettlementError::AmountTooSmall);
        }
        let rule = read_rule_or_default(&env, merchant);
        calculate_split(&env, amount, &rule)
    }

    /// Retrieve a payment record for a merchant by its reference, extending
    /// the storage TTL if found.
    ///
    /// The reference is resolved within the merchant's own namespace, so the
    /// same reference held by a different merchant is not returned.
    /// This read is public: the 32-byte payment reference is the lookup
    /// capability used by indexers and composing contracts.
    ///
    /// # Panics
    ///
    /// * Auth failure — if the caller is not the merchant who owns the
    ///   record. Reads are gated behind the merchant's own authorization so
    ///   the gross/fee/net amounts cannot be probed by anyone who can guess
    ///   a reference (issue #492).
    /// * [`PaymentOrphaned`](SettlementError::PaymentOrphaned) — if the
    ///   merchant was unregistered, its payment records are orphaned and no
    ///   longer readable (issue #490).
    pub fn get_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
    ) -> Option<PaymentRecord> {
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
    /// References are resolved within the merchant's own namespace and the
    /// returned vector contains only records that exist.
    /// This read is public so indexers and composing contracts can verify
    /// known payment references without a merchant signature.
    ///
    /// # Panics
    ///
    /// * Auth failure — if the caller is not the merchant who owns the
    ///   records (issue #492).
    /// * [`PaymentOrphaned`](SettlementError::PaymentOrphaned) — if the
    ///   merchant was unregistered, its payment records are orphaned and no
    ///   longer readable (issue #490).
    /// * [`BatchTooLarge`](SettlementError::BatchTooLarge) — if `refs` exceeds
    ///   [`MAX_PAYMENTS_BATCH`].
    pub fn get_payments(env: Env, merchant: Address, refs: Vec<BytesN<32>>) -> Vec<PaymentRecord> {
        assert_payments_readable(&env, &merchant);
        if refs.len() > MAX_PAYMENTS_BATCH {
            panic_with_error!(env, SettlementError::BatchTooLarge);
        }

        let mut payments = Vec::new(&env);
        for reference in refs.iter() {
            let key = DataKey::Payment(merchant.clone(), reference);
            if let Some(payment) = env.storage().persistent().get::<_, PaymentRecord>(&key) {
                payments.push_back(payment);
            }
        }
        payments
    }
}
