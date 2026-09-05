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
//! ### Upgrade Process
//! `upgrade` is safe because it ignores storage, which is also why changing a
//! stored type is a separate problem. Nothing converts existing entries, and
//! nothing checks that they still match the types the new code expects — a
//! mismatched read fails at runtime, after the upgrade has landed.
//!
//! 1. Wasm upgrades replace code only; every storage entry survives untouched.
//! 2. Storage migrations run **inside the upgraded contract**, as an
//!    admin-gated `migrate` entry point — not from a separate migration
//!    contract. A contract can only reach its own storage, so another contract
//!    has no access path to these entries.
//! 3. Ship the old type definition in the same Wasm as the new one. It is what
//!    makes existing entries readable while they are converted.
//! 4. Order is: upgrade the Wasm, then call `migrate`, then verify the
//!    post-upgrade state, then remove the migration code in a later upgrade.
//! 5. `Anchor(Address)` and `SystemParam(Symbol)` are keyed by value and
//!    Soroban cannot enumerate storage keys, so those cannot be migrated by
//!    iteration. Convert them lazily on read, or pass the keys in explicitly.
//!
//! Full guidance, including worked examples and the TTL hazards, is in
//! [`DEVELOPMENT.md`](https://github.com/Betta-Pay/BettaPay-Contract/blob/main/DEVELOPMENT.md).
//!
//! ### Pause / Unpause
//! The admin can halt all mutating governance operations by calling
//! [`GovernanceContract::pause`]. This sets a boolean flag in instance storage
//! and emits a `paused` event. All entry-points that write state call the
//! internal `assert_not_paused` guard. The contract is re-enabled with
//! [`GovernanceContract::unpause`], which emits an `unpaused` event.
//!
//! ## Pause Model
//! The pause flag blocks the anchor registry and fee configuration
//! (`upsert_anchor`, `remove_anchor`, `set_fee_config` all call
//! `assert_not_paused`). The following administrative operations are
//! intentionally NOT blocked during pause, so the admin can fix the root
//! cause of the emergency:
//! - `upgrade` — deploy a fix
//! - `transfer_admin` — rotate compromised keys
//! - `change_threshold` — re-balance the admin multisig
//! - `update_system_param` — adjust system configuration
//! - `initiate_recovery` / `cancel_recovery` / `execute_recovery` — repair a
//!   lost or corrupted admin set
//!
//! This matrix is pinned by `pause_blocks_fee_and_anchor_writes` and
//! `pause_allows_admin_transfer_threshold_and_recovery`. See also
//! [`adr/001-selective-pause-model.md`](https://github.com/Betta-Pay/BettaPay-Contract/blob/main/adr/001-selective-pause-model.md).
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
//! | 5 | `Paused` | Contract is paused |
//! | 6 | `InvalidAdmin` | Transfer target is zero-address or current admin |
//! | 7 | `InvalidRecoveryAddress` | Recovery address is zero-address or otherwise invalid |
//! | 8 | `RecoveryNotPending` | No recovery operation is currently pending |
//! | 9 | `RecoveryDelayActive` | Recovery delay period has not yet elapsed |
//! | 13 | `InvalidWasmInterface` | The deployed WASM does not implement the required interface |
//! | 14 | `InvalidThreshold` | The provided multisig threshold is invalid |
//! | 15 | `AlreadyPaused` | `pause` called while the contract was already paused |
//! | 16 | `AlreadyUnpaused` | `unpause` called while the contract was already unpaused |
//! | 200 | `AnchorMissing` | Tried to remove an unregistered anchor |
//! | 201 | `InvalidParamValue` | Supplied system parameter value is invalid or out of bounds |
//! | 204 | `SameAdmin` | Transfer target is identical to the current admin set and threshold |
//!
//! ## Event Conventions
//!
//! Events are emitted via [`soroban_sdk::Env::events`]. To give off-chain
//! indexers a predictable topic layout, every event in this contract follows
//! the same conventions:
//!
//! - `topic[0]` is always the event name as a [`Symbol`], constructed via
//!   [`Symbol::new`]. Indexers filter on this single topic to dispatch by
//!   event type.
//! - `topic[1..n]` carry the entity identifiers that scope the event —
//!   typically an [`Address`] (asset, admin, recovery address), but for some
//!   events also a [`BytesN<32>`] (new Wasm hash on `contract_upgraded`) or a
//!   [`Symbol`] (system-parameter key on `sys_param_updated`). The exact
//!   shape of `topic[1..n]` is fixed per event.
//! - The **data payload** carries the values describing the state change.
//!   Its shape is event-specific: a single value, a tuple, a typed struct
//!   such as [`AdminTransferred`], or `()`.
//! - Each entry point emits exactly the events tied to the state change it
//!   performs; no two events emitted by the same call describe the same
//!   logical change.
//!
//! ## Emitted Events
//!
//! | Event symbol | Trigger |
//! |---|---|
//! | `initialized` | Contract initialized |
//! | `contract_upgraded` | Wasm upgrade succeeded |
//! | `admin_transferred` | Admin transfer completed |
//! | `threshold_changed` | Multisig threshold changed |
//! | `paused` | Contract paused |
//! | `unpaused` | Contract unpaused |
//! | `recovery_initiated` / `recovery_cancelled` / `recovery_executed` | Admin-recovery lifecycle |
//! | `sys_param_updated` | System parameter updated |
//! | `fee_config_updated` | Fee configuration changed |
//! | `anchor_upserted` | Anchor created or replaced for an asset | Data: `(Option<Address> previous, Address current)` |
//! | `anchor_removed` | Anchor removed for an asset |

// TODO: Refactor flat file structure into modular hierarchy (Issue #84)
// Intended module structure:
// - mod types: Data structures (enums, structs)
// - mod storage: DataKey and storage access helpers
// - mod events: Event definitions and emission helpers
// - mod errors: Error enums
// - mod contract: Main contract trait implementation
// - mod test: Unit and integration tests

#![no_std]

use bettapay_common::{
    constants::{
        BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS, TTL_BUMP_LEDGERS,
        TTL_THRESHOLD_LEDGERS,
    },
    error_codes,
    events::{self, AdminTransferred, PendingRecovery},
    storage::{self, CommonDataKey},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, BytesN, Env,
    IntoVal, Symbol, TryFromVal, Val, Vec,
};

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

// TTL constants are kept locally aliased to `TTL_THRESHOLD_LEDGERS` /
// `TTL_BUMP_LEDGERS` so existing call sites and tests can keep referring to
// the named per-key constants.
const ANCHOR_TTL_THRESHOLD: u32 = TTL_THRESHOLD_LEDGERS;
const ANCHOR_TTL_BUMP: u32 = TTL_BUMP_LEDGERS;
const SYSTEM_PARAM_TTL_THRESHOLD: u32 = TTL_THRESHOLD_LEDGERS;
const SYSTEM_PARAM_TTL_BUMP: u32 = TTL_BUMP_LEDGERS;

// Instance-storage TTL policy for short-lived reads of non-`Admin` entries
// (`RecoveryAddress` here). Deliberately shorter than the 14/30 day policy
// above because these entries are only consulted during a recovery window,
// per `adr/003-ttl-value-selection.md`.
const READ_INSTANCE_TTL_THRESHOLD: u32 = 50_000;
const READ_INSTANCE_TTL_BUMP: u32 = 100_000;

// Admin, RecoveryAddress, PendingRecovery, and Paused live in
// `bettapay_common::storage::CommonDataKey` instead of here - see that
// type's doc comment for why a shared key type is safe to mix with this
// contract's own storage without a migration.
//
// The schema-version marker (issue #507) is instance storage and is written
// at `init`, so the first real storage migration has a defined baseline to
// distinguish "pre-marker" from "current" data.
#[derive(Clone)]
#[contracttype]
enum DataKey {
    /// Storage key for the contract admin addresses.
    Admin,

    /// Storage key for arbitrary system parameters.
    SystemParam(Symbol),

    /// Storage key for the fee configuration data.
    FeeConfig,

    /// Storage key for the anchor address associated with a specific asset.
    Anchor(Address),

    /// Instance-storage schema version (u32) written at `init`. Baseline for
    /// the first storage migration (issue #507).
    SchemaVersion,
    /// Instance — stored at `init` to gate initialization to the deployer
    /// and prevent front-running (issue #684).
    Deployer,
}

/// The schema version this build expects. `init` writes this value and
/// `migrate` advances any stored value below it.
const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The single interface version advertised by `supports_interface`.
///
/// `upgrade` probes the incoming Wasm with `supports_interface(SUPPORTED_INTERFACE_VERSION)`
/// before committing the swap. Any Wasm that returns `false` (or traps) is
/// rejected with `InvalidWasmInterface`. Increment this constant in a future
/// Wasm update when a breaking API change requires callers to distinguish
/// the new contract from this one (issue #48).
const SUPPORTED_INTERFACE_VERSION: u32 = 1;

// Discriminants below are pinned to `bettapay_common::error_codes` so that a
// numeric error code means the same thing in both contracts (issue #517).
// Shared concepts use the registry's constant value directly; codes with no
// settlement_contract equivalent are contract-specific and live in the
// `200..=299` range reserved for this contract. `governance_error_codes_match_registry`
// below fails the build if these literals ever drift from the registry.
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
    /// The contract is currently paused and the operation is not allowed.
    Paused = 5,
    /// The provided admin address is invalid (e.g., zero address or same as current admin).
    InvalidAdmin = 6,
    InvalidRecoveryAddress = 7,
    RecoveryNotPending = 8,
    RecoveryDelayActive = 9,
    /// The deployed WASM does not implement the required interface.
    InvalidWasmInterface = 13,
    /// The provided multisig threshold is invalid.
    InvalidThreshold = 14,
    /// The anchor for the specified asset was not found.
    AnchorMissing = 200,
    InvalidParamValue = 201,
    /// `pause` was called while the contract was already paused.
    AlreadyPaused = 15,
    /// `unpause` was called while the contract was already unpaused.
    AlreadyUnpaused = 16,
    /// The new admin set and threshold are identical to the current ones.
    SameAdmin = 204,
}

const _: () = {
    assert!(GovernanceError::AlreadyInitialized as u32 == error_codes::ALREADY_INITIALIZED);
    assert!(GovernanceError::NotInitialized as u32 == error_codes::NOT_INITIALIZED);
    assert!(GovernanceError::Unauthorized as u32 == error_codes::UNAUTHORIZED);
    assert!(GovernanceError::InvalidFeeBps as u32 == error_codes::INVALID_FEE_BPS);
    assert!(GovernanceError::Paused as u32 == error_codes::PAUSED);
    assert!(GovernanceError::InvalidAdmin as u32 == error_codes::INVALID_ADMIN);
    assert!(
        GovernanceError::InvalidRecoveryAddress as u32 == error_codes::INVALID_RECOVERY_ADDRESS
    );
    assert!(GovernanceError::RecoveryNotPending as u32 == error_codes::RECOVERY_NOT_PENDING);
    assert!(GovernanceError::RecoveryDelayActive as u32 == error_codes::RECOVERY_DELAY_ACTIVE);
    assert!(GovernanceError::InvalidWasmInterface as u32 == error_codes::INVALID_WASM_INTERFACE);
    assert!(GovernanceError::InvalidThreshold as u32 == error_codes::INVALID_THRESHOLD);
    assert!(GovernanceError::AnchorMissing as u32 >= error_codes::GOVERNANCE_RANGE_START);
    assert!(GovernanceError::InvalidParamValue as u32 >= error_codes::GOVERNANCE_RANGE_START);
    assert!(GovernanceError::AlreadyPaused as u32 == error_codes::ALREADY_PAUSED);
    assert!(GovernanceError::AlreadyUnpaused as u32 == error_codes::ALREADY_UNPAUSED);
    assert!(GovernanceError::SameAdmin as u32 >= error_codes::GOVERNANCE_RANGE_START);
};

#[contract]
pub struct GovernanceContract;

#[contractimpl]
impl GovernanceContract {
    pub fn supports_interface(_env: Env, version: u32) -> bool {
        version == SUPPORTED_INTERFACE_VERSION
    }

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
    pub fn init(env: Env, deployer: Address, admins: Vec<Address>, threshold: u32, recovery_address: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, GovernanceError::AlreadyInitialized);
        }
        // Gate initialization to the deployer to prevent front-running (issue #684).
        deployer.require_auth();
        validate_admins_and_threshold(&env, &admins, threshold);
        assert_not_zero(
            &env,
            &recovery_address,
            GovernanceError::InvalidRecoveryAddress,
        );
        for i in 0..threshold {
            admins.get(i).unwrap().require_auth();
        }
        env.storage().instance().set(&DataKey::Deployer, &deployer);
        env.storage().instance().set(&DataKey::Admin, &admins);
        env.storage()
            .instance()
            .set(&CommonDataKey::Threshold, &threshold);
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &recovery_address);
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
        let admin = admins.get(0).unwrap();
        env.events()
            .publish((Symbol::new(&env, events::INITIALIZED_EVENT),), admin);
    }

    pub fn is_initialized(env: Env) -> bool {
        // `is_initialized` is a cheap probe that should not bump the instance
        // TTL — going through `storage::read_admin` would do an extend_ttl on
        // every check and could panic if the contract has no instance entries.
        env.storage().instance().has(&DataKey::Admin)
    }

    pub fn get_admin(env: Env) -> Vec<Address> {
        read_admins(&env)
    }

    pub fn get_threshold(env: Env) -> u32 {
        read_threshold(&env)
    }

    pub fn get_recovery_address(env: Env) -> Address {
        read_recovery_address(&env)
    }

    pub fn update_recovery_address(env: Env, signers: Vec<Address>, new_recovery: Address) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();
        assert_not_zero(
            &env,
            &new_recovery,
            GovernanceError::InvalidRecoveryAddress,
        );
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &new_recovery);
        env.events().publish(
            (
                Symbol::new(&env, events::RECOVERY_ADDRESS_UPDATED_EVENT),
                new_recovery.clone(),
            ),
            admin,
        );
    }

    /// Upgrades the contract Wasm code to a new version.
    ///
    /// This function replaces only the contract's executable Wasm code;
    /// all persistent and instance storage entries remain intact. A
    /// separate storage-migration function should be written and called
    /// after the upgrade if the new code expects a different schema.
    ///
    /// Before swapping the executable the function deploys a probe instance of
    /// the new Wasm and calls `supports_interface(1)` on it.  If the function
    /// is missing or returns `false`, the upgrade panics with
    /// [`GovernanceError::InvalidWasmInterface`] and the running code is
    /// unchanged.
    ///
    /// ### Events
    /// - Emits `contract_upgraded` with topic
    ///   `(Symbol("contract_upgraded"), caller)` and data
    ///   `(new_wasm_hash)`.
    ///
    /// ### Panics
    /// - Panics with [`Unauthorized`](GovernanceError::Unauthorized) if the caller is not the current admin.
    /// - Panics with [`InvalidWasmInterface`](GovernanceError::InvalidWasmInterface) if the new Wasm does not support interface version 1.
    pub fn upgrade(env: Env, signers: Vec<Address>, new_wasm_hash: BytesN<32>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));

        // Deploy a probe instance of the new Wasm so we can call
        // `supports_interface` on it.  We use the wasm hash itself as the
        // salt so the probe address is deterministic and collision-free.
        let probe = env
            .deployer()
            .with_current_contract(new_wasm_hash.clone())
            .deploy(new_wasm_hash.clone());

        let version_args: Vec<u32> = soroban_sdk::vec![&env, 1u32];
        let supports: bool = match env.try_invoke_contract::<bool, GovernanceError>(
            &probe,
            &Symbol::new(&env, "supports_interface"),
            version_args.into_val(&env),
        ) {
            Ok(Ok(v)) => v,
            _ => panic_with_error!(&env, GovernanceError::InvalidWasmInterface),
        };
        if !supports {
            panic_with_error!(&env, GovernanceError::InvalidWasmInterface);
        }

        let event_wasm_hash = new_wasm_hash.clone();
        let caller = signers.get(0).unwrap();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        env.events().publish(
            (
                Symbol::new(&env, events::CONTRACT_UPGRADED_EVENT),
                event_wasm_hash,
            ),
            caller,
        );
    }

    pub fn initiate_recovery(env: Env, new_admin: Address) {
        let recovery_address = read_recovery_address(&env);
        recovery_address.require_auth();
        assert_not_zero(&env, &new_admin, GovernanceError::InvalidAdmin);

        let pending = PendingRecovery {
            new_admin: new_admin.clone(),
            execute_after: env.ledger().timestamp() + RECOVERY_DELAY_SECONDS,
            initiated_by: recovery_address.clone(),
        };
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
        events::emit_recovery_initiated(&env, &recovery_address, &new_admin, pending.execute_after);
    }

    pub fn cancel_recovery(env: Env, signers: Vec<Address>) {
        // Cancellation policy (issue #560): the pending recovery may be
        // cancelled by the address recorded as `initiated_by` (the recovery
        // address that started it) or by the current admin set meeting the
        // multisig threshold. The admin path is unchanged; the initiator
        // path lets an initiation be undone by the address that made it.
        // Any other caller is refused with `Unauthorized`.
        let pending = read_pending_recovery(&env);
        let cancelled_by_initiator =
            signers.len() == 1 && signers.get(0).unwrap() == pending.initiated_by;
        if cancelled_by_initiator {
            pending.initiated_by.require_auth();
        } else {
            verify_admin_auth(&env, &signers, read_threshold(&env));
        }
        let canceller = signers.get(0).unwrap();
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_cancelled(&env, &canceller);
    }

    pub fn execute_recovery(env: Env) {
        let pending = read_pending_recovery(&env);
        if env.ledger().timestamp() < pending.execute_after {
            panic_with_error!(&env, GovernanceError::RecoveryDelayActive);
        }

        // Issue #514: never let event-building read the possibly-corrupt admin
        // entry and abort recovery before it can repair the set. Resolve the
        // old admin to `Option` and fall back to the zero-address sentinel
        // when the entry is missing or has no primary admin, so recovery
        // always succeeds in replacing the set.
        let old_admin = read_optional_primary_admin(&env);

        let new_admins = soroban_sdk::vec![&env, pending.new_admin.clone()];
        env.storage().instance().set(&DataKey::Admin, &new_admins);
        env.storage()
            .instance()
            .set(&CommonDataKey::Threshold, &1u32);
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        events::emit_recovery_executed(
            &env,
            &AdminTransferred {
                old_admin,
                new_admin: pending.new_admin.clone(),
            },
        );
    }

    /// Transfers the admin set and multisig threshold to `new_admins` / `new_threshold`.
    ///
    /// # Errors
    ///
    /// Panics with `GovernanceError::InvalidAdmin` if `new_admins` is empty or
    /// contains the zero address or duplicate entries.
    /// Panics with `GovernanceError::SameAdmin` if `new_admins` and `new_threshold`
    /// are identical to the current admin set and threshold.
    pub fn transfer_admin(
        env: Env,
        signers: Vec<Address>,
        new_admins: Vec<Address>,
        new_threshold: u32,
    ) {
        let old_threshold = read_threshold(&env);
        verify_admin_auth(&env, &signers, old_threshold);
        validate_admins_and_threshold(&env, &new_admins, new_threshold);

        let old_admins = read_admins(&env);
        if old_admins == new_admins && old_threshold == new_threshold {
            panic_with_error!(&env, GovernanceError::SameAdmin);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admins);
        env.storage()
            .instance()
            .set(&CommonDataKey::Threshold, &new_threshold);
        events::emit_admin_transferred(
            &env,
            &AdminTransferred {
                old_admin: storage::primary_admin(&old_admins).unwrap(),
                new_admin: new_admins.get(0).unwrap(),
            },
        );
    }

    pub fn change_threshold(env: Env, signers: Vec<Address>, new_threshold: u32) {
        let admins = read_admins(&env);
        if new_threshold == 0 || new_threshold > admins.len() {
            panic_with_error!(&env, GovernanceError::InvalidThreshold);
        }

        let current_threshold = read_threshold(&env);
        verify_admin_auth(&env, &signers, current_threshold + 1);

        env.storage()
            .instance()
            .set(&CommonDataKey::Threshold, &new_threshold);
        env.events().publish(
            (Symbol::new(&env, events::THRESHOLD_CHANGED_EVENT),),
            (current_threshold, new_threshold),
        );
    }

    pub fn pause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if Self::is_paused(env.clone()) {
            panic_with_error!(&env, GovernanceError::AlreadyPaused);
        }
        let admin = signers.get(0).unwrap();
        storage::apply_pause(&env, &admin);
    }

    pub fn unpause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if !Self::is_paused(env.clone()) {
            panic_with_error!(&env, GovernanceError::AlreadyUnpaused);
        }
        let admin = signers.get(0).unwrap();
        storage::apply_unpause(&env, &admin);
    }

    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// Idempotent schema migration entry point.
    ///
    /// Issue #507: ships the schema-version marker and a migration entry point
    /// so the first real storage migration has a defined baseline. There is no
    /// existing storage-format difference to convert yet, so calling `migrate`
    /// simply confirms the `SchemaVersion` marker. It is admin-gated and
    /// idempotent: a contract already at `CURRENT_SCHEMA_VERSION` is a no-op.
    pub fn migrate(env: Env, signers: Vec<Address>) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        if read_schema_version(&env) < CURRENT_SCHEMA_VERSION {
            env.storage()
                .instance()
                .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
        }
        env.events().publish(
            (Symbol::new(&env, events::MIGRATED_EVENT),),
            (admin, CURRENT_SCHEMA_VERSION),
        );
    }

    pub fn update_system_param(env: Env, signers: Vec<Address>, key: Symbol, value: i128) {
        verify_admin_auth(&env, &signers, read_threshold(&env));

        if value < 0 {
            panic_with_error!(&env, GovernanceError::InvalidParamValue);
        }

        let admin = signers.get(0).unwrap();
        let storage_key = DataKey::SystemParam(key.clone());
        let previous_value: Option<i128> = env.storage().persistent().get(&storage_key);

        env.storage().persistent().set(&storage_key, &value);
        env.storage().persistent().extend_ttl(
            &storage_key,
            SYSTEM_PARAM_TTL_THRESHOLD,
            SYSTEM_PARAM_TTL_BUMP,
        );

        env.events().publish(
            (Symbol::new(&env, events::SYS_PARAM_UPDATED_EVENT), key),
            (admin, previous_value, value),
        );
    }

    pub fn get_system_param(env: Env, key: Symbol) -> Option<i128> {
        let storage_key = DataKey::SystemParam(key);
        if env.storage().persistent().has(&storage_key) {
            env.storage().persistent().extend_ttl(
                &storage_key,
                SYSTEM_PARAM_TTL_THRESHOLD,
                SYSTEM_PARAM_TTL_BUMP,
            );
        }
        env.storage().persistent().get(&storage_key)
    }

    /// Sets the global fee configuration.
    ///
    /// **Fee Ceiling Policy**: Governance is the trust root for cross-contract fee ceilings.
    /// While individual fees are bounded by `MAX_FEE_BPS` and their sum by `BPS_DENOMINATOR`,
    /// Governance is fully trusted to set safe rates within those technical boundaries.
    ///
    pub fn set_fee_config(env: Env, signers: Vec<Address>, config: FeeConfig) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));

        if config.platform_fee_bps < MIN_FEE_BPS
            || config.platform_fee_bps > MAX_FEE_BPS
            || config.network_fee_bps < MIN_FEE_BPS
            || config.network_fee_bps > MAX_FEE_BPS
        {
            panic_with_error!(&env, GovernanceError::InvalidFeeBps);
        }

        if config.platform_fee_bps + config.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, GovernanceError::InvalidFeeBps);
        }

        let admin = signers.get(0).unwrap();
        let key = DataKey::FeeConfig;
        env.storage().instance().set(&key, &config);
        env.events().publish(
            (Symbol::new(&env, events::FEE_CONFIG_UPDATED_EVENT),),
            (admin, config),
        );
    }

    pub fn get_fee_config(env: Env) -> Option<FeeConfig> {
        let key = DataKey::FeeConfig;
        env.storage().instance().get(&key)
    }

    pub fn upsert_anchor(env: Env, signers: Vec<Address>, asset: Address, anchor: Address) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        assert_not_zero(&env, &asset, GovernanceError::InvalidAdmin);
        assert_not_zero(&env, &anchor, GovernanceError::InvalidAdmin);
        if asset == anchor {
            panic_with_error!(&env, GovernanceError::InvalidAdmin);
        }
        let key = DataKey::Anchor(asset.clone());
        let old_anchor: Option<Address> = env.storage().persistent().get(&key);
        env.storage().persistent().set(&key, &anchor.clone());
        env.storage()
            .persistent()
            .extend_ttl(&key, ANCHOR_TTL_THRESHOLD, ANCHOR_TTL_BUMP);
        env.events().publish(
            (Symbol::new(&env, events::ANCHOR_UPSERTED_EVENT), asset),
            (old_anchor, anchor),
        );
    }

    pub fn remove_anchor(env: Env, signers: Vec<Address>, asset: Address) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let key = DataKey::Anchor(asset.clone());

        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, GovernanceError::AnchorMissing);
        }

        env.storage().persistent().remove(&key);
        env.events()
            .publish((Symbol::new(&env, events::ANCHOR_REMOVED_EVENT), asset), ());
    }

    pub fn get_anchor(env: Env, asset: Address) -> Option<Address> {
        let key = DataKey::Anchor(asset.clone());
        let result = env.storage().persistent().get(&key);
        if result.is_some() {
            // `extend_ttl` only writes when the current TTL is below
            // `threshold`, so this has the same externally observable
            // behavior as a manual get_ttl-then-extend check, without
            // depending on `get_ttl`, which is test-only in production code.
            env.storage()
                .persistent()
                .extend_ttl(&key, ANCHOR_TTL_THRESHOLD, ANCHOR_TTL_BUMP);
        }
        result
    }
}

fn read_admins(env: &Env) -> Vec<Address> {
    // Admin reads use the 50k/100k instance policy (issue #515), matching
    // settlement's `read_admins` and ADR 003's "Admin & Governance" guidance,
    // rather than the standard 14/30-day `bump_instance_ttl` policy.
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, GovernanceError::NotInitialized))
}

fn read_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&CommonDataKey::Threshold)
        .unwrap_or_else(|| panic_with_error!(env, GovernanceError::NotInitialized))
}

fn validate_admins_and_threshold(env: &Env, admins: &Vec<Address>, threshold: u32) {
    if threshold == 0 || threshold > admins.len() {
        panic_with_error!(env, GovernanceError::InvalidThreshold);
    }
    if admins.is_empty() {
        panic_with_error!(env, GovernanceError::InvalidAdmin);
    }
    for i in 0..admins.len() {
        let admin = admins.get(i).unwrap();
        assert_not_zero(env, &admin, GovernanceError::InvalidAdmin);
        for j in (i + 1)..admins.len() {
            if admin == admins.get(j).unwrap() {
                panic_with_error!(env, GovernanceError::InvalidAdmin);
            }
        }
    }
}

fn verify_admin_auth(env: &Env, signers: &Vec<Address>, required_count: u32) {
    let admins = read_admins(env);
    if signers.len() < required_count {
        panic_with_error!(env, GovernanceError::Unauthorized);
    }
    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();
        let mut is_admin = false;
        for j in 0..admins.len() {
            if signer == admins.get(j).unwrap() {
                is_admin = true;
                break;
            }
        }
        if !is_admin {
            panic_with_error!(env, GovernanceError::Unauthorized);
        }
        for j in (i + 1)..signers.len() {
            if signer == signers.get(j).unwrap() {
                panic_with_error!(env, GovernanceError::Unauthorized);
            }
        }
        signer.require_auth();
    }
}

fn read_recovery_address(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&CommonDataKey::RecoveryAddress)
        .unwrap_or_else(|| panic_with_error!(env, GovernanceError::NotInitialized))
}

fn read_pending_recovery(env: &Env) -> PendingRecovery {
    // Decode by hand so a pending recovery written before `initiated_by`
    // existed (pre-issue #560) is refused with `RecoveryNotPending` instead
    // of surfacing a host-level conversion panic. Refusing is deliberate:
    // an old-format record must never be treated as a valid pending
    // recovery (default-deny, never default-allow).
    let val = env
        .storage()
        .instance()
        .get::<_, Val>(&CommonDataKey::PendingRecovery)
        .unwrap_or_else(|| panic_with_error!(env, GovernanceError::RecoveryNotPending));
    PendingRecovery::try_from_val(env, &val)
        .unwrap_or_else(|_| panic_with_error!(env, GovernanceError::RecoveryNotPending))
}

/// Returns the instance-storage schema version, defaulting to the current
/// version when the marker is absent. Per DEVELOPMENT.md, an entry written
/// before the marker existed is treated as version 1 (issue #507).
fn read_schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SchemaVersion)
        .unwrap_or(CURRENT_SCHEMA_VERSION)
}

/// Returns the primary admin address, or the zero-address sentinel when the
/// admin entry is missing or has no primary. Used only by `execute_recovery`,
/// which must be able to repair a corrupt admin set (issue #514).
fn read_optional_primary_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Vec<Address>>(&DataKey::Admin)
        .and_then(|admins| storage::primary_admin(&admins))
        .unwrap_or_else(|| {
            Address::from_string(&soroban_sdk::String::from_str(
                env,
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            ))
        })
}

fn assert_not_zero(env: &Env, address: &Address, error: GovernanceError) {
    if storage::is_zero_address(env, address) {
        panic_with_error!(env, error);
    }
}

/// Ensures the governance contract is not currently paused.
///
/// This helper is called before mutating operations that should be disabled
/// while the contract is paused. It enforces the pause state centrally so
/// callers do not need to duplicate the check themselves.
///
/// # Panics
///
/// Panics with `GovernanceError::Paused` if the contract is currently paused.
fn assert_not_paused(env: &Env) {
    if storage::is_paused(env) {
        panic_with_error!(env, GovernanceError::Paused);
    }
}

/// Shared test setup used across the main test module and the anchor_*
/// sub-modules, so a change to `init`'s signature only needs updating here.
#[cfg(test)]
pub(crate) fn setup() -> (Env, GovernanceContractClient<'static>, Vec<Address>) {
    use soroban_sdk::testutils::Address as _;
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    client.init(&deployer, &admins, &1, &recovery_address);
    (env, client, admins)
}

#[cfg(test)]
mod anchor_auth_tests;

#[cfg(test)]
mod anchor_event_tests;

#[cfg(test)]
mod anchor_removal_tests;

#[cfg(test)]
mod anchor_no_event_error_tests;

#[cfg(test)]
mod real_auth_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::testutils::storage::Persistent;
    use soroban_sdk::testutils::{Address as _, Events};
    use soroban_sdk::{vec, Bytes, FromVal, String};

    fn setup() -> (
        Env,
        GovernanceContractClient<'static>,
        Vec<Address>,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);
        let admins = vec![&env, admin1.clone(), admin2.clone()];
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &2, &recovery_address);
        (env, client, admins, recovery_address)
    }

    #[allow(dead_code)]
    fn upload_test_wasm(env: &Env) -> BytesN<32> {
        let wasm = Bytes::from_slice(env, &[]);
        env.deployer().upload_contract_wasm(wasm)
    }

    // -----------------------------------------------------------------------
    // supports_interface — issue #48
    // -----------------------------------------------------------------------
    //
    // These tests pin the exact version semantics so the function can never
    // silently degrade into an always-true stub.

    /// Version 1 is the current advertised interface; `supports_interface(1)`
    /// must return `true`.
    #[test]
    fn supports_interface_returns_true_for_current_version() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(
            client.supports_interface(&SUPPORTED_INTERFACE_VERSION),
            "supports_interface must return true for the current interface version ({})",
            SUPPORTED_INTERFACE_VERSION,
        );
    }

    /// Version 0 has never been a valid interface version; it must be rejected.
    #[test]
    fn supports_interface_returns_false_for_version_zero() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(
            !client.supports_interface(&0u32),
            "supports_interface must return false for version 0",
        );
    }

    /// Version 2 is a hypothetical future version not yet implemented; it
    /// must be rejected so callers can distinguish old Wasm from new.
    #[test]
    fn supports_interface_returns_false_for_unknown_future_version() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(
            !client.supports_interface(&(SUPPORTED_INTERFACE_VERSION + 1)),
            "supports_interface must return false for a future version not yet implemented",
        );
    }

    /// A large sentinel value must also be rejected (issue #48: must not be
    /// an always-true stub).
    #[test]
    fn supports_interface_returns_false_for_large_sentinel() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(
            !client.supports_interface(&u32::MAX),
            "supports_interface must return false for a large out-of-range version",
        );
    }

    #[test]
    fn executes_contract_wasm_upgrade_successfully() {
        // After adding the interface check, empty wasm (no exports) is correctly
        // rejected rather than silently accepted.  This test verifies rejection
        // and confirms the contract remains operational afterwards.
        //
        // The positive case (conforming wasm accepted) requires uploading the
        // governance wasm bytes; the `upgrade_rejects_wasm_missing_supports_interface`
        // and `upgrade_rejects_non_admin_before_interface_check` tests cover the
        // negative guard paths.
        let (env, client, admins, _recovery) = setup();
        let bad_hash = upload_test_wasm(&env); // empty wasm — no supports_interface

        let result = client.try_upgrade(&admins, &bad_hash);
        assert!(
            result.is_err(),
            "upgrade with non-conforming wasm must be rejected"
        );

        // Contract is intact after the failed upgrade.
        let live_client = GovernanceContractClient::new(&env, &client.address);
        assert_eq!(live_client.get_admin(), admins);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn governance_rejects_double_initialization() {
        let (_env, client, admins, recovery) = setup();
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &2, &recovery);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #14)")]
    fn governance_rejects_zero_threshold_init() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let recovery = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &vec![env, admin], &0, &recovery);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn governance_rejects_zero_address_admin_transfer() {
        let (env, client, admins, _recovery) = setup();
        let zero_address = Address::from_string(&String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));

        client.transfer_admin(&admins, &vec![&env, zero_address], &1);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #204)")]
    fn rejects_same_admin_transfer() {
        let (_env, client, admins, _recovery) = setup();
        let threshold = client.get_threshold();
        client.transfer_admin(&admins, &admins, &threshold);
    }

    #[test]
    fn updates_system_parameters() {
        let (env, client, admins, _recovery) = setup();
        let key = Symbol::new(&env, "max_settle");
        let before = env.events().all().len();
        client.update_system_param(&admins, &key, &1440);
        assert_eq!(client.get_system_param(&key), Some(1440));
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn system_parameter_key_uniqueness_overwrites_value() {
        let (env, client, admins, _recovery) = setup();
        let key = Symbol::new(&env, "test_param");

        client.update_system_param(&admins, &key, &100);
        assert_eq!(client.get_system_param(&key), Some(100));

        client.update_system_param(&admins, &key, &200);
        assert_eq!(client.get_system_param(&key), Some(200));

        client.update_system_param(&admins, &key, &300);
        assert_eq!(client.get_system_param(&key), Some(300));
    }

    #[test]
    fn sets_fee_config() {
        let (env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };
        let before = env.events().all().len();
        client.set_fee_config(&admins, &cfg);
        let got = client.get_fee_config().expect("expected config");
        assert_eq!(got.platform_fee_bps, 120);
        assert_eq!(got.network_fee_bps, 35);
        assert!(env.events().all().len() > before);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn set_fee_config_blocked_when_paused() {
        let (_env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };

        client.pause(&admins);
        client.set_fee_config(&admins, &cfg);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn set_fee_config_checks_paused_before_auth_and_validation() {
        let (env, client, admins, _recovery) = setup();
        let non_admin = Address::generate(&env);
        let non_admin_signer = vec![&env, non_admin];
        let invalid_cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 4,
        };

        client.pause(&admins);
        client.set_fee_config(&non_admin_signer, &invalid_cfg);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn set_fee_config_rejects_signatures_below_threshold_when_not_paused() {
        let (env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };
        let single_signer = vec![&env, admins.get(0).unwrap()];

        client.set_fee_config(&single_signer, &cfg);
    }

    #[test]
    fn fee_config_event_emitted_with_correct_fields() {
        let (env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };

        client.set_fee_config(&admins, &cfg);

        let events = env.events().all();
        let event = events.last().unwrap();

        let (_contract_id, topics, data) = event;

        assert_eq!(topics.len(), 1);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, events::FEE_CONFIG_UPDATED_EVENT)
        );

        let (event_admin, event_cfg): (Address, FeeConfig) = FromVal::from_val(&env, &data);
        assert_eq!(event_admin, admins.get(0).unwrap());
        assert_eq!(event_cfg.platform_fee_bps, 120);
        assert_eq!(event_cfg.network_fee_bps, 35);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn set_fee_config_rejects_fees_exceeding_ceiling() {
        let (_env, client, admins, _recovery) = setup();

        // Sum exceeds BPS_DENOMINATOR
        let cfg = FeeConfig {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_001,
        };

        client.set_fee_config(&admins, &cfg);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn set_fee_config_rejects_individual_fee_exceeding_max() {
        let (_env, client, admins, _recovery) = setup();

        // Individual fee exceeds MAX_FEE_BPS (governance trust root)
        let cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 0,
        };

        client.set_fee_config(&admins, &cfg);
    }

    #[test]
    fn upserts_and_removes_anchor() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        let before_upsert = env.events().all().len();
        client.upsert_anchor(&admins, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        assert!(env.events().all().len() > before_upsert);

        let before_remove = env.events().all().len();
        client.remove_anchor(&admins, &asset);
        assert_eq!(client.get_anchor(&asset), None);
        assert!(env.events().all().len() > before_remove);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn rejects_upsert_anchor_with_zero_address_asset() {
        let (env, client, admins, _recovery) = setup();
        let zero_address = Address::from_string(&String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        let anchor = Address::generate(&env);

        client.upsert_anchor(&admins, &zero_address, &anchor);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn rejects_upsert_anchor_with_zero_address_anchor() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);
        let zero_address = Address::from_string(&String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));

        client.upsert_anchor(&admins, &asset, &zero_address);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #6)")]
    fn rejects_upsert_anchor_where_asset_equals_anchor() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);

        client.upsert_anchor(&admins, &asset, &asset);
    }

    #[test]
    fn get_anchor_extends_anchor_ttl() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);

        client.upsert_anchor(&admins, &asset, &anchor);
        assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
        assert_eq!(client.get_anchor(&asset), Some(anchor));
    }

    #[test]
    fn upsert_anchor_uses_same_ttl_policy_as_read() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);
        let seq = env.ledger().sequence();

        client.upsert_anchor(&admins, &asset, &anchor);

        let live_until_after_write = env.as_contract(&client.address, || {
            env.storage()
                .persistent()
                .get_ttl(&DataKey::Anchor(asset.clone()))
        });

        // The write path bumps with ANCHOR_TTL_BUMP (via extend_ttl with the
        // ANCHOR_TTL threshold/bump pair), so the remaining TTL (live-until
        // minus the current ledger) must clear the anchor bump policy.
        assert!(
            live_until_after_write - seq >= ANCHOR_TTL_BUMP,
            "write path remaining TTL {} < ANCHOR_TTL_BUMP ({ANCHOR_TTL_BUMP})",
            live_until_after_write - seq
        );

        assert_eq!(client.get_anchor(&asset), Some(anchor));

        let live_until_after_read = env.as_contract(&client.address, || {
            env.storage()
                .persistent()
                .get_ttl(&DataKey::Anchor(asset.clone()))
        });

        // The read path uses the same ANCHOR_TTL_THRESHOLD / ANCHOR_TTL_BUMP,
        // so it never shrinks the policy established by the write.
        assert!(
            live_until_after_read >= live_until_after_write,
            "read path live-until ({live_until_after_read}) should not be below write path ({live_until_after_write})"
        );
    }

    #[test]
    fn anchor_upsert_overwrites_existing_anchor() {
        let (env, client, admins, _recovery) = setup();
        let asset = Address::generate(&env);
        let anchor_one = Address::generate(&env);
        let anchor_two = Address::generate(&env);

        client.upsert_anchor(&admins, &asset, &anchor_one);
        assert_eq!(client.get_anchor(&asset), Some(anchor_one));

        client.upsert_anchor(&admins, &asset, &anchor_two);
        assert_eq!(client.get_anchor(&asset), Some(anchor_two));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn rejects_fee_bps_above_max() {
        let (_env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 5_001,
            network_fee_bps: 100,
        };
        client.set_fee_config(&admins, &cfg);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #4)")]
    fn rejects_fee_bps_below_min() {
        let (_env, client, admins, _recovery) = setup();
        let cfg = FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 4,
        };
        client.set_fee_config(&admins, &cfg);
    }

    #[test]
    fn accepts_fee_bps_at_boundaries() {
        let (_env, client, admins, _recovery) = setup();
        client.set_fee_config(
            &admins,
            &FeeConfig {
                platform_fee_bps: 5,
                network_fee_bps: 5,
            },
        );
        client.set_fee_config(
            &admins,
            &FeeConfig {
                platform_fee_bps: 5_000,
                network_fee_bps: 5_000,
            },
        );
    }

    proptest! {
        #[test]
        fn valid_fee_configs_are_accepted(
            (platform_fee_bps, network_fee_bps) in
                (5u32..=5_000, 5u32..=5_000)
                    .prop_filter("fee sum must fit the denominator", |(platform, network)| {
                        *platform + *network <= BPS_DENOMINATOR
                    }),
        ) {
            let env = Env::default();
            env.mock_all_auths();
            let admin = Address::generate(&env);
            let recovery = Address::generate(&env);
            let admins = vec![&env, admin];
            let contract_id = env.register_contract(None, GovernanceContract);
            let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
            client.init(&deployer, &admins, &1, &recovery);

            let config = FeeConfig {
                platform_fee_bps,
                network_fee_bps,
            };
            client.set_fee_config(&admins, &config);
            let stored = client.get_fee_config().unwrap();
            prop_assert_eq!(stored.platform_fee_bps, platform_fee_bps);
            prop_assert_eq!(stored.network_fee_bps, network_fee_bps);
        }

        #[test]
        fn fee_configs_with_an_out_of_range_leg_are_rejected(
            platform_fee_bps in 0u32..=5_000,
            network_fee_bps in 0u32..=5_000,
            invalid_platform in any::<bool>(),
        ) {
            let invalid_value = if invalid_platform {
                5_001
            } else {
                4
            };
            let config = if invalid_platform {
                FeeConfig {
                    platform_fee_bps: invalid_value,
                    network_fee_bps,
                }
            } else {
                FeeConfig {
                    platform_fee_bps,
                    network_fee_bps: invalid_value,
                }
            };
            let env = Env::default();
            env.mock_all_auths();
            let admin = Address::generate(&env);
            let recovery = Address::generate(&env);
            let admins = vec![&env, admin];
            let contract_id = env.register_contract(None, GovernanceContract);
            let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
            client.init(&deployer, &admins, &1, &recovery);

            prop_assert!(client.try_set_fee_config(&admins, &config).is_err());
        }

        #[test]
        fn threshold_validation_accepts_exact_admin_count_and_rejects_out_of_range(
            admin_count in 1u32..=5,
            threshold in 0u32..=6,
        ) {
            let env = Env::default();
            env.mock_all_auths();
            let mut admins = Vec::new(&env);
            for _ in 0..admin_count {
                admins.push_back(Address::generate(&env));
            }
            let recovery = Address::generate(&env);
            let contract_id = env.register_contract(None, GovernanceContract);
            let client = GovernanceContractClient::new(&env, &contract_id);

            let result = client.try_init(&admins, &threshold, &recovery);
            if threshold == 0 || threshold > admin_count {
                prop_assert!(result.is_err());
            } else {
                prop_assert!(result.is_ok());
                prop_assert_eq!(client.get_threshold(), threshold);
            }
        }
    }

    #[test]
    #[should_panic]
    fn rejects_removing_unknown_anchor() {
        let (env, client, admins, _recovery) = setup();
        let missing_asset = Address::generate(&env);
        client.remove_anchor(&admins, &missing_asset);
    }

    #[test]
    fn checks_if_initialized() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        assert!(!client.is_initialized());
    let deployer = Address::generate(&env);
        client.init(&deployer, &vec![env, admin.clone()], &1, &recovery_address);
        assert!(client.is_initialized());
    }

    #[test]
    fn emits_event_on_initialization() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);

        let admins = vec![&env, admin.clone()];
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery_address);
        assert!(client.is_initialized());
        assert_eq!(client.get_admin(), admins);
        assert_eq!(client.get_threshold(), 1);

        let events = env.events().all();
        assert_eq!(events.len(), 1, "exactly one event emitted on init");

        let (_contract_id, topics, data) = events.get(0).unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, events::INITIALIZED_EVENT)
        );
        assert_eq!(Address::from_val(&env, &data), admin);
    }

    // `Symbol`'s 32-character limit is enforced by the Stellar protocol
    // itself (SCSYMBOL_LIMIT) at construction time, not merely by an SDK
    // convenience check. `Symbol::new` below panics with
    // `Error(Value, InvalidInput)` before `update_system_param` is ever
    // invoked, so there is no public API path to construct an in-memory
    // `Symbol` over 32 characters. The test documents this protocol-level
    // invariant and ensures the contract does not assert a code path that
    // cannot be reached through it.
    #[test]
    #[should_panic(expected = "Error(Value, InvalidInput)")]
    fn rejects_oversized_symbol_key() {
        let (env, client, admins, _recovery) = setup();
        let oversized = "this_is_a_very_long_system_parameter_key";
        let key = Symbol::new(&env, oversized);
        client.update_system_param(&admins, &key, &123);
    }

    #[test]
    fn accepts_valid_symbol_key() {
        let (env, client, admins, _recovery) = setup();
        let key = Symbol::new(&env, "valid_key_32_chars_or_less");
        client.update_system_param(&admins, &key, &123);
        assert_eq!(client.get_system_param(&key), Some(123));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn rejects_operation_when_signatures_below_threshold() {
        let (env, client, admins, _recovery) = setup();
        let key = Symbol::new(&env, "key");
        // threshold is 2, but only 1 signer provided
        let single_signer = vec![&env, admins.get(0).unwrap()];
        client.update_system_param(&single_signer, &key, &100);
    }

    #[test]
    fn changes_threshold_with_threshold_plus_one_signatures() {
        let env = Env::default();
        env.mock_all_auths();

        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let a3 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2.clone(), a3.clone()];
        let recovery = Address::generate(&env);

        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery);

        assert_eq!(client.get_threshold(), 1);

        // Threshold is 1, so change_threshold requires 1 + 1 = 2 signatures.
        let signers = vec![&env, a1.clone(), a2.clone()];
        client.change_threshold(&signers, &2);
        assert_eq!(client.get_threshold(), 2);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn change_threshold_fails_with_insufficient_signatures() {
        let env = Env::default();
        env.mock_all_auths();

        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2.clone()];
        let recovery = Address::generate(&env);

        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery);

        // Current threshold is 1, needs 2 signatures for change_threshold, but only 1 provided.
        let single_signer = vec![&env, a1.clone()];
        client.change_threshold(&single_signer, &2);
    }

    // Issue #565: setting a threshold above the admin count must surface
    // `InvalidThreshold` (#14), not `Unauthorized` (#3) from the auth gate.
    #[test]
    #[should_panic(expected = "Error(Contract, #14)")]
    fn change_threshold_above_admin_count_rejects_with_invalid_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2.clone()];
        let recovery = Address::generate(&env);

        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery);

        // Threshold 3 > admins.len() 2 — must fail with InvalidThreshold, not auth.
        client.change_threshold(&admins, &3);
    }

    // Issue #565: threshold == 0 must also be rejected before the auth gate.
    #[test]
    #[should_panic(expected = "Error(Contract, #14)")]
    fn change_threshold_zero_rejects_with_invalid_threshold() {
        let env = Env::default();
        env.mock_all_auths();

        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2.clone()];
        let recovery = Address::generate(&env);

        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &2, &recovery);

        client.change_threshold(&admins, &0);
    }

    #[test]
    fn transfers_admin_successfully() {
        let (env, client, admins, _recovery) = setup();
        let new_a1 = Address::generate(&env);
        let new_admins = vec![&env, new_a1.clone()];
        client.transfer_admin(&admins, &new_admins, &1);
        assert_eq!(client.get_admin(), new_admins);
        assert_eq!(client.get_threshold(), 1);
    }

    #[test]
    fn pause_then_unpause_round_trip_succeeds() {
        let (env, client, admins, _recovery) = setup();

        assert!(!client.is_paused());

        let before_pause = env.events().all().len();
        client.pause(&admins);
        assert!(client.is_paused());
        assert!(env.events().all().len() > before_pause);

        let before_unpause = env.events().all().len();
        client.unpause(&admins);
        assert!(!client.is_paused());
        assert!(env.events().all().len() > before_unpause);
    }

    /// Pins `pause`/`unpause` to `bettapay_common::events`' canonical topic
    /// constants rather than a locally inlined string, so this test fails if
    /// either entry point stops routing through the shared emit helper.
    #[test]
    fn pause_and_unpause_emit_canonical_shared_topics() {
        let (env, client, admins, _recovery) = setup();

        client.pause(&admins);
        let (_, pause_topics, _) = env.events().all().last().unwrap();
        assert_eq!(
            Symbol::from_val(&env, &pause_topics.get(0).unwrap()),
            Symbol::new(&env, bettapay_common::events::PAUSED_EVENT)
        );

        client.unpause(&admins);
        let (_, unpause_topics, _) = env.events().all().last().unwrap();
        assert_eq!(
            Symbol::from_val(&env, &unpause_topics.get(0).unwrap()),
            Symbol::new(&env, bettapay_common::events::UNPAUSED_EVENT)
        );
    }

    // -----------------------------------------------------------------------
    // InvalidWasmInterface: upgrade flow enforces supports_interface(1)
    // -----------------------------------------------------------------------

    /// Uploading an empty Wasm (which has no `supports_interface` export)
    /// must be rejected with `InvalidWasmInterface` (code 13).
    #[test]
    #[should_panic(expected = "Error(Contract, #13)")]
    fn upgrade_rejects_wasm_missing_supports_interface() {
        let (env, client, admins, _recovery) = setup();
        // Empty wasm has no exports — the probe call will fail, raising the typed error.
        let bad_hash = env
            .deployer()
            .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
        client.upgrade(&admins, &bad_hash);
    }

    /// Upgrading with a non-admin caller must still be rejected with
    /// `Unauthorized` (code 3), showing auth is checked before interface probing.
    #[test]
    #[should_panic(expected = "Error(Contract, #3)")]
    fn upgrade_rejects_non_admin_before_interface_check() {
        let (env, client, _admins, _recovery) = setup();
        let non_admin = Address::generate(&env);
        let bad_hash = env
            .deployer()
            .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
        client.upgrade(&soroban_sdk::vec![&env, non_admin], &bad_hash);
    }

    // -----------------------------------------------------------------------
    // Issue #507: schema-version marker + migrate skeleton
    // -----------------------------------------------------------------------

    #[test]
    fn init_writes_schema_version_marker_and_migrate_is_idempotent() {
        let (env, client, admins, _recovery) = setup();

        let version = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get::<_, u32>(&DataKey::SchemaVersion)
        });
        assert_eq!(version, Some(CURRENT_SCHEMA_VERSION));

        client.migrate(&admins);
        client.migrate(&admins);

        let version_after = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get::<_, u32>(&DataKey::SchemaVersion)
        });
        assert_eq!(version_after, Some(CURRENT_SCHEMA_VERSION));

        let (_, topics, _) = env.events().all().last().unwrap();
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, events::MIGRATED_EVENT)
        );
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #5)")]
    fn migrate_is_blocked_while_paused() {
        let (_env, client, admins, _recovery) = setup();
        client.pause(&admins);
        client.migrate(&admins);
    }

    // -----------------------------------------------------------------------
    // Issue #514: execute_recovery repairs a corrupt/empty admin set
    // -----------------------------------------------------------------------

    #[test]
    fn execute_recovery_repairs_an_empty_corrupt_admin_set() {
        use soroban_sdk::testutils::Ledger;
        let (env, client, _admins, _recovery_address) = setup();
        let recovered = Address::generate(&env);

        client.initiate_recovery(&recovered);
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + RECOVERY_DELAY_SECONDS + 1);

        // Corrupt the admin entry: overwrite with an empty admin set so there
        // is no primary admin to read when building the recovery event.
        env.as_contract(&client.address, || {
            let empty: Vec<Address> = Vec::new(&env);
            env.storage().instance().set(&DataKey::Admin, &empty);
        });

        client.execute_recovery();

        assert_eq!(client.get_admin(), vec![&env, recovered.clone()]);
        assert_eq!(client.get_threshold(), 1);
    }

    // -----------------------------------------------------------------------
    // Issue #515: governance admin reads use the 50k/100k instance policy
    // -----------------------------------------------------------------------

    #[test]
    fn get_admin_uses_50k_100k_instance_ttl_policy() {
        use soroban_sdk::testutils::storage::Instance;
        let (env, client, _admins, _recovery) = setup();

        client.get_admin();

        let ttl = env.as_contract(&client.address, || env.storage().instance().get_ttl());
        assert!(
            ttl >= READ_INSTANCE_TTL_BUMP,
            "expected get_admin to bump instance TTL to at least {READ_INSTANCE_TTL_BUMP}, got {ttl}"
        );
    }

    // -----------------------------------------------------------------------
    // Issue #516: reconciled pause matrix
    // -----------------------------------------------------------------------

    #[test]
    fn pause_blocks_fee_and_anchor_writes() {
        let (env, client, admins, _recovery) = setup();
        client.pause(&admins);
        assert!(client.is_paused());

        let asset = Address::generate(&env);
        let anchor = Address::generate(&env);
        let cfg = FeeConfig {
            platform_fee_bps: 120,
            network_fee_bps: 35,
        };

        assert!(
            client.try_set_fee_config(&admins, &cfg).is_err(),
            "set_fee_config must be blocked while paused"
        );
        assert!(
            client.try_upsert_anchor(&admins, &asset, &anchor).is_err(),
            "upsert_anchor must be blocked while paused"
        );
        assert!(
            client.try_remove_anchor(&admins, &asset).is_err(),
            "remove_anchor must be blocked while paused"
        );
    }

    #[test]
    fn pause_allows_admin_transfer_threshold_and_recovery() {
        use soroban_sdk::testutils::Ledger;
        let env = Env::default();
        env.mock_all_auths();
        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);
        let admins = vec![&env, a1.clone(), a2.clone()];
        let recovery_address = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery_address);

        client.pause(&admins);
        assert!(client.is_paused());

        // change_threshold (threshold 1 -> needs threshold + 1 = 2 signers)
        client.change_threshold(&admins, &2);
        assert_eq!(client.get_threshold(), 2);

        // transfer_admin (threshold 2 -> needs 2 signers)
        let new_a = Address::generate(&env);
        client.transfer_admin(&admins, &vec![&env, new_a.clone()], &1);
        assert_eq!(client.get_admin(), vec![&env, new_a.clone()]);

        // recovery flow
        let recovered = Address::generate(&env);
        client.initiate_recovery(&recovered);
        env.ledger()
            .set_timestamp(env.ledger().timestamp() + RECOVERY_DELAY_SECONDS + 1);
        client.execute_recovery();
        assert_eq!(client.get_admin(), vec![&env, recovered.clone()]);
        assert_eq!(client.get_threshold(), 1);
    }

    // -----------------------------------------------------------------------
    // Recovery initiator (issue #560)
    // -----------------------------------------------------------------------

    // The pending recovery must record the address that initiated it, so
    // `cancel_recovery` can validate the cancellation against the initiator
    // and off-chain consumers can audit who started the recovery.
    #[test]
    fn pending_recovery_records_initiating_address() {
        let (env, client, _admins, recovery_address) = setup();
        let new_admin = Address::generate(&env);

        client.initiate_recovery(&new_admin);

        let pending: PendingRecovery = env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get(&CommonDataKey::PendingRecovery)
                .unwrap()
        });
        assert_eq!(pending.initiated_by, recovery_address);
        assert_eq!(pending.new_admin, new_admin);
    }

    // The address that initiated a recovery may cancel it directly. The
    // admin gate is unchanged, so this is an additional canceller, not a
    // replacement.
    #[test]
    fn cancel_recovery_by_initiator_succeeds() {
        let (env, client, _admins, recovery_address) = setup();
        let new_admin = Address::generate(&env);
        client.initiate_recovery(&new_admin);

        client.cancel_recovery(&vec![&env, recovery_address]);

        assert!(
            !env.as_contract(&client.address, || env
                .storage()
                .instance()
                .has(&CommonDataKey::PendingRecovery)),
            "a cancelled recovery must be removed from storage"
        );
    }

    // A caller that is neither an admin nor the recorded initiator is
    // refused, and the pending recovery survives the refused attempt.
    #[test]
    fn cancel_recovery_by_non_initiator_non_admin_is_refused() {
        let (env, client, _admins, _recovery_address) = setup();
        let new_admin = Address::generate(&env);
        client.initiate_recovery(&new_admin);

        let stranger = Address::generate(&env);
        let result = client.try_cancel_recovery(&vec![&env, stranger]);
        assert_eq!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                GovernanceError::Unauthorized as u32
            )))
        );
        assert!(
            env.as_contract(&client.address, || env
                .storage()
                .instance()
                .has(&CommonDataKey::PendingRecovery)),
            "a refused cancellation must leave the pending recovery in place"
        );
    }

    // The pre-existing admin path is preserved: the admin set can still
    // cancel a recovery it did not initiate.
    #[test]
    fn cancel_recovery_by_admin_still_succeeds() {
        let (env, client, admins, _recovery_address) = setup();
        let new_admin = Address::generate(&env);
        client.initiate_recovery(&new_admin);

        client.cancel_recovery(&admins);

        assert!(
            !env.as_contract(&client.address, || env
                .storage()
                .instance()
                .has(&CommonDataKey::PendingRecovery)),
            "the admin path must still be able to cancel a pending recovery"
        );
    }
}
