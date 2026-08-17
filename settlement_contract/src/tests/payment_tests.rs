//! Tests for payment reference storage and fee-split calculation.

extern crate std;

use crate::*;
use soroban_sdk::testutils::Ledger;
use soroban_sdk::{Address, BytesN, Env};

use super::setup;

fn rule(platform_fee_bps: u32, network_fee_bps: u32) -> SettlementRule {
    SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger: 0,
        auto_settle: false,
    }
}

fn payment_reference(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

fn registered_merchant_with_rule(
    client: &SettlementContractClient<'static>,
    admins: &soroban_sdk::Vec<Address>,
    merchant: &Address,
    settlement_rule: SettlementRule,
) {
    client.register_merchant(admins, merchant);
    client.set_settlement_rule(admins, merchant, &settlement_rule);
}

#[test]
fn calculate_fee_split_preserves_valid_combined_fees() {
    let (_env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(250, 50));

    let split = client.calculate_fee_split(&merchant, &10_000);

    assert_eq!(split.gross_amount, 10_000);
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_amount, 9_700);
}

#[test]
fn calculate_fee_split_allows_exact_100_percent_when_rounding_fits() {
    let (_env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(5_000, 5_000));

    let split = client.calculate_fee_split(&merchant, &100);

    assert_eq!(split.gross_amount, 100);
    assert_eq!(split.platform_fee_amount, 50);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_amount, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn calculate_fee_split_rejects_ceil_rounded_fees_above_amount() {
    let (_env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(5_000, 5_000));

    client.calculate_fee_split(&merchant, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn calculate_fee_split_rejects_minimal_amount_when_rounding_exceeds_gross() {
    let (_env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(5_000, 5_000));

    client.calculate_fee_split(&merchant, &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #314)")]
fn calculate_fee_split_rejects_zero_amount_before_split_calculation() {
    let (_env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(250, 50));

    client.calculate_fee_split(&merchant, &0);
}

#[test]
fn store_payment_reference_rejects_ceil_rounded_split_without_storing_record() {
    let (env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(5_000, 5_000));
    let reference = payment_reference(&env, 1);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.store_payment_reference(&merchant, &reference, &101);
    }));

    assert!(result.is_err());
    assert!(client.get_payment_reference(&reference).is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_default_rule_rejects_fee_sum_over_100_percent() {
    let (_env, client, admins, _merchant) = setup();

    client.set_default_rule(&admins, &rule(6_000, 6_000));
}

#[test]
fn scheduled_set_default_rule_rejects_fee_sum_over_100_percent() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let invalid_rule = rule(6_000, 6_000);
    let operation = Operation::SetDefaultRule(invalid_rule);

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.execute(&operation);
    }));

    assert!(result.is_err());
    assert!(client.get_default_rule().is_none());
}

#[test]
fn store_payment_reference_keeps_valid_payment_storage_unchanged() {
    let (env, client, admins, merchant) = setup();
    registered_merchant_with_rule(&client, &admins, &merchant, rule(250, 50));
    let reference = payment_reference(&env, 2);

    let split = client.store_payment_reference(&merchant, &reference, &10_000);
    let record = client.get_payment_reference(&reference).unwrap();

    assert_eq!(split.gross_amount, 10_000);
    assert_eq!(split.platform_fee_amount, 250);
    assert_eq!(split.network_fee_amount, 50);
    assert_eq!(split.merchant_amount, 9_700);
    assert_eq!(record.amount, 10_000);
    assert_eq!(record.platform_fee_amount, 250);
    assert_eq!(record.network_fee_amount, 50);
    assert_eq!(record.merchant_amount, 9_700);
}
