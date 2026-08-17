//! Anchor removal verification test

#[cfg(test)]
mod tests {
    use crate::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::testutils::{Ledger, Storage};
    use soroban_sdk::Address;

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
    #[should_panic]
    fn rejects_removing_unregistered_anchor() {
        let (env, client, admin) = setup();
        let missing_asset = Address::generate(&env);
        client.remove_anchor(&admin, &missing_asset);
    }

    /// Issue #583: remove_anchor on an expired/archived anchor must surface
    /// the typed `AnchorMissing` error (code 200) rather than a raw host
    /// error from the storage layer.
    #[test]
    #[should_panic(expected = "Error(Contract, #200)")]
    fn remove_anchor_on_expired_anchor_returns_typed_anchor_missing() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));

        env.as_contract(&client.address, || {
            let key = DataKey::Anchor(asset.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        client.remove_anchor(&admin, &asset);
    }

    /// get_anchor on an expired/archived entry must return `None` instead of
    /// panicking with a raw host error.
    #[test]
    fn get_anchor_on_expired_anchor_returns_none() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));

        env.as_contract(&client.address, || {
            let key = DataKey::Anchor(asset.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_anchor(&asset), None);
    }

    /// upsert_anchor on an expired/archived entry must treat the entry as
    /// missing (old_anchor = None) and store the new value without a raw
    /// host error.
    #[test]
    fn upsert_anchor_on_expired_anchor_treats_as_missing_and_succeeds() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor_v1 = Address::generate(&env);
        let anchor_v2 = Address::generate(&env);

        client.upsert_anchor(&admin, &asset, &anchor_v1);
        assert_eq!(client.get_anchor(&asset), Some(anchor_v1.clone()));

        env.as_contract(&client.address, || {
            let key = DataKey::Anchor(asset.clone());
            let current_ttl = env.storage().persistent().get_ttl(&key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        client.upsert_anchor(&admin, &asset, &anchor_v2);
        assert_eq!(client.get_anchor(&asset), Some(anchor_v2));
    }

    /// get_system_param on an expired/archived entry must return `None`
    /// instead of panicking with a raw host error.
    #[test]
    fn get_system_param_on_expired_returns_none() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "test_param");

        client.update_system_param(&admin, &key, &42);
        assert_eq!(client.get_system_param(&key), Some(42));

        env.as_contract(&client.address, || {
            let storage_key = DataKey::SystemParam(key.clone());
            let current_ttl = env.storage().persistent().get_ttl(&storage_key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_system_param(&key), None);
    }

    /// get_fee_config on an expired/archived entry must return `None`
    /// instead of panicking with a raw host error.
    #[test]
    fn get_fee_config_on_expired_returns_none() {
        let (env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 50,
        };

        client.set_fee_config(&admin, &cfg);
        assert!(client.get_fee_config().is_some());

        env.as_contract(&client.address, || {
            let storage_key = DataKey::FeeConfig;
            let current_ttl = env.storage().persistent().get_ttl(&storage_key);
            env.ledger().with_mut(|li| {
                li.sequence_number += current_ttl + 1;
            });
        });

        assert_eq!(client.get_fee_config(), None);
    }
}
