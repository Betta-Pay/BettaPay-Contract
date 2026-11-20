//! Issue #1104: the per-merchant payment cap is enforced on the write path and
//! pruning frees capacity again.

use super::setup;
use crate::errors::SettlementError;
use crate::policy::MAX_PAYMENTS_PER_MERCHANT;
use crate::types::DataKey;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, Error};

fn reference(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

/// Seeds the counter directly so the boundary can be tested without storing
/// 10,000 individual payment records.
fn set_payment_count(env: &Env, contract: &Address, merchant: &Address, count: u32) {
    env.as_contract(contract, || {
        env.storage()
            .persistent()
            .set(&DataKey::PaymentCount(merchant.clone()), &count);
    });
}

#[test]
fn store_payment_reference_rejects_payment_past_cap_and_prune_restores_capacity() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let first = reference(&env, 1);
    client.store_payment_reference(&merchant, &first, &1_000);
    set_payment_count(&env, &client.address, &merchant, MAX_PAYMENTS_PER_MERCHANT);

    let overflow = reference(&env, 2);
    assert!(matches!(
        client.try_store_payment_reference(&merchant, &overflow, &1_000),
        Err(Ok(error))
            if error == Error::from_contract_error(SettlementError::MaxPaymentsReached as u32)
    ));
    assert_eq!(
        client.get_payment_count(&merchant, &soroban_sdk::vec![&env]),
        MAX_PAYMENTS_PER_MERCHANT
    );
    // The rejected call must not leave the reentrancy-guard dummy behind.
    assert!(client
        .get_payment_reference(&merchant, &overflow, &soroban_sdk::vec![&env])
        .is_none());

    client.prune_payment(&admins, &merchant, &first);
    assert_eq!(
        client.get_payment_count(&merchant, &soroban_sdk::vec![&env]),
        MAX_PAYMENTS_PER_MERCHANT - 1
    );

    client.store_payment_reference(&merchant, &overflow, &1_000);
    assert_eq!(
        client.get_payment_count(&merchant, &soroban_sdk::vec![&env]),
        MAX_PAYMENTS_PER_MERCHANT
    );
}

#[test]
fn prune_payments_also_restores_capped_capacity() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let first = reference(&env, 1);
    let second = reference(&env, 2);
    client.store_payment_reference(&merchant, &first, &1_000);
    client.store_payment_reference(&merchant, &second, &2_000);
    set_payment_count(&env, &client.address, &merchant, MAX_PAYMENTS_PER_MERCHANT);

    let overflow = reference(&env, 3);
    assert!(matches!(
        client.try_store_payment_reference(&merchant, &overflow, &1_000),
        Err(Ok(error))
            if error == Error::from_contract_error(SettlementError::MaxPaymentsReached as u32)
    ));

    let refs = soroban_sdk::vec![&env, first, second];
    assert_eq!(client.prune_payments(&admins, &merchant, &refs), 2);
    assert_eq!(
        client.get_payment_count(&merchant, &soroban_sdk::vec![&env]),
        MAX_PAYMENTS_PER_MERCHANT - 2
    );

    client.store_payment_reference(&merchant, &overflow, &1_000);
    assert_eq!(
        client.get_payment_count(&merchant, &soroban_sdk::vec![&env]),
        MAX_PAYMENTS_PER_MERCHANT - 1
    );
}
