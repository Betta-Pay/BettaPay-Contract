use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::events;

use crate::errors::SettlementError;
use crate::policy::{MERCHANT_TTL_BUMP, MERCHANT_TTL_THRESHOLD};
use crate::storage::{
    assert_admin_and_not_paused, is_merchant_registered_read, read_fallback_rule,
    validate_nonzero_address,
};
use crate::types::{DataKey, SettlementRule};
use crate::SettlementContract;

#[contractimpl]
impl SettlementContract {
    /// Registers a new merchant in the protocol.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`ZeroAddress`](SettlementError::ZeroAddress) — if the provided merchant address is the zero address.
    /// * [`InvalidAdmin`](SettlementError::InvalidAdmin) — if attempting to register an admin as a merchant.
    /// * [`MerchantExists`](SettlementError::MerchantExists) — if the merchant is already registered.
    pub fn register_merchant(env: Env, signers: Vec<Address>, merchant: Address) {
        assert_admin_and_not_paused(&env, &signers);

        validate_nonzero_address(&env, &merchant, SettlementError::ZeroAddress);
        let admin = signers.get(0).unwrap();

        // Prevent an admin from being registered as a merchant
        use crate::storage::read_admins;
        let admins = read_admins(&env);
        for i in 0..admins.len() {
            if admins.get(i).unwrap() == merchant {
                panic_with_error!(&env, SettlementError::InvalidAdmin);
            }
        }

        // The merchant must consent to its own registration — admin auth alone
        // is not proof the merchant address is controlled by the party being
        // registered, and would otherwise let an admin register arbitrary or
        // squatted addresses as merchants.
        merchant.require_auth();

        // Distinct from `merchant_registered` below: this marks the proof of
        // the merchant's own consent, separate from the admin action.
        env.events().publish(
            (
                Symbol::new(&env, events::MERCHANT_CONSENT_EVENT),
                merchant.clone(),
            ),
            (),
        );

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &());
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);

        // Remove any ArchivedMerchant tombstone from a prior registration so
        // the re-registered merchant can read new payment records (issue #685).
        let archived_key = DataKey::ArchivedMerchant(merchant.clone());
        env.storage().persistent().remove(&archived_key);

        env.events().publish(
            (
                Symbol::new(&env, events::MERCHANT_REGISTERED_EVENT),
                merchant,
            ),
            admin,
        );
    }

    /// Unregisters an existing merchant from the protocol.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is currently paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not currently registered.
    pub fn unregister_merchant(env: Env, signers: Vec<Address>, merchant: Address) {
        assert_admin_and_not_paused(&env, &signers);
        let admin = signers.get(0).unwrap();

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        // Orphan the merchant's payment history: an ArchivedMerchant tombstone
        // makes every existing payment record unreadable for as long as the
        // merchant stays unregistered (issue #490). Re-registration clears the
        // tombstone (issue #685) so a re-registered merchant can read its new
        // records again.
        let archived_key = DataKey::ArchivedMerchant(merchant.clone());
        env.storage().persistent().set(&archived_key, &());
        env.storage().persistent().extend_ttl(
            &archived_key,
            MERCHANT_TTL_THRESHOLD,
            MERCHANT_TTL_BUMP,
        );

        let rule_key = DataKey::Rule(merchant.clone());
        let old_rule: Option<SettlementRule> = env.storage().persistent().get(&rule_key);
        if let Some(old_rule) = old_rule {
            env.storage().persistent().remove(&rule_key);
            // Emit the same canonical event shape as clear_settlement_rule
            // (issue #491): (admin, removed, fallback). Use the shared
            // fallback chain (default → governance → bootstrap) so the event
            // matches the rule that will actually govern the next payment
            // (issue #689).
            let fallback = read_fallback_rule(&env);
            events::emit_settlement_rule_cleared(&env, &merchant, &admin, &old_rule, &fallback);
        }

        env.events().publish(
            (
                Symbol::new(&env, events::MERCHANT_UNREGISTERED_EVENT),
                merchant,
            ),
            admin,
        );
    }

    /// Returns `true` if the given address is a registered merchant, `false` otherwise.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    pub fn is_merchant_registered(env: Env, merchant: Address) -> bool {
        if !env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::NotInitialized);
        }
        is_merchant_registered_read(&env, merchant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::setup;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn cannot_register_admin_as_merchant() {
        let (env, client, admins, _merchant) = setup();
        let admin = admins.get(0).unwrap();

        // Registering an active admin as a merchant must fail with InvalidAdmin
        let result = client.try_register_merchant(&admins, &admin);
        assert_eq!(
            result.unwrap_err().unwrap(),
            SettlementError::InvalidAdmin.into()
        );

        // Registering a non-admin address still succeeds
        let non_admin = Address::generate(&env);
        client.register_merchant(&admins, &non_admin);
        assert!(client.is_merchant_registered(&non_admin));
    }

    #[test]
    fn register_merchant_rejects_zero_address_without_writing_storage() {
        let (env, client, admins, _) = setup();
        let zero_address = bettapay_common::storage::zero_address(&env);
        assert!(bettapay_common::storage::is_zero_address(
            &env,
            &zero_address
        ));

        let result = client.try_register_merchant(&admins, &zero_address);
        assert_eq!(
            result.unwrap_err().unwrap(),
            SettlementError::ZeroAddress.into()
        );

        let merchant_key = DataKey::Merchant(zero_address.clone());
        let archived_key = DataKey::ArchivedMerchant(zero_address);
        env.as_contract(&client.address, || {
            assert!(!env.storage().persistent().has(&merchant_key));
            assert!(!env.storage().persistent().has(&archived_key));
        });
    }

    #[test]
    fn register_merchant_accepts_valid_generated_address() {
        let (env, client, admins, _) = setup();
        let merchant = Address::generate(&env);

        client.register_merchant(&admins, &merchant);

        assert!(client.is_merchant_registered(&merchant));
    }
}
