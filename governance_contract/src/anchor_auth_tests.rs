use soroban_sdk::testutils::{MockAuth, MockAuthInvoke, Events};
use soroban_sdk::{Address, Env};

// Import the contract and client
use super::*;

fn setup() -> (Env, GovernanceContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    client.init(&admin);
    (env, client, admin)
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn rejects_upsert_anchor_non_admin() {
    let (env, client, _admin) = setup();
    let non_admin = Address::generate(&env);
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&non_admin, &asset, &anchor);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn rejects_remove_anchor_non_admin() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    // set up anchor with admin first
    client.upsert_anchor(&admin, &asset, &anchor);
    // attempt removal with non-admin
    let non_admin = Address::generate(&env);
    client.remove_anchor(&non_admin, &asset);
}
