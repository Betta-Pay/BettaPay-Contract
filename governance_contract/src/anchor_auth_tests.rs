use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

// Import the contract and client
use super::*;

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn rejects_upsert_anchor_non_admin() {
    let (env, client, _admins) = setup();
    let non_admin = Address::generate(&env);
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&soroban_sdk::vec![&env, non_admin], &asset, &anchor);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn rejects_remove_anchor_non_admin() {
    let (env, client, admins) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    // set up anchor with admins first
    client.upsert_anchor(&admins, &asset, &anchor);
    // attempt removal with non-admin
    let non_admin = Address::generate(&env);
    client.remove_anchor(&soroban_sdk::vec![&env, non_admin], &asset);
}
