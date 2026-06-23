#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address, BytesN, Env,
    Symbol,
};

const BPS_DENOMINATOR: i128 = 10_000;

#[derive(Clone)]
#[contracttype]
pub struct SettlementRule {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
    pub settlement_delay_ledger: u32,
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeSplit {
    pub gross_amount: i128,
    pub platform_fee_amount: i128,
    pub network_fee_amount: i128,
    pub merchant_amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct PaymentRecord {
    pub merchant: Address,
    pub amount: i128,
    pub platform_fee_amount: i128,
    pub network_fee_amount: i128,
    pub merchant_amount: i128,
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
    pub ledger: u32,
    pub settlement_delay_ledger: u32,
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Admin,
    Merchant(Address),
    Rule(Address),
    Payment(BytesN<32>),
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    MerchantExists = 4,
    MerchantMissing = 5,
    InvalidFeeBps = 6,
    InvalidAmount = 7,
    DuplicatePaymentReference = 8,
    Paused = 9,
    RuleNotSet = 10,
}

#[contract]
pub struct SettlementContract;

#[contractimpl]
impl SettlementContract {
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    pub fn get_admin(env: Env) -> Address {
        read_admin(&env)
    }

    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish((symbol_short!("admin"),), new_admin);
    }

    pub fn pause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("pause"),), true);
    }

    pub fn unpause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("unpause"),), false);
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    pub fn register_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &true);
        env.events()
            .publish((symbol_short!("merchant"), merchant), true);
    }

    /// ## Emitted Event: `settlement_rule_updated`
    ///
    /// **Topics**: `(Symbol("settlement_rule_updated"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was updated
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the rule change
    /// - `previous`: the rule values before the update (or system defaults on first set)
    /// - `current`: the new rule values after the update
    pub fn set_settlement_rule(env: Env, merchant: Address, rule: SettlementRule) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR as u32 || rule.network_fee_bps > BPS_DENOMINATOR as u32 {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }

        let prev = env.storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or(SettlementRule {
                platform_fee_bps: 100,
                network_fee_bps: 0,
                settlement_delay_ledger: 0,
                auto_settle: false,
            });

        env.storage()
            .persistent()
            .set(&DataKey::Rule(merchant.clone()), &rule);

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_updated"), merchant),
            (admin, prev, rule),
        );
    }

    /// ## Emitted Event: `settlement_rule_cleared`
    ///
    /// **Topics**: `(Symbol("settlement_rule_cleared"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was cleared
    ///
    /// **Data**: `(Address caller, SettlementRule removed)`
    /// - `caller`: the admin who authorized the removal
    /// - `removed`: the rule values that were removed from storage
    pub fn clear_settlement_rule(env: Env, merchant: Address) {
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Rule(merchant.clone());
        let removed = env.storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::RuleNotSet));

        env.storage().persistent().remove(&key);

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_cleared"), merchant),
            (admin, removed),
        );
    }

    pub fn store_payment_reference(env: Env, merchant: Address, reference: BytesN<32>, amount: i128) -> FeeSplit {
        assert_not_paused(&env);
        merchant.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if amount <= 0 {
            panic_with_error!(&env, SettlementError::InvalidAmount);
        }

        let payment_key = DataKey::Payment(reference.clone());
        if env.storage().persistent().has(&payment_key) {
            panic_with_error!(&env, SettlementError::DuplicatePaymentReference);
        }

        let rule = read_rule_or_default(&env, merchant.clone());
        let split = calculate_split(amount, &rule);
        let record = PaymentRecord {
            merchant: merchant.clone(),
            amount,
            platform_fee_amount: split.platform_fee_amount,
            network_fee_amount: split.network_fee_amount,
            merchant_amount: split.merchant_amount,
            platform_fee_bps: rule.platform_fee_bps,
            network_fee_bps: rule.network_fee_bps,
            ledger: env.ledger().sequence(),
            settlement_delay_ledger: rule.settlement_delay_ledger,
            auto_settle: rule.auto_settle,
        };

        env.storage().persistent().set(&payment_key, &record);
        env.events()
            .publish((symbol_short!("payment"), merchant.clone()), reference);
        env.events().publish(
            (symbol_short!("split"), merchant),
            (split.gross_amount, split.platform_fee_amount, split.network_fee_amount, split.merchant_amount),
        );

        split
    }

    pub fn is_merchant_registered(env: Env, merchant: Address) -> bool {
        is_merchant_registered_internal(&env, merchant)
    }

    pub fn get_settlement_rule(env: Env, merchant: Address) -> Option<SettlementRule> {
        env.storage().persistent().get(&DataKey::Rule(merchant))
    }

    pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if amount <= 0 {
            panic_with_error!(&env, SettlementError::InvalidAmount);
        }
        let rule = read_rule_or_default(&env, merchant);
        calculate_split(amount, &rule)
    }

    pub fn get_payment_reference(env: Env, reference: BytesN<32>) -> Option<PaymentRecord> {
        env.storage().persistent().get(&DataKey::Payment(reference))
    }
}

fn read_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Merchant(merchant))
        .unwrap_or(false)
}

fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
    env.storage()
        .persistent()
        .get(&DataKey::Rule(merchant))
        .unwrap_or(SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        })
}

fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}

fn assert_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

fn calculate_split(amount: i128, rule: &SettlementRule) -> FeeSplit {
    let platform_fee_amount = amount * (rule.platform_fee_bps as i128) / BPS_DENOMINATOR;
    let network_fee_amount = amount * (rule.network_fee_bps as i128) / BPS_DENOMINATOR;
    let merchant_amount = amount - platform_fee_amount - network_fee_amount;
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
    use soroban_sdk::FromVal;

    fn setup() -> (Env, SettlementContractClient<'static>, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let contract_id = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);
        client.init(&admin);
        (env, client, admin, merchant)
    }

    #[test]
    fn registers_merchant_and_persists_flag() {
        let (env, client, _admin, merchant) = setup();
        let before = env.events().all().len();
        client.register_merchant(&merchant);
        assert!(client.is_merchant_registered(&merchant));
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn sets_and_reads_settlement_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: true,
        };

        let prev_count = env.events().all().len();
        client.set_settlement_rule(&merchant, &rule);
        let got = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");

        assert_eq!(got.platform_fee_bps, 175);
        assert_eq!(got.network_fee_bps, 25);
        assert_eq!(got.settlement_delay_ledger, 42);
        assert!(got.auto_settle);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

        // Topic[0] must be the fixed event-name symbol
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        // Topic[1] must be the merchant (rule identifier)
        assert_eq!(
            Address::from_val(&env, &topics.get(1).unwrap()),
            merchant
        );
    }

    #[test]
    fn emits_structured_event_when_updating_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let first_rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &first_rule);

        let second_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 20,
            auto_settle: true,
        };

        let prev_count = env.events().all().len();
        client.set_settlement_rule(&merchant, &second_rule);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

        // Topic[0] must be the fixed event-name symbol
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        // Topic[1] must be the merchant
        assert_eq!(
            Address::from_val(&env, &topics.get(1).unwrap()),
            merchant
        );

        // Verify storage was updated
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.platform_fee_bps, 200);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.settlement_delay_ledger, 20);
        assert!(stored.auto_settle);
    }

    #[test]
    fn stores_payment_reference_once_and_calculates_split() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[7; 32]);
        let before = env.events().all().len();
        let split = client.store_payment_reference(&merchant, &reference, &20_000);
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 100);
        assert_eq!(split.merchant_amount, 19_400);
        assert_eq!(stored.platform_fee_bps, 250);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.amount, 20_000);
        assert!(env.events().all().len() >= before + 2);
    }

    #[test]
    fn calculates_split_without_storing_reference() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500); // Because default is 100 bps
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);
    }

    #[test]
    #[should_panic]
    fn rejects_duplicate_merchant() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        client.register_merchant(&merchant);
    }

    #[test]
    #[should_panic]
    fn rejects_duplicate_payment_reference() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[1; 32]);
        client.store_payment_reference(&merchant, &reference, &1000);
        client.store_payment_reference(&merchant, &reference, &2000);
    }

    #[test]
    #[should_panic]
    fn rejects_invalid_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[2; 32]);
        client.store_payment_reference(&merchant, &reference, &0);
    }

    #[test]
    #[should_panic]
    fn rejects_invalid_fee_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let bad_rule = SettlementRule {
            platform_fee_bps: 10_001,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    #[test]
    fn admin_clears_custom_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: true,
        };
        client.set_settlement_rule(&merchant, &rule);

        let prev_count = env.events().all().len();
        client.clear_settlement_rule(&merchant);

        // Storage key is gone: getter returns None
        assert!(client.get_settlement_rule(&merchant).is_none());

        // Event check
        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_cleared")
        );
        assert_eq!(
            Address::from_val(&env, &topics.get(1).unwrap()),
            merchant
        );
    }

    #[test]
    fn clearing_rule_falls_back_to_defaults() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 200,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_settlement_rule(&merchant, &rule);

        // Clear the custom rule
        client.clear_settlement_rule(&merchant);

        // calculate_fee_split should now use default rates (100 bps platform, 0 bps network)
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500); // 100 bps of 50_000
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);
    }

    #[test]
    #[should_panic]
    fn clear_settlement_rule_fails_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let merchant = Address::generate(&env);
        let contract_id: Address = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

        // Authorize admin for init
        let invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: soroban_sdk::vec![&env, admin.to_val()],
            sub_invokes: &[],
        };
        let auth = MockAuth {
            address: &admin,
            invoke: &invoke,
        };
        env.set_auths(&[(&auth).into()]);
        client.init(&admin);

        // Authorize admin for register_merchant
        let reg_invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_merchant",
            args: soroban_sdk::vec![&env, merchant.to_val()],
            sub_invokes: &[],
        };
        let reg_auth = MockAuth {
            address: &admin,
            invoke: &reg_invoke,
        };
        env.set_auths(&[(&reg_auth).into()]);
        client.register_merchant(&merchant);

        // Do NOT authorize admin for clear_settlement_rule — should panic
        client.clear_settlement_rule(&merchant);
    }

    #[test]
    #[should_panic]
    fn clear_settlement_rule_fails_when_not_set() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        client.clear_settlement_rule(&merchant);
    }
}
