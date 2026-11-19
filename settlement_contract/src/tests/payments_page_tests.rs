//! Tests for `SettlementContract::get_payments` page semantics (issue #1162).
//!
//! `get_payments` cannot enumerate storage keys: the caller supplies `refs` and
//! every reference is looked up in the merchant's namespace. These tests pin the
//! two observable consequences of that design:
//!
//! 1. References with no stored record are silently skipped while the existing
//!    records keep the relative order of the caller's `refs`.
//! 2. `next_cursor` is derived from `refs.len()`, not from the number of records
//!    found, so a full `MAX_PAYMENTS_BATCH` page always signals another page
//!    even when some of its refs were missing.

use crate::*;
use soroban_sdk::{BytesN, Env, Error, Vec};

use super::setup;

/// Distinct, non-zero 32-byte reference for `tag`.
fn reference(env: &Env, tag: u8) -> BytesN<32> {
    BytesN::from_array(env, &[tag; 32])
}

#[test]
fn missing_refs_are_skipped_and_existing_records_keep_relative_order() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let first = reference(&env, 1);
    let second = reference(&env, 2);
    let third = reference(&env, 3);
    client.store_payment_reference(&merchant, &first, &1_000);
    client.store_payment_reference(&merchant, &second, &2_000);
    client.store_payment_reference(&merchant, &third, &3_000);

    let missing_a = reference(&env, 201);
    let missing_b = reference(&env, 202);
    let refs = soroban_sdk::vec![
        &env,
        missing_a.clone(),
        first.clone(),
        missing_b.clone(),
        third.clone(),
        second.clone(),
    ];

    let page = client.get_payments(&merchant, &refs, &0, &soroban_sdk::vec![&env]);

    assert_eq!(
        page.records.len(),
        3,
        "missing refs must be omitted without error"
    );
    assert_eq!(page.records.get(0).unwrap().amount, 1_000);
    assert_eq!(page.records.get(1).unwrap().amount, 3_000);
    assert_eq!(page.records.get(2).unwrap().amount, 2_000);
    assert_eq!(
        page.records.get(0).unwrap().platform_fee_bps,
        page.records.get(1).unwrap().platform_fee_bps,
        "records must come from this merchant's namespace"
    );
    assert!(
        page.next_cursor.is_none(),
        "a short page must not advertise another page"
    );

    assert!(client
        .get_payment_reference(&merchant, &missing_a, &soroban_sdk::vec![&env])
        .is_none());
    assert!(client
        .get_payment_reference(&merchant, &missing_b, &soroban_sdk::vec![&env])
        .is_none());
}

#[test]
fn full_page_reports_next_cursor_from_refs_len() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let mut refs: Vec<BytesN<32>> = Vec::new(&env);
    for index in 1..=MAX_PAYMENTS_BATCH {
        let reference = reference(&env, index as u8);
        client.store_payment_reference(&merchant, &reference, &1_000);
        refs.push_back(reference);
    }
    assert_eq!(refs.len(), MAX_PAYMENTS_BATCH);

    let page = client.get_payments(&merchant, &refs, &0, &soroban_sdk::vec![&env]);

    assert_eq!(page.records.len(), MAX_PAYMENTS_BATCH);
    assert_eq!(
        page.next_cursor,
        Some(MAX_PAYMENTS_BATCH),
        "a full page reports cursor + refs.len()"
    );

    let with_offset = client.get_payments(&merchant, &refs, &250, &soroban_sdk::vec![&env]);
    assert_eq!(with_offset.next_cursor, Some(350));

    let short = soroban_sdk::vec![&env, reference(&env, 1), reference(&env, 2)];
    let short_page = client.get_payments(&merchant, &short, &250, &soroban_sdk::vec![&env]);
    assert_eq!(
        short_page.next_cursor, None,
        "below the cap the caller's reference set is exhausted"
    );
}

#[test]
fn batch_over_max_panics_batch_too_large() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let mut refs: Vec<BytesN<32>> = Vec::new(&env);
    for index in 1..=MAX_PAYMENTS_BATCH + 1 {
        refs.push_back(reference(&env, index as u8));
    }
    assert_eq!(refs.len(), MAX_PAYMENTS_BATCH + 1);

    assert!(
        matches!(
            client.try_get_payments(&merchant, &refs, &0, &soroban_sdk::vec![&env]),
            Err(Ok(e)) if e == Error::from_contract_error(SettlementError::BatchTooLarge as u32)
        ),
        "refs over MAX_PAYMENTS_BATCH must be rejected with BatchTooLarge"
    );
}
