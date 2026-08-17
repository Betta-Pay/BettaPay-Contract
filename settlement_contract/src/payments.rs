use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env, Symbol, Vec};

use bettapay_common::constants::BPS_DENOMINATOR;

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_internal, persistent_get_safe, persistent_has_safe,
    read_rule_or_default,
};
use crate::types::{DataKey, FeeSplit, PaymentRecord, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, MIN_PAYMENT_AMOUNT, PAYMENT_TTL_BUMP,
    PAYMENT_TTL_THRESHOLD,
};

/// Computes the platform, network, and merchant fee amounts for an amount using ceil-based rounding.
///
/// # Known edge case: negative merchant amount
///
/// Ceiling rounding of both fees independently can make
/// `platform_fee_amount + network_fee_amount > amount` for small gross amounts
/// (e.g. `amount = 1`, `platform_fee_bps = 5000`, `network_fee_bps = 5000`),
/// which yields a **negative** `merchant_amount`. This is intentional with the
/// current rounding policy (fees are never under-collected); callers must treat
/// a negative merchant payout as a known, documented outcome rather than a bug.
fn calculate_split(env: &Env, amount: i128, rule: &SettlementRule) -> FeeSplit {
    let denom = BPS_DENOMINATOR as i128;
    let platform_bps = rule.platform_bps();
    let network_bps = rule.network_bps();

    // Guard against `amount * bps + (denom - 1)` overflowing i128 before it is attempted below,
    // so callers get a readable AmountOverflow error instead of a raw arithmetic-overflow panic.
    // The `denom - 1` term (the ceil-rounding adjustment) is subtracted from the budget up front
    // so the check stays exact at the boundary instead of leaving a narrow window where the
    // multiplication is "safe" but the following `+ denom - 1` still overflows.
    let max_bps = core::cmp::max(platform_bps.as_i128(), network_bps.as_i128());
    if max_bps > 0 && amount > (i128::MAX - (denom - 1)) / max_bps {
        panic_with_error!(env, SettlementError::AmountOverflow);
    }

    // Integer arithmetic is used instead of floats to ensure deterministic, reproducible smart contract execution.
    // Standard integer division (`/`) truncates fractions toward zero, causing precision loss and under-collecting fees.
    // To prevent fee under-collection, ceiling division is simulated by adding `BPS_DENOMINATOR - 1` to the numerator.
    // Edge case: For small amounts, ceil rounding can force fees to 1 unit even when the basis points represent a tiny fraction.
    let platform_fee_amount = platform_bps.calculate_fee_ceil(amount);
    let network_fee_amount = network_bps.calculate_fee_ceil(amount);

    // The merchant amount is calculated as the subtraction remainder of the gross amount minus all rounded-up fees.
    // This ensures the sum of the split amounts (platform fee + network fee + merchant share) always equals the gross amount.
    // Consequence: The merchant absorbs all rounding dust. For very small gross amounts with high/extreme fee percentages,
    // the sum of rounded-up fees can exceed the gross amount, resulting in a negative merchant payout.
    let merchant_amount = amount - platform_fee_amount - network_fee_amount;
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_amount,
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
        /// * [`DuplicatePaymentReference`](SettlementError::DuplicatePaymentReference) — if the reference already exists.
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

            if !is_merchant_registered_internal(&env, merchant.clone()) {
                panic_with_error!(&env, SettlementError::MerchantMissing);
            }
            merchant.require_auth();
            if reference == BytesN::from_array(&env, &[0; 32]) {
                panic_with_error!(&env, SettlementError::InvalidPaymentReference);
            }
            if amount < MIN_PAYMENT_AMOUNT {
                panic_with_error!(&env, SettlementError::AmountTooSmall);
            }

            let payment_key = DataKey::Payment(reference.clone());
            if persistent_has_safe(&env, &payment_key) {
                panic_with_error!(&env, SettlementError::DuplicatePaymentReference);
            }

            let rule = read_rule_or_default(&env, merchant.clone());
            let split = calculate_split(&env, amount, &rule);
            let record = PaymentRecord {
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
                    Symbol::new(&env, "payment_stored"),
                    merchant.clone(),
                    reference.clone(),
                ),
                (),
            );

            split
        }

        /// Calculate the fee split for a given merchant and amount without storing a payment reference.
        ///
        /// # Panics
        ///
        /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
        /// * [`AmountZero`](SettlementError::AmountZero) — if `amount` is zero.
        /// * [`AmountNegative`](SettlementError::AmountNegative) — if `amount` is negative.
        /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
        pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
            if !is_merchant_registered_internal(&env, merchant.clone()) {
                panic_with_error!(&env, SettlementError::MerchantMissing);
            }
            if amount == 0 {
                panic_with_error!(&env, SettlementError::AmountZero);
            }
            if amount < 0 {
                panic_with_error!(&env, SettlementError::AmountNegative);
            }
            let rule = read_rule_or_default(&env, merchant);
            calculate_split(&env, amount, &rule)
        }

        /// Retrieve a payment record by its reference, extending the storage TTL if found.
        pub fn get_payment_reference(env: Env, reference: BytesN<32>) -> Option<PaymentRecord> {
            let key = DataKey::Payment(reference);
            let record: Option<PaymentRecord> = persistent_get_safe(&env, &key);
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

        /// Retrieve multiple payment records by a vector of references.
        ///
        /// The returned vector preserves the input order. Each entry is `Some(payment)`
        /// when a record exists for the corresponding reference and `None` otherwise.
        pub fn get_payments(env: Env, references: Vec<BytesN<32>>) -> Vec<Option<PaymentRecord>> {
            // `references.len()` is known upfront, so pre-allocating would avoid repeated
            // reallocation as this vector grows. soroban-sdk 21.7.7's Vec<T> has no
            // `with_capacity` constructor (only `new`, `from_array`, `from_slice`), so
            // this is left as a potential optimization for a future SDK version.
            let mut payments = Vec::new(&env);

            for reference in references.iter() {
                let payment = Self::get_payment_reference(env.clone(), reference.clone());
                payments.push_back(payment);
            }

            payments
        }

}
