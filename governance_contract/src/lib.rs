#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, Symbol,
};

/// Minimum allowed fee in basis points (0.05%).
const MIN_FEE_BPS: u32 = 5;
/// Maximum allowed fee in basis points (50%).
const MAX_FEE_BPS: u32 = 5_000;

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    Admin,
    SystemParam(Symbol),
    FeeConfig,
    Anchor(Address),
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidFeeBps = 4,
    AnchorMissing = 5,
    Paused = 6,
    InvalidSystemParamKey = 7,
    InvalidAdmin = 7,
}

#[contract]
pub struct GovernanceContract;

#[contractimpl]
impl GovernanceContract {
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, GovernanceError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    pub fn is_initialized(env: Env) -> bool {
        env.storage().instance().has(&DataKey::Admin)
    }

    pub fn get_admin(env: Env) -> Address {
        read_admin(&env)
    }

    /// Upgrades the contract Wasm code to a new version.
    ///
    /// This function replaces only the contract's executable Wasm code;
    /// all persistent and instance storage entries remain intact. A
    /// separate storage-migration function should be written and called
    /// after the upgrade if the new code expects a different schema.
    ///
    /// ### Events
    /// - Emits `contract_upgraded` with topic
    ///   `(Symbol("contract_upgraded"), new_wasm_hash)` and data
    ///   `(caller)`.
    ///
    /// ### Panics
    /// - If the caller is not the stored admin.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let admin = read_admin(&env);
        admin.require_auth();
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        env.events().publish(
            (Symbol::new(&env, "contract_upgraded"), new_wasm_hash),
            admin,
        );
    }

    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin = read_admin(&env);
        admin.require_auth();
        
        if admin == new_admin {
            panic_with_error!(&env, GovernanceError::InvalidAdmin);
        }
        
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

    pub fn update_system_param(env: Env, key: Symbol, value: i128) {
        assert_not_paused(&env);
        if key.to_string(&env).len() > 32 {
            panic_with_error!(&env, GovernanceError::InvalidSystemParamKey);
        }
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::SystemParam(key.clone()), &value);
        env.events()
            .publish((symbol_short!("sys_param"), key), value);
    }

    pub fn get_system_param(env: Env, key: Symbol) -> Option<i128> {
        let storage_key = DataKey::SystemParam(key);
        if env.storage().persistent().has(&storage_key) {
            env.storage().persistent().extend_ttl(&storage_key, 50_000, 100_000);
        }
        env.storage().persistent().get(&storage_key)
    }

    pub fn set_fee_config(env: Env, config: FeeConfig) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if config.platform_fee_bps < MIN_FEE_BPS
            || config.platform_fee_bps > MAX_FEE_BPS
            || config.network_fee_bps < MIN_FEE_BPS
            || config.network_fee_bps > MAX_FEE_BPS
        {
            panic_with_error!(&env, GovernanceError::InvalidFeeBps);
        }

        env.storage()
            .persistent()
            .set(&DataKey::FeeConfig, &config.clone());
        env.events().publish(
            (symbol_short!("fee_cfg"),),
            (config.platform_fee_bps, config.network_fee_bps),
        );
    }

    pub fn get_fee_config(env: Env) -> Option<FeeConfig> {
        env.storage().persistent().get(&DataKey::FeeConfig)
    }

    pub fn upsert_anchor(env: Env, asset: Address, anchor: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Anchor(asset.clone()), &anchor.clone());
        env.events()
            .publish((symbol_short!("anchor_up"), asset), anchor);
    }

    pub fn remove_anchor(env: Env, asset: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();
        let key = DataKey::Anchor(asset.clone());

        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, GovernanceError::AnchorMissing);
        }

        env.storage().persistent().remove(&key);
        env.events()
            .publish((symbol_short!("anchor_rm"), asset), true);
        env.events().publish(
            (Symbol::new(&env, "anchor_removed"), asset),
            true,
        );
    }

    pub fn get_anchor(env: Env, asset: Address) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Anchor(asset))
    }
}

fn read_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, GovernanceError::NotInitialized))
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn assert_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, GovernanceError::Paused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
    use soroban_sdk::{vec, Bytes, FromVal};

    fn setup() -> (Env, GovernanceContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
        client.init(&admin);
        (env, client, admin)
    }

    fn setup_no_mock() -> (Env, GovernanceContractClient<'static>, Address) {
        let env = Env::default();
        let admin = Address::generate(&env);
        let contract_id: Address = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        let invoke = MockAuthInvoke {
            contract: &contract_id,
            fn_name: "init",
            args: vec![&env, admin.to_val()],
            sub_invokes: &[],
        };
        let auth = MockAuth {
            address: &admin,
            invoke: &invoke,
        };
        env.set_auths(&[(&auth).into()]);
        client.init(&admin);
        (env, client, admin)
    }

    fn upload_test_wasm(env: &Env) -> BytesN<32> {
        let wasm = Bytes::from_slice(
            env,
            include_bytes!("../../target/wasm32-unknown-unknown/release/governance_contract.wasm"),
        );
        env.deployer().upload_contract_wasm(wasm)
    }

    #[test]
    fn updates_system_parameters() {
        let (env, client, _admin) = setup();
        let key = Symbol::new(&env, "max_settle");
        let before = env.events().all().len();
        client.update_system_param(&key, &1440);
        assert_eq!(client.get_system_param(&key), Some(1440));
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn sets_fee_config() {
        let (env, client, _admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };
        let before = env.events().all().len();
        client.set_fee_config(&cfg);
        let got = client.get_fee_config().expect("expected config");
        assert_eq!(got.platform_fee_bps, 120);
        assert_eq!(got.network_fee_bps, 35);
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn upserts_and_removes_anchor() {
        let (env, client, _admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        let before_upsert = env.events().all().len();
        client.upsert_anchor(&asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        assert!(env.events().all().len() > before_upsert);

        let before_remove = env.events().all().len();
        client.remove_anchor(&asset);
        assert_eq!(client.get_anchor(&asset), None);
        assert!(env.events().all().len() > before_remove);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_bps_above_max() {
        let (_env, client, _admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 100,
        };
        client.set_fee_config(&cfg);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_bps_below_min() {
        let (_env, client, _admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 4, // below MIN_FEE_BPS
        };
        client.set_fee_config(&cfg);
    }

    #[test]
    fn accepts_fee_bps_at_boundaries() {
        let (_env, client, _admin) = setup();
        // Exactly at minimum
        client.set_fee_config(&FeeConfig {
            platform_fee_bps: 5,
            network_fee_bps: 5,
        });
        // Exactly at maximum
        client.set_fee_config(&FeeConfig {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_000,
        });
    }

    #[test]
    #[should_panic]
    fn rejects_removing_unknown_anchor() {
        let (env, client, _admin) = setup();
        let missing_asset = Address::generate(&env);
        client.remove_anchor(&missing_asset);
    }

    #[test]
    #[should_panic]
    fn rejects_reinitialization() {
        let (env, client, admin) = setup();
        client.init(&admin);
        let _ = env;
    }

    #[test]
    fn checks_if_initialized() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(!client.is_initialized());
        client.init(&admin);
        assert!(client.is_initialized());
    }

    #[test]
    #[should_panic]
    fn rejects_oversized_symbol_key() {
        let (env, client, _admin) = setup();
        // A string longer than 32 characters
        let oversized = "this_is_a_very_long_system_parameter_key";
        let key = Symbol::new(&env, oversized);
        client.update_system_param(&key, &123);
    }

    #[test]
    fn accepts_valid_symbol_key() {
        let (env, client, _admin) = setup();
        let key = Symbol::new(&env, "valid_key_32_chars_or_less");
        client.update_system_param(&key, &123);
        assert_eq!(client.get_system_param(&key), Some(123));
    #[should_panic(expected = "Error(Contract, #7)")]
    fn rejects_same_admin_transfer() {
        let (_env, client, admin) = setup();
        client.transfer_admin(&admin);
    }

    #[test]
    fn transfers_admin_successfully() {
        let (env, client, _admin) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);
    }
}
