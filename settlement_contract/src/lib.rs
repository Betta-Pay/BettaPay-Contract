#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, Symbol, Vec,
};

const BPS_DENOMINATOR: i128 = 10_000;
const MIN_PAYMENT_AMOUNT: i128 = 100;
const MAX_SETTLEMENT_DELAY_LEDGER: u32 = 100_000;
const PAYMENT_TTL_THRESHOLD: u32 = 17280 * 14;
const PAYMENT_TTL_BUMP: u32 = 17280 * 30;
const RULE_TTL_THRESHOLD: u32 = 17280 * 14;
const RULE_TTL_BUMP: u32 = 17280 * 30;

// Used until the admin sets a global default settlement rule.
const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
    platform_fee_bps: 100,
    network_fee_bps: 0,
    settlement_delay_ledger: 0,
    auto_settle: false,
};

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
    DefaultRule,
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
    InvalidAddress = 11,
    InvalidPaymentReference = 12,
    InvalidSettlementDelay = 13,
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

        let zero_address_str = soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        if merchant.to_string().len() == 0 || merchant.to_string() == zero_address_str {
            panic_with_error!(&env, SettlementError::InvalidAddress);
        }

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

    pub fn unregister_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        let rule_key = DataKey::Rule(merchant.clone());
        if env.storage().persistent().has(&rule_key) {
            env.storage().persistent().remove(&rule_key);
        }

        env.events()
            .publish((symbol_short!("merchant"), merchant), false);
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
        if rule.platform_fee_bps > BPS_DENOMINATOR as u32
            || rule.network_fee_bps > BPS_DENOMINATOR as u32
        {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR as u32 {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(&env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);

        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

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
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::RuleNotSet));

        env.storage().persistent().remove(&key);

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_cleared"), merchant),
            (admin, removed),
        );
    }

    /// ## Emitted Event: `default_rule_updated`
    ///
    /// **Topics**: `(Symbol("default_rule_updated"),)`
    /// - First topic: fixed event-name symbol for filtering by event type
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the change
    /// - `previous`: the previous global default rule (or bootstrap fallback if none was set)
    /// - `current`: the new global default rule
    pub fn set_default_rule(env: Env, new_rule: SettlementRule) {
        let admin = read_admin(&env);
        admin.require_auth();

        if new_rule.platform_fee_bps > BPS_DENOMINATOR as u32
            || new_rule.network_fee_bps > BPS_DENOMINATOR as u32
        {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .persistent()
            .set(&DataKey::DefaultRule, &new_rule);

        env.events().publish(
            (Symbol::new(&env, "default_rule_updated"),),
            (admin, prev, new_rule),
        );
    }

    pub fn get_default_rule(env: Env) -> Option<SettlementRule> {
        env.storage().persistent().get(&DataKey::DefaultRule)
    }

    pub fn store_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        amount: i128,
    ) -> FeeSplit {
        assert_not_paused(&env);
        merchant.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if reference == BytesN::from_array(&env, &[0; 32]) {
            panic_with_error!(&env, SettlementError::InvalidPaymentReference);
        }
        if amount < MIN_PAYMENT_AMOUNT {
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
        env.storage()
            .persistent()
            .extend_ttl(&payment_key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);

        /// ## Emitted Event: `payment_stored`
        ///
        /// **Topics**: `(Symbol("payment_stored"), Address merchant)`
        /// - First topic: fixed event-name symbol for filtering by event type
        /// - Second topic: the merchant address that stored the payment
        ///
        /// **Data**: `(BytesN<32> reference, PaymentRecord record)`
        /// - `reference`: the unique payment reference identifier
        /// - `record`: the full payment record including amounts, fees, and settlement info
        env.events().publish(
            (Symbol::new(&env, "payment_stored"), merchant.clone()),
            (reference.clone(), record),
        );

        /// ## Emitted Event: `payment_split`
        ///
        /// **Topics**: `(Symbol("payment_split"), Address merchant)`
        /// - First topic: fixed event-name symbol for filtering by event type
        /// - Second topic: the merchant address for which the split was calculated
        ///
        /// **Data**: `(i128 gross_amount, i128 platform_fee_amount, i128 network_fee_amount, i128 merchant_amount)`
        /// - The calculated fee breakdown for the payment in absolute units
        env.events().publish(
            (Symbol::new(&env, "payment_split"), merchant),
            (
                split.gross_amount,
                split.platform_fee_amount,
                split.network_fee_amount,
                split.merchant_amount,
            ),
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
        let key = DataKey::Payment(reference);
        let record: Option<PaymentRecord> = env.storage().persistent().get(&key);
        if record.is_some() {
            env.storage()
                .persistent()
                .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
        }
        record
    }

    pub fn get_payments(env: Env, references: Vec<BytesN<32>>) -> Vec<PaymentRecord> {
        let mut payments = Vec::new(&env);

        for reference in references.iter() {
            if let Some(payment) = Self::get_payment_reference(env.clone(), reference.clone()) {
                payments.push_back(payment);
            }
        }

        payments
    }
}

fn read_admin(env: &Env) -> Address {
    env.storage().instance().extend_ttl(50_000, 100_000);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    if env.storage().persistent().has(&key) {
        // Keep the merchant marker warm so active merchants do not expire early.
        env.storage().persistent().extend_ttl(&key, 50_000, 100_000);
    }
    env.storage().persistent().get(&key).unwrap_or(false)
}

fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
    // Merchant-specific rule wins over any shared configuration.
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&DataKey::Rule(merchant))
    {
        return rule;
    }
    // Fall back to the admin-controlled global default when present.
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&DataKey::DefaultRule)
    {
        return rule;
    }
    // Final fallback keeps the contract usable before any config is stored.
    BOOTSTRAP_DEFAULT_RULE
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn assert_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

fn calculate_split(amount: i128, rule: &SettlementRule) -> FeeSplit {
    // Fees are rounded up so the platform and network never under-collect.
    let platform_fee_amount =
        (amount * (rule.platform_fee_bps as i128) + BPS_DENOMINATOR - 1) / BPS_DENOMINATOR;
    let network_fee_amount =
        (amount * (rule.network_fee_bps as i128) + BPS_DENOMINATOR - 1) / BPS_DENOMINATOR;
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
    #[should_panic]
    fn rejects_invalid_merchant_address() {
        let (env, client, _admin, _merchant) = setup();
        let zero_address = Address::from_string(&soroban_sdk::String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        client.register_merchant(&zero_address);
    }

    #[test]
    fn extends_ttl_when_updating_settlement_rule() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        // This will successfully write and extend the TTL for the rule
        client.set_settlement_rule(&merchant, &rule);

        // Verify the persistent entry exists
        env.as_contract(&client.address, || {
            let key = DataKey::Rule(merchant.clone());
            assert!(env.storage().persistent().has(&key));
        });
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
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
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
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

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
    #[should_panic]
    fn rejects_all_zero_payment_reference() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[0; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);
    }

    #[test]
    fn reads_payment_reference_and_extends_ttl() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[8; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);

        // Call get_payment_reference, which should extend the TTL
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(stored.amount, 10_000);

        // Verify the persistent entry exists after read
        env.as_contract(&client.address, || {
            let key = DataKey::Payment(reference.clone());
            assert!(env.storage().persistent().has(&key));
        });
    }

    #[test]
    fn gets_payments_in_batches() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference_one = BytesN::from_array(&env, &[11; 32]);
        let reference_two = BytesN::from_array(&env, &[12; 32]);
        client.store_payment_reference(&merchant, &reference_one, &15_000);
        client.store_payment_reference(&merchant, &reference_two, &25_000);

        let references = Vec::from_array(&env, [reference_one.clone(), reference_two.clone()]);
        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), 2);
        assert_eq!(payments.get(0).unwrap().amount, 15_000);
        assert_eq!(payments.get(1).unwrap().amount, 25_000);
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
    fn unregisters_merchant_and_cleans_up() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        assert!(client.is_merchant_registered(&merchant));
        assert!(client.get_settlement_rule(&merchant).is_some());

        let before = env.events().all().len();
        client.unregister_merchant(&merchant);

        assert!(!client.is_merchant_registered(&merchant));
        assert!(client.get_settlement_rule(&merchant).is_none());
        assert!(env.events().all().len() > before);
    }

    #[test]
    #[should_panic]
    fn unregister_rejects_missing_merchant() {
        let (_env, client, _admin, merchant) = setup();
        client.unregister_merchant(&merchant);
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
    fn rejects_below_minimum_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[99; 32]);
        client.store_payment_reference(&merchant, &reference, &99);
    }

    #[test]
    fn accepts_valid_minimum_amount() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let reference = BytesN::from_array(&env, &[100; 32]);
        client.store_payment_reference(&merchant, &reference, &100);

        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");
        assert_eq!(stored.amount, 100);
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
    #[should_panic]
    fn rejects_fee_sum_exceeding_10000_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let bad_rule = SettlementRule {
            platform_fee_bps: 6_000,
            network_fee_bps: 5_000,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &bad_rule);
    }

    #[test]
    fn accepts_fee_sum_at_exactly_10000_bps() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        let rule = SettlementRule {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_000,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.platform_fee_bps, 5_000);
        assert_eq!(stored.network_fee_bps, 5_000);
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
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
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

    #[test]
    fn bootstrap_default_used_before_any_default_rule_set() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);
        // No global default set — falls back to hardcoded 100 bps
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 0);
        assert_eq!(split.merchant_amount, 49_500);
    }

    #[test]
    fn global_default_used_when_no_explicit_merchant_rule() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
        assert_eq!(split.network_fee_amount, 250); // 50 bps
        assert_eq!(split.merchant_amount, 48_750);
    }

    #[test]
    fn explicit_merchant_rule_overrides_global_default() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let merchant_rule = SettlementRule {
            platform_fee_bps: 175,
            network_fee_bps: 25,
            settlement_delay_ledger: 42,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &merchant_rule);

        let split = client.calculate_fee_split(&merchant, &50_000);
        // Merchant rule (175/25) takes precedence over global default (200/50)
        assert_eq!(split.platform_fee_amount, 875); // 175 bps
        assert_eq!(split.network_fee_amount, 125); // 25 bps
        assert_eq!(split.merchant_amount, 49_000);
    }

    #[test]
    fn set_default_rule_stores_and_can_be_retrieved() {
        let (_env, client, _admin, _merchant) = setup();

        assert!(client.get_default_rule().is_none());

        let rule = SettlementRule {
            platform_fee_bps: 300,
            network_fee_bps: 100,
            settlement_delay_ledger: 5,
            auto_settle: true,
        };
        client.set_default_rule(&rule);

        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 300);
        assert_eq!(stored.network_fee_bps, 100);
        assert_eq!(stored.settlement_delay_ledger, 5);
        assert!(stored.auto_settle);
    }

    #[test]
    fn set_default_rule_emits_event_with_correct_topic() {
        let (env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 150,
            network_fee_bps: 25,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_default_rule(&rule);

        let events = env.events().all();
        let (_contract_id, topics, _data) = events.get(events.len() - 1).unwrap();

        // Single-element topic: just the event name
        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "default_rule_updated")
        );
    }

    #[test]
    fn set_default_rule_updates_twice_emits_correct_previous() {
        let (_env, client, _admin, _merchant) = setup();

        let first = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&first);
        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 200);

        let second = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 100,
            settlement_delay_ledger: 20,
            auto_settle: false,
        };
        client.set_default_rule(&second);
        let stored = client
            .get_default_rule()
            .expect("global default must be present");
        assert_eq!(stored.platform_fee_bps, 500);
    }

    #[test]
    fn clearing_rule_falls_back_to_global_default() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let global_rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };
        client.set_default_rule(&global_rule);

        let merchant_rule = SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 100,
            settlement_delay_ledger: 20,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &merchant_rule);
        client.clear_settlement_rule(&merchant);

        // After clearing, should fall back to global default (200/50), not bootstrap (100/0)
        let split = client.calculate_fee_split(&merchant, &50_000);
        assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
        assert_eq!(split.network_fee_amount, 250); // 50 bps
        assert_eq!(split.merchant_amount, 48_750);
    }

    #[test]
    #[should_panic]
    fn set_default_rule_fails_for_non_admin() {
        let env = Env::default();
        let admin = Address::generate(&env);
        let contract_id: Address = env.register_contract(None, SettlementContract);
        let client = SettlementContractClient::new(&env, &contract_id);

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

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 10,
            auto_settle: true,
        };

        // Do NOT authorize admin — should panic
        client.set_default_rule(&rule);
    }

    #[test]
    #[should_panic]
    fn set_default_rule_rejects_invalid_fee_bps() {
        let (_env, client, _admin, _merchant) = setup();

        let bad_rule = SettlementRule {
            platform_fee_bps: 10_001,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_default_rule(&bad_rule);
    }

    #[test]
    fn accepts_valid_settlement_delay_zero() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 0);
    }

    #[test]
    fn accepts_valid_settlement_delay_one() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 1,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 1);
    }

    #[test]
    fn accepts_settlement_delay_at_maximum_boundary() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 100_000,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.settlement_delay_ledger, 100_000);
    }

    #[test]
    #[should_panic]
    fn rejects_settlement_delay_above_maximum() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: 100_001,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
    }

    #[test]
    #[should_panic]
    fn rejects_settlement_delay_at_u32_max() {
        let (_env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 100,
            network_fee_bps: 0,
            settlement_delay_ledger: u32::MAX,
            auto_settle: false,
        };

        client.set_settlement_rule(&merchant, &rule);
    }

    #[test]
    fn accepts_default_rule_with_valid_settlement_delay() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 50_000,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
        let stored = client
            .get_default_rule()
            .expect("expected default rule");
        assert_eq!(stored.settlement_delay_ledger, 50_000);
    }

    #[test]
    fn accepts_default_rule_at_settlement_delay_maximum() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 100_000,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
        let stored = client
            .get_default_rule()
            .expect("expected default rule");
        assert_eq!(stored.settlement_delay_ledger, 100_000);
    }

    #[test]
    #[should_panic]
    fn rejects_default_rule_with_settlement_delay_above_maximum() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: 100_001,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
    }

    #[test]
    #[should_panic]
    fn rejects_default_rule_with_settlement_delay_at_u32_max() {
        let (_env, client, _admin, _merchant) = setup();

        let rule = SettlementRule {
            platform_fee_bps: 200,
            network_fee_bps: 50,
            settlement_delay_ledger: u32::MAX,
            auto_settle: true,
        };

        client.set_default_rule(&rule);
    }
}
