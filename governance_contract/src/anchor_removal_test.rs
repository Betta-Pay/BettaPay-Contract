//! Anchor removal verification test

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::{Env, vec};

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
    fn anchor_removal_clears_entry() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);
        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        client.remove_anchor(&admin, &asset);
        assert_eq!(client.get_anchor(&asset), None);
    }
}
