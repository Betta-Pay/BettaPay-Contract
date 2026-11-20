//! Anchor removal verification test

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Error};

use super::setup;

#[test]
fn anchor_removal_clears_entry() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admin, &asset, &anchor);
    assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
    client.remove_anchor(&admin, &asset);
    assert_eq!(client.get_anchor(&asset), None);
}

#[test]
fn rejects_removing_unregistered_anchor() {
    let (env, client, admin) = setup();
    let missing_asset = Address::generate(&env);
    let result = client.try_remove_anchor(&admin, &missing_asset);
    assert!(
        matches!(result, Err(Ok(e)) if e == Error::from_contract_error(GovernanceError::AnchorMissing as u32)),
        "remove_anchor must reject unregistered anchor with AnchorMissing"
    );
}
