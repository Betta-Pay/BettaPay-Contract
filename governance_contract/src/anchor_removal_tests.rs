//! Anchor removal verification test

#[cfg(test)]
mod tests {
    use crate::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Address;

    #[test]
    fn anchor_removal_clears_entry() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);
        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor));
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
}
