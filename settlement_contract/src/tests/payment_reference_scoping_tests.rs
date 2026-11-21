//! Cross-merchant payment reference isolation (issue #1161).
//!
//! `DataKey::Payment(merchant, reference)` is keyed by both fields, so a
//! reference is unique per merchant rather than globally. These tests pin that
//! contract: two merchants may store the same 32-byte reference, and each read
//! resolves only its own record.

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

use super::setup;

#[test]
fn two_merchants_can_reuse_the_same_payment_reference() {
    let (env, client, admins, _) = setup();
    let first_merchant = Address::generate(&env);
    let second_merchant = Address::generate(&env);
    client.register_merchant(&admins, &first_merchant);
    client.register_merchant(&admins, &second_merchant);

    let shared = BytesN::from_array(&env, &[77; 32]);

    client.store_payment_reference(&first_merchant, &shared, &1_000);
    client.store_payment_reference(&second_merchant, &shared, &9_000);

    let first_record = client
        .get_payment_reference(&first_merchant, &shared, &soroban_sdk::vec![&env])
        .expect("first merchant's record must exist");
    let second_record = client
        .get_payment_reference(&second_merchant, &shared, &soroban_sdk::vec![&env])
        .expect("second merchant's record must exist");

    assert_eq!(first_record.amount, 1_000);
    assert_eq!(first_record.merchant, first_merchant);
    assert_eq!(second_record.amount, 9_000);
    assert_eq!(second_record.merchant, second_merchant);
    assert_ne!(
        first_record.merchant, second_record.merchant,
        "each record must be scoped to its own merchant"
    );
}
