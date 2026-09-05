use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Symbol, Vec};

use bettapay_common::events;

use crate::errors::SettlementError;
use crate::storage::{
    assert_not_paused, is_merchant_registered_internal, read_fallback_rule, read_threshold,
    validate_nonzero_address, verify_admin_auth,
};
use crate::types::{DataKey, SettlementRule};
use crate::{
    SettlementContract, SettlementContractClient, MERCHANT_TTL_BUMP,
    MERCHANT_TTL_THRESHOLD,
};

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
        assert_not_paused(&env);

        validate_nonzero_address(
            &env,
            &merchant,
            SettlementError::ZeroAddress,
        );

        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        // Prevent an admin from being registered as a merchant
        use crate::storage::read_admins;
        let admins = read_admins(&env);
        for i in 0..admins.len() {
            if admins.get(i).unwrap() == merchant {
                panic_with_error!(&env, SettlementError::InvalidAdmin);
            }
        }

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
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        // Orphan the merchant's payment history: an ArchivedMerchant tombstone
        // makes every existing payment record unreadable for the rest of its
        // TTL (issue #490). The tombstone survives re-registration, so a
        // re-registered merchant cannot resurrect records from an earlier
        // registration either.
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
        is_merchant_registered_internal(&env, merchant)
    }
}
