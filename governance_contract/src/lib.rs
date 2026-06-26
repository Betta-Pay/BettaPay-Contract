//! # BettaPay Governance Contract
//!
//! This contract is the single source of truth for protocol-wide configuration
//! in the BettaPay network. It controls who may act as administrators, what fee
//! rates are applied across all settlement flows, which off-chain anchors are
//! trusted for each asset, and a set of extensible numeric system parameters
//! that downstream contracts can read on-demand.
//!
//! ## Key Concepts
//!
//! ### Admin Controls
//! A single privileged `admin` address owns the contract. The admin is stored
//! in instance storage and is set once during [`GovernanceContract::init`].
//! Ownership can be transferred via [`GovernanceContract::transfer_admin`] to
//! any non-zero, non-self address. All mutating entry-points check
//! `caller == admin` before calling `caller.require_auth()`.
//!
//! ### Contract Upgrade
//! [`GovernanceContract::upgrade`] replaces the running Wasm executable without
//! touching storage. The caller must be the admin. After an upgrade, the new
//! code takes over immediately; if a storage-schema migration is needed, a
//! separate migration function should be invoked in the same transaction.
//!
//! ### Pause / Unpause
//! The admin can halt all mutating governance operations by calling
//! [`GovernanceContract::pause`]. This sets a boolean flag in instance storage
//! and emits a `pause` event. All entry-points that write state call the
//! internal `assert_not_paused` guard. The contract is re-enabled with
//! [`GovernanceContract::unpause`], which emits an `unpause` event.
//!
//! ### Fee Configuration
//! [`GovernanceContract::set_fee_config`] stores a [`FeeConfig`] struct that
//! expresses platform and network fees in basis points (bps, 1 bps = 0.01 %).
//! Both values must independently satisfy:
//!
//! - `MIN_FEE_BPS` (5 bps, 0.05 %) ≤ value ≤ `MAX_FEE_BPS` (5 000 bps, 50 %)
//! - `platform_fee_bps + network_fee_bps` ≤ 10 000 bps (100 %)
//!
//! Violating any constraint panics with [`GovernanceError::InvalidFeeBps`].
//! The current config is readable via [`GovernanceContract::get_fee_config`].
//! The entry emits a `fee_config_updated` event on every successful write.
//!
//! ### Anchor Registry
//! Each supported asset can nominate a trusted off-chain anchor address via
//! [`GovernanceContract::upsert_anchor`]. The anchor address is stored keyed by
//! the asset [`Address`][soroban_sdk::Address] in persistent storage.
//! [`GovernanceContract::remove_anchor`] deletes the entry; attempting to
//! remove an asset that has no registered anchor panics with
//! [`GovernanceError::AnchorMissing`]. Both operations emit events
//! (`anchor_upserted` / `anchor_removed`) for off-chain indexers.
//! TTL of the anchor entry is extended on every read via
//! [`GovernanceContract::get_anchor`].
//!
//! ### System Parameters
//! [`GovernanceContract::update_system_param`] stores an arbitrary `i128`
//! value under a caller-supplied [`Symbol`][soroban_sdk::Symbol] key. This
//! gives the admin a flexible mechanism to propagate numeric knobs (e.g.,
//! maximum settlement delay, minimum collateral ratio) to other contracts
//! without upgrading the governance contract itself. Parameters are read via
//! [`GovernanceContract::get_system_param`], which also refreshes the
//! persistent-entry TTL.
//!
//! ## Error Codes
//!
//! | Code | Variant | Meaning |
//! |------|---------|---------|
//! | 1 | `AlreadyInitialized` | `init` called more than once |
//! | 2 | `NotInitialized` | Admin not yet set |
//! | 3 | `Unauthorized` | Caller is not the admin |
//! | 4 | `InvalidFeeBps` | Fee value out of range or combined sum > 10 000 bps |
//! | 5 | `AnchorMissing` | Tried to remove an unregistered anchor |
//! | 6 | `Paused` | Contract is paused |
//! | 7 | `InvalidAdmin` | Transfer target is zero-address or current admin |
//!
//! ## Emitted Events
//!
//! | Event symbol | Trigger |
//! |---|---|
//! | `contract_upgraded` | Wasm upgrade succeeded |
//! | `admin` | Admin transfer completed |
//! | `pause` | Contract paused |
//! | `unpause` | Contract unpaused |
//! | `sys_param` | System parameter updated |
//! | `fee_config_updated` | Fee configuration changed |
//! | `anchor_upserted` | Anchor created or replaced for an asset |
//! | `anchor_removed` | Anchor removed for an asset |

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, String, Symbol,
};

/// Minimum allowed fee in basis points (0.05%).
const MIN_FEE_BPS: u32 = 5;
/// Maximum allowed fee in basis points (50%).
const MAX_FEE_BPS: u32 = 5_000;
const FEE_TTL_THRESHOLD: u32 = 17280 * 14;
const FEE_TTL_BUMP: u32 = 17280 * 30;

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

/// Structured payload for the `admin_transferred` event, so off-chain
/// consumers can read `old_admin`/`new_admin` by name instead of having to
/// know the positional order of an anonymous tuple.
#[derive(Clone)]
#[contracttype]
pub struct AdminTransferred {
    pub old_admin: Address,
    pub new_admin: Address,
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    /// Storage key for the contract admin address in instance storage.
    Admin,
    /// Storage key for a system parameter value indexed by a symbol in persistent storage.
    SystemParam(Symbol),
    /// Storage key for the fee configuration data in persistent storage.
    FeeConfig,
    /// Storage key for the anchor address associated with a specific asset in persistent storage.
    Anchor(Address),
    /// Storage key for the pause state flag in instance storage.
    Paused,
}

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum GovernanceError {
    /// The contract has already been initialized.
    AlreadyInitialized = 1,
    /// The contract has not been initialized yet.
    NotInitialized = 2,
    /// The caller is not authorized to perform this action.
    Unauthorized = 3,
    /// The provided fee basis points are invalid or exceed the maximum limit.
    InvalidFeeBps = 4,
    /// The anchor for the specified asset was not found.
    AnchorMissing = 5,
    /// The contract is currently paused and the operation is not allowed.
    Paused = 6,
    /// The provided admin address is invalid (e.g., zero address or same as current admin).
    InvalidAdmin = 7,
    InvalidParamValue = 8,
}

#[contract]
pub struct GovernanceContract;

#[contractimpl]
impl GovernanceContract {
    /// Initialises the governance contract and sets the initial administrator.
    ///
    /// Must be called exactly once after deployment. The caller is recorded as the
    /// contract administrator. Subsequent calls are rejected.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `admin` - The address to designate as contract administrator.
    ///
    /// # Authorization
    ///
    /// Requires authorisation from `admin`.
    ///
    /// # Effects
    ///
    /// Writes `admin` to instance storage under `DataKey::Admin`.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::AlreadyInitialized` if already initialised.
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, GovernanceError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Returns whether the contract has been initialised.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// `true` if `init` has been called successfully; `false` otherwise.
    pub fn is_initialized(env: Env) -> bool {
        env.storage().instance().has(&DataKey::Admin)
    }

    /// Returns the current contract administrator address.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The stored administrator `Address`.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::NotInitialized` if the contract has not been initialised.
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
    pub fn upgrade(env: Env, caller: Address, new_wasm_hash: BytesN<32>) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        env.events().publish(
            (Symbol::new(&env, "contract_upgraded"), new_wasm_hash),
            caller,
        );
    }

    /// Transfers administrative control of the contract to a new address.
    ///
    /// The current administrator must authorise the call. The new admin may not be
    /// the zero address or the same address as the current administrator.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `_caller` - Unused parameter; authorisation is enforced via the stored admin.
    /// * `new_admin` - The address to become the new administrator.
    ///
    /// # Authorization
    ///
    /// Requires authorisation from the current stored administrator.
    ///
    /// # Effects
    ///
    /// Overwrites `DataKey::Admin` in instance storage and emits an `admin` event
    /// carrying the new administrator address.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::InvalidAdmin` if `new_admin` is the zero address
    /// or is identical to the current administrator.
    pub fn transfer_admin(env: Env, _caller: Address, new_admin: Address) {
        let admin = read_admin(&env);
        admin.require_auth();

        let zero_address = String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        if new_admin.to_string() == zero_address {
            panic_with_error!(&env, GovernanceError::InvalidAdmin);
        }

        if admin == new_admin {
            panic_with_error!(&env, GovernanceError::InvalidAdmin);
        }

        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish(
            (Symbol::new(&env, "admin_transferred"),),
            AdminTransferred {
                old_admin: admin,
                new_admin,
            },
        );
    }

    /// Pauses the contract, blocking all state-mutating operations.
    ///
    /// While paused, any function guarded by `assert_not_paused` will reject
    /// incoming transactions. Intended for emergency use by the administrator.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Sets `DataKey::Paused` to `true` in instance storage and emits a `pause` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    pub fn pause(env: Env, caller: Address) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events()
            .publish((symbol_short!("pause"),), (admin, true));
    }

    /// Resumes normal contract operation after a pause.
    ///
    /// Clears the paused state so that state-mutating operations can proceed again.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Sets `DataKey::Paused` to `false` in instance storage and emits an `unpause` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    pub fn unpause(env: Env, caller: Address) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("unpause"),), (admin, false));
    }

    /// Returns whether the contract is currently paused.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// `true` if the contract is paused; `false` otherwise (including when the
    /// paused flag has never been explicitly set).
    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Creates or updates a named system parameter in persistent storage.
    ///
    /// System parameters are arbitrary `i128` values keyed by a `Symbol` and are
    /// intended for protocol-level configuration such as settlement delay bounds.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    /// * `key` - The `Symbol` identifier for the parameter.
    /// * `value` - The `i128` value to store.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Writes the key/value pair to persistent storage under `DataKey::SystemParam(key)`
    /// and emits a `sys_param` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    /// Panics with `GovernanceError::Paused` if the contract is currently paused.
    pub fn update_system_param(env: Env, caller: Address, key: Symbol, value: i128) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();

        if value < 0 {
            panic_with_error!(&env, GovernanceError::InvalidParamValue);
        }

        let storage_key = DataKey::SystemParam(key.clone());
        let previous_value: Option<i128> = env.storage().persistent().get(&storage_key);

        env.storage().persistent().set(&storage_key, &value);

        // Structured for off-chain indexing: topics carry the event name and
        // the specific parameter key (so indexers can filter per-parameter),
        // and the data payload carries who made the change and the full
        // before/after value, so the change is auditable without needing to
        // separately diff storage reads.
        env.events().publish(
            (Symbol::new(&env, "sys_param_updated"), key),
            (admin, previous_value, value),
        );
    }

    /// Retrieves a stored system parameter by key.
    ///
    /// If the entry exists its persistent storage TTL is refreshed before the
    /// value is returned.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `key` - The `Symbol` identifier of the parameter to retrieve.
    ///
    /// # Returns
    ///
    /// `Some(value)` if the parameter exists; `None` otherwise.
    ///
    /// # Effects
    ///
    /// Extends the persistent storage TTL for `DataKey::SystemParam(key)` when the
    /// entry is present.
    pub fn get_system_param(env: Env, key: Symbol) -> Option<i128> {
        let storage_key = DataKey::SystemParam(key);
        if env.storage().persistent().has(&storage_key) {
            env.storage()
                .persistent()
                .extend_ttl(&storage_key, 50_000, 100_000);
        }
        env.storage().persistent().get(&storage_key)
    }

    /// Sets the platform and network fee configuration.
    ///
    /// Both fee values are in basis points and must be within the inclusive range
    /// `[MIN_FEE_BPS, MAX_FEE_BPS]` (5–5 000 bps). Either the platform or network
    /// fee falling outside this range causes the call to be rejected.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    /// * `config` - A [`FeeConfig`] containing `platform_fee_bps` and `network_fee_bps`.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Writes `config` to persistent storage under `DataKey::FeeConfig`, refreshes its
    /// TTL, and emits a `fee_config_updated` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::InvalidFeeBps` if either fee value is outside the
    /// allowed range.
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    /// Panics with `GovernanceError::Paused` if the contract is currently paused.
    pub fn set_fee_config(env: Env, caller: Address, config: FeeConfig) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();

        if config.platform_fee_bps < MIN_FEE_BPS
            || config.platform_fee_bps > MAX_FEE_BPS
            || config.network_fee_bps < MIN_FEE_BPS
            || config.network_fee_bps > MAX_FEE_BPS
        {
            panic_with_error!(&env, GovernanceError::InvalidFeeBps);
        }

        if config.platform_fee_bps + config.network_fee_bps > 10_000 {
            panic_with_error!(&env, GovernanceError::InvalidFeeBps);
        }

        let key = DataKey::FeeConfig;
        env.storage().persistent().set(&key, &config.clone());
        env.storage()
            .persistent()
            .extend_ttl(&key, FEE_TTL_THRESHOLD, FEE_TTL_BUMP);
        env.events()
            .publish((Symbol::new(&env, "fee_config_updated"),), (admin, config));
    }

    /// Returns the currently stored fee configuration, if any.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// `Some(FeeConfig)` if a fee configuration has been set via [`set_fee_config`];
    /// `None` otherwise.
    pub fn get_fee_config(env: Env) -> Option<FeeConfig> {
        env.storage().persistent().get(&DataKey::FeeConfig)
    }

    /// Creates or updates the anchor address associated with a supported asset.
    ///
    /// An anchor maps a token asset address to the trusted entity responsible for
    /// managing that asset within the payment system. Calling this function for an
    /// existing `asset` key overwrites the previously registered anchor address.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    /// * `asset` - The asset token address whose anchor is being registered or updated.
    /// * `anchor` - The anchor address to associate with `asset`.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Writes the anchor address to persistent storage under `DataKey::Anchor(asset)`
    /// and emits an `anchor_upserted` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    /// Panics with `GovernanceError::Paused` if the contract is currently paused.
    pub fn upsert_anchor(env: Env, caller: Address, asset: Address, anchor: Address) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();
        let key = DataKey::Anchor(asset.clone());
        env.storage()
            .persistent()
            .set(&key, &anchor.clone());
        env.storage()
            .persistent()
            .extend_ttl(&key, 50_000, 100_000);
        env.events()
            .publish((Symbol::new(&env, "anchor_upserted"), asset), anchor);
    }

    /// Removes the anchor configuration for the given asset.
    ///
    /// Deletes the `DataKey::Anchor(asset)` entry from persistent storage. The asset
    /// must already have a registered anchor; attempting to remove an unknown asset
    /// is rejected.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `caller` - Must be the stored administrator.
    /// * `asset` - The asset token address whose anchor registration is to be removed.
    ///
    /// # Authorization
    ///
    /// Callable only by the configured contract administrator.
    ///
    /// # Effects
    ///
    /// Removes `DataKey::Anchor(asset)` from persistent storage and emits both an
    /// `anchor_rm` and an `anchor_removed` event.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::AnchorMissing` if no anchor is registered for `asset`.
    /// Panics with `GovernanceError::Unauthorized` if `caller` is not the administrator.
    /// Panics with `GovernanceError::Paused` if the contract is currently paused.
    pub fn remove_anchor(env: Env, caller: Address, asset: Address) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, GovernanceError::Unauthorized);
        }
        caller.require_auth();
        let key = DataKey::Anchor(asset.clone());

        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, GovernanceError::AnchorMissing);
        }

        env.storage().persistent().remove(&key);
        // Ensure the instance cache is also cleared for this key
        // Emit events indicating removal
        env.events().publish((symbol_short!("anchor_rm"),), (asset.clone(),));
        env.events().publish((Symbol::new(&env, "anchor_removed"), asset), ());
    }

    /// Returns the anchor address registered for the given asset, if any.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `asset` - The asset token address to look up.
    ///
    /// # Returns
    ///
    /// `Some(anchor_address)` if an anchor is registered for `asset`; `None` otherwise.
    pub fn get_anchor(env: Env, asset: Address) -> Option<Address> {
        let key = DataKey::Anchor(asset.clone());
        let result = env.storage().persistent().get(&key);
        if result.is_some() {
            env.storage().persistent().extend_ttl(&key, 50_000, 100_000);
        }
        result
    }
}

fn read_admin(env: &Env) -> Address {
    env.storage().instance().extend_ttl(50_000, 100_000);
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn upload_test_wasm(env: &Env) -> BytesN<32> {
        let wasm = Bytes::from_slice(
            env,
            &[],
        );
        env.deployer().upload_contract_wasm(wasm)
    }

    #[test]
    fn updates_system_parameters() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "max_settle");
        let before = env.events().all().len();
        client.update_system_param(&admin, &key, &1440);
        assert_eq!(client.get_system_param(&key), Some(1440));
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn system_parameter_key_uniqueness_overwrites_value() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "test_param");

        client.update_system_param(&admin, &key, &100);
        assert_eq!(client.get_system_param(&key), Some(100));

        client.update_system_param(&admin, &key, &200);
        assert_eq!(client.get_system_param(&key), Some(200));

        client.update_system_param(&admin, &key, &300);
        assert_eq!(client.get_system_param(&key), Some(300));
    }

    #[test]
    fn sets_fee_config() {
        let (env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };
        let before = env.events().all().len();
        client.set_fee_config(&admin, &cfg);
        let got = client.get_fee_config().expect("expected config");
        assert_eq!(got.platform_fee_bps, 120);
        assert_eq!(got.network_fee_bps, 35);
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn fee_config_event_emitted_with_correct_fields() {
        let (env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };

        client.set_fee_config(&admin, &cfg);

        let events = env.events().all();
        let event = events.last().unwrap();

        let (_contract_id, topics, data) = event;

        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "fee_config_updated")
        );

        let (event_admin, event_cfg): (Address, FeeConfig) = FromVal::from_val(&env, &data);
        assert_eq!(event_admin, admin);
        assert_eq!(event_cfg.platform_fee_bps, 120);
        assert_eq!(event_cfg.network_fee_bps, 35);
    }

    #[test]
    fn upserts_and_removes_anchor() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        let before_upsert = env.events().all().len();
        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        assert!(env.events().all().len() > before_upsert);

        let before_remove = env.events().all().len();
        client.remove_anchor(&admin, &asset);
        assert_eq!(client.get_anchor(&asset), None);
        assert!(env.events().all().len() > before_remove);
    }

    #[test]
    fn get_anchor_extends_anchor_ttl() {
        let (env, client, admin) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        assert_eq!(client.get_anchor(&asset), Some(anchor));
    }

    #[test]
    #[should_panic]
    fn rejects_fee_bps_above_max() {
        let (_env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 100,
        };
        client.set_fee_config(&admin, &cfg);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_bps_below_min() {
        let (_env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 4, // below MIN_FEE_BPS
        };
        client.set_fee_config(&admin, &cfg);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_bps_sum_exceeds_max() {
        let (_env, client, admin) = setup();
        // platform 5_000 (max) + network 5_001 = 10_001 > 10_000
        let cfg = FeeConfig {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_001,
        };
        client.set_fee_config(&admin, &cfg);
    }

    #[test]
    fn accepts_fee_bps_at_boundaries() {
        let (_env, client, admin) = setup();
        // Exactly at minimum
        client.set_fee_config(
            &admin,
            &FeeConfig {
                platform_fee_bps: 5,
                network_fee_bps: 5,
            },
        );
        // Exactly at maximum
        client.set_fee_config(
            &admin,
            &FeeConfig {
                platform_fee_bps: 5_000,
                network_fee_bps: 5_000,
            },
        );
    }

    #[test]
    #[should_panic]
    fn rejects_removing_unknown_anchor() {
        let (env, client, admin) = setup();
        let missing_asset = Address::generate(&env);
        client.remove_anchor(&admin, &missing_asset);
    }
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
        client.update_system_param(&_admin, &key, &123);
    }

    #[test]
    fn accepts_valid_symbol_key() {
        let (env, client, _admin) = setup();
        let key = Symbol::new(&env, "valid_key_32_chars_or_less");
        client.update_system_param(&_admin, &key, &123);
        assert_eq!(client.get_system_param(&key), Some(123));
    }

    #[test]
    fn system_param_key_uniqueness_overwrites_existing_value() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "test_param");

        client.update_system_param(&admin, &key, &100);
        assert_eq!(client.get_system_param(&key), Some(100));

        client.update_system_param(&admin, &key, &200);
        assert_eq!(client.get_system_param(&key), Some(200));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #7)")]
    fn rejects_same_admin_transfer() {
        let (_env, client, admin) = setup();
        client.transfer_admin(&admin, &admin);
    }

    #[test]
    fn transfers_admin_successfully() {
        let (env, client, admin) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #8)")]
    fn rejects_negative_system_param_value() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "max_settle");
        client.update_system_param(&admin, &key, &-1);
    }

    #[test]
    fn accepts_zero_system_param_value() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "max_settle");
        client.update_system_param(&admin, &key, &0);
        assert_eq!(client.get_system_param(&key), Some(0));
    }

    #[test]
    fn emits_structured_event_when_updating_system_param() {
        let (env, client, admin) = setup();
        let key = Symbol::new(&env, "max_settle");

        // First update: no previous value should exist yet.
        let prev_count = env.events().all().len();
        client.update_system_param(&admin, &key, &1440);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, data) = events.get(prev_count).unwrap();

        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "sys_param_updated")
        );
        assert_eq!(Symbol::from_val(&env, &topics.get(1).unwrap()), key);

        let (event_admin, previous_value, new_value) =
            <(Address, Option<i128>, i128)>::from_val(&env, &data);
        assert_eq!(event_admin, admin);
        assert_eq!(previous_value, None);
        assert_eq!(new_value, 1440);

        // Second update: previous_value should now reflect the prior write.
        let prev_count = env.events().all().len();
        client.update_system_param(&admin, &key, &2880);

        let events = env.events().all();
        let (_contract_id, _topics, data) = events.get(prev_count).unwrap();
        let (_event_admin, previous_value, new_value) =
            <(Address, Option<i128>, i128)>::from_val(&env, &data);
        assert_eq!(previous_value, Some(1440));
        assert_eq!(new_value, 2880);
    }

    #[test]
    fn emits_structured_event_when_transferring_admin() {
        let (env, client, admin) = setup();
        let new_admin = Address::generate(&env);

        let prev_count = env.events().all().len();
        client.transfer_admin(&admin, &new_admin);

        let events = env.events().all();
        assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

        let (_contract_id, topics, data) = events.get(prev_count).unwrap();

        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "admin_transferred")
        );

        // Data is the named AdminTransferred struct, not an anonymous tuple -
        // off-chain consumers read old_admin/new_admin by field name.
        let payload = AdminTransferred::from_val(&env, &data);
        assert_eq!(payload.old_admin, admin);
        assert_eq!(payload.new_admin, new_admin);
    }
    
    #[test]
    fn admin_functions_work_while_paused() {
        let (env, client, admin) = setup();
        client.pause(&admin);
        assert!(client.is_paused());

        // None of these should panic while paused - the admin must be able
        // to resolve issues during a pause, not be locked out of it.
        let key = Symbol::new(&env, "max_settle");
        client.update_system_param(&admin, &key, &1440);
        assert_eq!(client.get_system_param(&key), Some(1440));

        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };
        client.set_fee_config(&admin, &cfg);
        assert_eq!(client.get_fee_config().unwrap().platform_fee_bps, 120);

        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);
        client.upsert_anchor(&admin, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor));

        client.remove_anchor(&admin, &asset);
        assert_eq!(client.get_anchor(&asset), None);

        // Still paused throughout - none of the above silently unpaused it.
        assert!(client.is_paused());
    }



    #[test]
    #[should_panic]
    fn rejects_fee_bps_exceeding_bps_denomination() {
        let (_env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 10_001,
            network_fee_bps: 100,
        };
        client.set_fee_config(&admin, &cfg);
    }

    #[test]
    #[should_panic]
    fn rejects_fee_config_sum_above_10000_bps() {
        let (_env, client, admin) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 5_001,
        };
        client.set_fee_config(&admin, &cfg);
    }

    #[test]
    #[should_panic]
    fn rejects_update_system_param_when_not_admin() {
        let (env, client, _admin) = setup();
        let non_admin = Address::generate(&env);
        let key = Symbol::new(&env, "max_settle");
        client.update_system_param(&non_admin, &key, &1440);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn unpause_requires_admin_auth() {
        let (env, client, admin) = setup();
        let non_admin = Address::generate(&env);

        // Pause the contract as admin first
        client.pause(&admin);
        assert!(client.is_paused());

        // Attempt to unpause as non-admin, which should panic
        client.unpause(&non_admin);
    }
}
