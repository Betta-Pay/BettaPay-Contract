use bettapay_common::events::AnchorUpserted;
use soroban_sdk::testutils::{Address as _, Events};
use soroban_sdk::{Address, FromVal, Symbol};

use super::*;

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

    let payload: AnchorUpserted = FromVal::from_val(&env, &data);
    assert_eq!(payload.previous, None);
    assert_eq!(payload.current, anchor);
    assert_eq!(payload.version, 1);
}

#[test]
fn remove_anchor_emits_anchor_removed_event() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    client.upsert_anchor(&admin, &asset, &anchor);

    let prev = env.events().all().len();
    client.remove_anchor(&admin, &asset);

    let events = env.events().all();
    assert_eq!(events.len(), prev + 1, "exactly one event emitted");

    let (_contract_id, topics, data) = events.get(prev).unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "anchor_removed")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), asset);

    // The removal event carries the last version counter so indexers can
    // correlate the removal with the prior upsert chain (issue #584).
    let version: u64 = FromVal::from_val(&env, &data);
    assert_eq!(version, 1);
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
    let payload: AnchorUpserted = FromVal::from_val(&env, &data);
    assert_eq!(payload.previous, Some(anchor_a));
    assert_eq!(payload.current, anchor_b);
    assert_eq!(payload.version, 2);
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

// ---------------------------------------------------------------------------
// Issue #584 — Event ordering tests
// ---------------------------------------------------------------------------

/// Rapid upserts for the same asset must produce strictly increasing
/// `version` values, so an indexer replaying events can reconstruct
/// the per-asset history unambiguously.
#[test]
fn rapid_upserts_produce_monotonically_increasing_versions() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);

    // Perform several rapid upserts — all land in the same ledger, so
    // without a version field the indexer could not determine the order.
    let anchor_count = 5;
    let mut anchors = soroban_sdk::vec![&env];
    for _ in 0..anchor_count {
        anchors.push_back(Address::generate(&env));
    }

    for i in 0..anchor_count {
        client.upsert_anchor(&admin, &asset, &anchors.get(i).unwrap());
    }

    // Collect the `anchor_upserted` events for this asset and verify the
    // version sequence is exactly [1, 2, 3, 4, 5].
    let events = env.events().all();
    let mut versions: soroban_sdk::Vec<u64> = soroban_sdk::vec![&env];
    let mut previous_anchors: soroban_sdk::Vec<Option<Address>> = soroban_sdk::vec![&env];

    for i in 0..events.len() {
        let (_contract_id, topics, data) = events.get(i).unwrap();
        if topics.len() == 2
            && Symbol::from_val(&env, &topics.get(0).unwrap())
                == Symbol::new(&env, "anchor_upserted")
            && Address::from_val(&env, &topics.get(1).unwrap()) == asset
        {
            let payload: AnchorUpserted = FromVal::from_val(&env, &data);
            versions.push_back(payload.version);
            previous_anchors.push_back(payload.previous);
        }
    }

    assert_eq!(versions.len(), anchor_count);
    for i in 0..anchor_count {
        assert_eq!(
            versions.get(i).unwrap(),
            (i as u64) + 1,
            "version must be exactly i+1 for the {i}-th upsert"
        );
    }

    // First upsert has no previous; every subsequent one references the
    // anchor from the prior upsert.
    assert_eq!(previous_anchors.get(0).unwrap(), None);
    for i in 1..anchor_count {
        assert_eq!(
            previous_anchors.get(i).unwrap(),
            Some(anchors.get(i - 1).unwrap()),
            "previous anchor at index {i} must match the anchor set at index {}",
            i - 1,
        );
    }
}

/// After removal, a subsequent upsert continues the version counter from
/// where it left off — the counter is *not* reset to 1.
#[test]
fn version_continues_after_removal() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);
    let anchor_a = Address::generate(&env);
    let anchor_b = Address::generate(&env);

    // v1: initial upsert
    client.upsert_anchor(&admin, &asset, &anchor_a);
    // remove (v1 stays in the version counter)
    client.remove_anchor(&admin, &asset);
    // v2: re-upsert — must NOT reset the counter
    client.upsert_anchor(&admin, &asset, &anchor_b);

    let events = env.events().all();
    let mut upsert_versions: soroban_sdk::Vec<u64> = soroban_sdk::vec![&env];

    for i in 0..events.len() {
        let (_contract_id, topics, data) = events.get(i).unwrap();
        if topics.len() == 2
            && Symbol::from_val(&env, &topics.get(0).unwrap())
                == Symbol::new(&env, "anchor_upserted")
            && Address::from_val(&env, &topics.get(1).unwrap()) == asset
        {
            let payload: AnchorUpserted = FromVal::from_val(&env, &data);
            upsert_versions.push_back(payload.version);
        }
    }

    assert_eq!(upsert_versions.len(), 2);
    assert_eq!(upsert_versions.get(0).unwrap(), 1);
    assert_eq!(upsert_versions.get(1).unwrap(), 2,
        "version must continue from 2 after removal, not reset to 1");
}

/// Version counters are per-asset: upserting different assets in
/// interleaved order must produce independent version sequences.
#[test]
fn versions_are_per_asset() {
    let (env, client, admin) = setup();
    let asset_a = Address::generate(&env);
    let asset_b = Address::generate(&env);
    let anchor_1 = Address::generate(&env);
    let anchor_2 = Address::generate(&env);
    let anchor_3 = Address::generate(&env);

    // Interleaved upserts: A1, B1, A2
    client.upsert_anchor(&admin, &asset_a, &anchor_1); // A v1
    client.upsert_anchor(&admin, &asset_b, &anchor_2); // B v1
    client.upsert_anchor(&admin, &asset_a, &anchor_3); // A v2

    let events = env.events().all();

    let mut a_versions: soroban_sdk::Vec<u64> = soroban_sdk::vec![&env];
    let mut b_versions: soroban_sdk::Vec<u64> = soroban_sdk::vec![&env];

    for i in 0..events.len() {
        let (_contract_id, topics, data) = events.get(i).unwrap();
        if topics.len() == 2
            && Symbol::from_val(&env, &topics.get(0).unwrap())
                == Symbol::new(&env, "anchor_upserted")
        {
            let event_asset = Address::from_val(&env, &topics.get(1).unwrap());
            let payload: AnchorUpserted = FromVal::from_val(&env, &data);
            if event_asset == asset_a {
                a_versions.push_back(payload.version);
            } else if event_asset == asset_b {
                b_versions.push_back(payload.version);
            }
        }
    }

    assert_eq!(a_versions.len(), 2);
    assert_eq!(a_versions.get(0).unwrap(), 1);
    assert_eq!(a_versions.get(1).unwrap(), 2);

    assert_eq!(b_versions.len(), 1);
    assert_eq!(b_versions.get(0).unwrap(), 1);
}

/// Ordered-event reconstruction: an indexer processing all `anchor_upserted`
/// events for a given asset can reconstruct the exact anchor history using
/// the version field alone.
#[test]
fn ordered_event_reconstruction() {
    let (env, client, admin) = setup();
    let asset = Address::generate(&env);

    // Simulate a series of upserts.
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);

    client.upsert_anchor(&admin, &asset, &a1);
    client.upsert_anchor(&admin, &asset, &a2);
    client.upsert_anchor(&admin, &asset, &a3);

    // --- Indexer replay ---
    // Collect all anchor_upserted events for `asset` and build an ordered
    // history using only the `version` field (sort by version ascending).
    let events = env.events().all();
    let mut history: soroban_sdk::Vec<(u64, Option<Address>, Address)> =
        soroban_sdk::vec![&env];

    for i in 0..events.len() {
        let (_contract_id, topics, data) = events.get(i).unwrap();
        if topics.len() == 2
            && Symbol::from_val(&env, &topics.get(0).unwrap())
                == Symbol::new(&env, "anchor_upserted")
            && Address::from_val(&env, &topics.get(1).unwrap()) == asset
        {
            let payload: AnchorUpserted = FromVal::from_val(&env, &data);
            history.push_back((payload.version, payload.previous, payload.current));
        }
    }

    // Sort by version (already ordered in this test, but the point is the
    // version makes sorting possible even if events arrive out of order).
    // Verify the reconstructed chain:
    //   v1: None → a1
    //   v2: a1   → a2
    //   v3: a2   → a3
    assert_eq!(history.len(), 3);

    let (v1, prev1, cur1) = history.get(0).unwrap();
    assert_eq!((v1, prev1, cur1), (1, None, a1.clone()));

    let (v2, prev2, cur2) = history.get(1).unwrap();
    assert_eq!((v2, prev2, cur2), (2, Some(a1), a2.clone()));

    let (v3, prev3, cur3) = history.get(2).unwrap();
    assert_eq!((v3, prev3, cur3), (3, Some(a2), a3));
}
