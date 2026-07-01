use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{symbol_short, Address, Env, FromVal, Symbol};

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
fn upsert_anchor_emits_anchor_upserted_event() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    let prev = env.events().all().len();
    client.upsert_anchor(&admin, &asset, &anchor);

    let events = env.events().all();
    assert_eq!(events.len(), prev + 1, "exactly one event emitted");

    let (_contract_id, topics, data) = events.get(prev).unwrap();

    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "anchor_upserted")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), asset);
    assert_eq!(Address::from_val(&env, &data), anchor);
}

#[test]
fn remove_anchor_emits_anchor_rm_and_removed_events() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    client.upsert_anchor(&admin, &asset, &anchor);

    let prev = env.events().all().len();
    client.remove_anchor(&admin, &asset);

    let events = env.events().all();
    assert_eq!(events.len(), prev + 2, "two events emitted");

    // First event: anchor_rm with short symbol
    let (_contract_id, topics, data) = events.get(prev).unwrap();
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        symbol_short!("anchor_rm")
    );
    let (event_asset,): (Address,) = FromVal::from_val(&env, &data);
    assert_eq!(event_asset, asset);

    // Second event: anchor_removed with full symbol
    let (_contract_id, topics, _data) = events.get(prev + 1).unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "anchor_removed")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), asset);
}

#[test]
fn upsert_anchor_update_also_emits_event() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor_a = Address::generate(&env);
    let anchor_b = Address::generate(&env);

    client.upsert_anchor(&admin, &asset, &anchor_a);

    let prev = env.events().all().len();
    client.upsert_anchor(&admin, &asset, &anchor_b);

    let events = env.events().all();
    assert_eq!(events.len(), prev + 1, "update emits one event");

    let (_contract_id, _topics, data) = events.get(prev).unwrap();
    assert_eq!(Address::from_val(&env, &data), anchor_b);
}

#[test]
fn get_anchor_does_not_emit_event() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    client.upsert_anchor(&admin, &asset, &anchor);

    let prev = env.events().all().len();
    let _ = client.get_anchor(&asset);

    assert_eq!(
        env.events().all().len(),
        prev,
        "get_anchor should not emit events"
    );
}
