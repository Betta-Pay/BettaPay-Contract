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
//! **Shared bounds (issue #987):** `MIN_FEE_BPS` and `MAX_FEE_BPS` are
//! imported from `bettapay_common::constants` and are the single source for
//! both this contract's `FeeConfig` and the settlement contract's
//! `SettlementRule`. Neither contract declares its own copy, so governance
//! cannot accept a config that settlement would reject on the per-leg bounds.
//! The settlement contract's `fee_bounds_parity_tests` pin this at each
//! boundary value.
//!
//! Violating any constraint panics with [`GovernanceError::InvalidFeeBps`].
//! The current config is readable via [`GovernanceContract::get_fee_config`].
//! The entry emits a `fee_config_updated` event on every successful write.
//!
//! ### Anchor Registry
//! Each supported asset can nominate a trusted off-chain anchor address via
//! [`GovernanceContract::upsert_anchor`]. The anchor address is stored keyed by
//! the asset [`Address`] in persistent storage.
//! [`GovernanceContract::remove_anchor`] deletes the entry; attempting to
//! remove an asset that has no registered anchor panics with
//! [`GovernanceError::AnchorMissing`]. Both operations emit events
//! (`anchor_upserted` / `anchor_removed`) for off-chain indexers.
//! TTL of the anchor entry is extended on every read via
//! [`GovernanceContract::get_anchor`].
//!
//! ### System Parameters
//! [`GovernanceContract::update_system_param`] stores an arbitrary `i128`
//! value under a caller-supplied [`Symbol`] key. This
//! gives the admin a flexible mechanism to propagate numeric knobs (e.g.,
//! maximum settlement delay, minimum collateral ratio) to other contracts
//! without upgrading the governance contract itself. Parameters are read via
//! [`GovernanceContract::get_system_param`], which also refreshes the
//! persistent-entry TTL.
//!
//! One key is special-cased: `bypass_gov_fees` (issue #743) is the settlement
//! fee circuit-breaker and only accepts `0` or `1`. Setting `1` makes
//! settlement skip this contract's `get_fee_config` and fall through to its
//! bootstrap default, decoupling payments from a broken fee config without a
//! contract upgrade. Any other value is rejected with
//! [`GovernanceError::InvalidParamValue`].
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
//! | 18 | `SameAdmin` | Transfer target is identical to the current admin set and threshold |
//! | 200 | `AnchorMissing` | Tried to remove an unregistered anchor |
//! | 201 | `InvalidParamValue` | Supplied system parameter value is invalid or out of bounds |
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
        BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, PAUSE_REASON_NONE, RECOVERY_DELAY_SECONDS,
        TTL_BUMP_LEDGERS, TTL_THRESHOLD_LEDGERS,
    },
    error_codes,
    events::{self, AdminTransferred, PendingRecovery},
    storage::{self, CommonDataKey},
    upgrade::probe_supports_interface,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, BytesN, Env,
    Symbol, TryFromVal, Val, Vec,
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

/// System-parameter key for the settlement fee circuit-breaker (issue #743).
///
/// The value is a boolean flag: `1` tells settlement to skip governance's
/// `get_fee_config` and fall through to its bootstrap default; `0` (and the
/// unset default) preserves the existing behaviour. It exists so operations
/// can decouple settlement from a broken governance fee config without a
/// contract upgrade. `update_system_param` restricts it to 0/1.
const BYPASS_GOV_FEES_PARAM: &str = "bypass_gov_fees";

// Instance-storage TTL policy for short-lived reads of non-`Admin` entries
// (`RecoveryAddress` here). Deliberately shorter than the 14/30 day policy
// above because these entries are only consulted during a recovery window,
// per `adr/003-ttl-value-selection.md`.
const READ_INSTANCE_TTL_THRESHOLD: u32 = 50_000;
const READ_INSTANCE_TTL_BUMP: u32 = 100_000;

// RecoveryAddress, PendingRecovery, Paused, and Threshold live in
// `bettapay_common::storage::CommonDataKey` instead of here - see that
// type's doc comment for why a shared key type is safe to mix with this
// contract's own storage without a migration.
//
// The schema-version marker (issue #507) is instance storage and is written
// at `init`, so the first real storage migration has a defined baseline to
// distinguish "pre-marker" from "current" data.
/// Storage key layout for governance_contract.
///
/// **Naming Convention (Issue #966):**
/// All `DataKey` variants follow the UpperCamelCase singular noun convention
/// without ad-hoc abbreviations (e.g., `Admin`, `SystemParam`, `FeeConfig`,
/// `Anchor`, `SchemaVersion`, `Deployer`).
///
/// **Stability Notice:**
/// In Soroban, contracttype enum variants are serialized into ledger keys using
/// their exact identifier as an `SCVal::Symbol`. Changing existing variant
/// names breaks binary compatibility with deployed ledger entries. All
/// existing variant names MUST remain strictly unchanged.
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
    /// A recovery is already pending; `initiate_recovery` was called again
    /// before the pending recovery was executed or cancelled (issue #468).
    RecoveryAlreadyPending = 15,
    /// The anchor for the specified asset was not found.
    AnchorMissing = 200,
    InvalidParamValue = 201,
    /// `pause` was called while the contract was already paused.
    AlreadyPaused = 17,
    /// `unpause` was called while the contract was already unpaused.
    AlreadyUnpaused = 16,
    /// The new admin set and threshold are identical to the current ones.
    SameAdmin = 18,
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
    assert!(
        GovernanceError::RecoveryAlreadyPending as u32 == error_codes::RECOVERY_ALREADY_PENDING
    );
    assert!(GovernanceError::AlreadyPaused as u32 == error_codes::ALREADY_PAUSED);
    assert!(GovernanceError::AlreadyUnpaused as u32 == error_codes::ALREADY_UNPAUSED);
    assert!(GovernanceError::SameAdmin as u32 == error_codes::SAME_ADMIN);
    assert!(GovernanceError::AnchorMissing as u32 >= error_codes::GOVERNANCE_RANGE_START);
    assert!(GovernanceError::InvalidParamValue as u32 >= error_codes::GOVERNANCE_RANGE_START);
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
    pub fn init(
        env: Env,
        deployer: Address,
        admins: Vec<Address>,
        threshold: u32,
        recovery_address: Address,
    ) {
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
        for i in 0..admins.len() {
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
        assert_not_zero(&env, &new_recovery, GovernanceError::InvalidRecoveryAddress);
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &new_recovery);
        env.events().publish(
            (
                Symbol::new(&env, events::RECOVERY_ADDRESS_UPDATED_EVENT),
                new_recovery,
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
    ///   `(new_wasm_hash)`. The event is published at the same logical point
    ///   as the settlement upgrade paths — after auth and interface
    ///   validation, immediately before the executable is swapped — so the
    ///   ordering is consistent regardless of which contract or path
    ///   performs the upgrade (issue #473).
    ///
    /// ### Panics
    /// - Panics with [`Unauthorized`](GovernanceError::Unauthorized) if the caller is not the current admin.
    /// - Panics with [`InvalidWasmInterface`](GovernanceError::InvalidWasmInterface) if the new Wasm does not support interface version 1.
    pub fn upgrade(env: Env, signers: Vec<Address>, new_wasm_hash: BytesN<32>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));

        // Verify the new Wasm supports the required BettaPay interface
        // (version 1) before overwriting the running code. See
        // `bettapay_common::upgrade::probe_supports_interface`.
        if !probe_supports_interface(&env, &new_wasm_hash, 1) {
            panic_with_error!(&env, GovernanceError::InvalidWasmInterface);
        }

        // Emit `contract_upgraded` before swapping the executable so every
        // BettaPay upgrade path publishes it at the same point: after auth
        // and interface validation, immediately before the code swap (issue
        // #473).
        let caller = signers.get(0).unwrap();
        env.events().publish(
            (
                Symbol::new(&env, events::CONTRACT_UPGRADED_EVENT),
                new_wasm_hash.to_val(),
            ),
            caller,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    pub fn initiate_recovery(env: Env, new_admin: Address) {
        let recovery_address = read_recovery_address(&env);
        recovery_address.require_auth();
        assert_not_zero(&env, &new_admin, GovernanceError::InvalidAdmin);

        // Issue #468: reject a second initiation while a recovery is already
        // pending — silently overwriting the original target would hide the
        // first recovery address's intent with no distinguishing event.
        if env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(&env, GovernanceError::RecoveryAlreadyPending);
        }

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

    /// Completes a pending admin recovery initiated by [`initiate_recovery`](GovernanceContract::initiate_recovery).
    ///
    /// # Executor policy
    ///
    /// This method intentionally requires **no authorization** from the caller.
    /// The recovery target was already validated by the recovery address during
    /// `initiate_recovery`, and the 7-day delay (`RECOVERY_DELAY_SECONDS`)
    /// provides a window for the current admin set to cancel via
    /// [`cancel_recovery`](GovernanceContract::cancel_recovery).  Once the delay has elapsed, anyone may call
    /// `execute_recovery` — the pending state is consumed atomically, so a
    /// second call will revert with [`GovernanceError::RecoveryNotPending`].
    ///
    /// # Panics
    ///
    /// - [`GovernanceError::RecoveryDelayActive`] if the delay window has not
    ///   yet elapsed.
    /// - [`GovernanceError::RecoveryNotPending`] if there is no pending
    ///   recovery record in storage.
    pub fn execute_recovery(env: Env) {
        let pending = read_pending_recovery(&env);
        if env.ledger().timestamp() < pending.execute_after {
            panic_with_error!(&env, GovernanceError::RecoveryDelayActive);
        }

        // Issue #514: never let event-building read the possibly-corrupt admin
        // entry and abort recovery before it can repair the set. Resolve the
        // old admin set to an empty vector when the entry is missing or
        // corrupt, so recovery always succeeds in replacing the set.
        let old_admins = read_optional_admins(&env);

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
                old_admins,
                new_admins,
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
                old_admins,
                new_admins,
            },
        );
    }

    pub fn change_threshold(env: Env, signers: Vec<Address>, new_threshold: u32) {
        let admins = read_admins(&env);
        if new_threshold == 0 || new_threshold > admins.len() {
            panic_with_error!(&env, GovernanceError::InvalidThreshold);
        }

        let current_threshold = read_threshold(&env);
        verify_admin_auth(&env, &signers, current_threshold);

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
        if storage::is_paused(&env) {
            panic_with_error!(&env, GovernanceError::AlreadyPaused);
        }
        let admin = signers.get(0).unwrap();
        // No reason exposed on this entry point (issue #991 scope: settlement
        // only); pass the shared "unspecified" sentinel through.
        storage::apply_pause(&env, &admin, PAUSE_REASON_NONE);
    }

    pub fn unpause(env: Env, signers: Vec<Address>) {
        verify_admin_auth(&env, &signers, read_threshold(&env));
        if !storage::is_paused(&env) {
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
    ///
    /// Issue #984: a stored `SchemaVersion` below `CURRENT_SCHEMA_VERSION` is
    /// advanced one version at a time through `migrate_step`, the hook where
    /// real record conversions go. Run it as upgrade, then `migrate`, then
    /// verify. Rejected with `Paused` while the contract is paused.
    pub fn migrate(env: Env, signers: Vec<Address>) {
        assert_not_paused(&env);
        verify_admin_auth(&env, &signers, read_threshold(&env));
        let admin = signers.get(0).unwrap();

        run_migrations(&env);
        env.events().publish(
            (Symbol::new(&env, events::MIGRATED_EVENT),),
            (admin, CURRENT_SCHEMA_VERSION),
        );
    }

    pub fn update_system_param(env: Env, signers: Vec<Address>, key: Symbol, value: i128) {
        assert_admin_and_not_paused(&env, &signers);

        if value < 0 {
            panic_with_error!(&env, GovernanceError::InvalidParamValue);
        }

        // Issue #743: the fee circuit-breaker is a boolean, not a magnitude.
        // Restrict it to 0/1 so a stray value cannot be silently read as "on"
        // (settlement treats exactly 1 as enabled) or confused with a future
        // multi-valued parameter of the same name.
        if key == Symbol::new(&env, BYPASS_GOV_FEES_PARAM) && value > 1 {
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
    /// **Shared bounds (issue #987)**: `MIN_FEE_BPS` / `MAX_FEE_BPS` come from
    /// `bettapay_common::constants`, the same source the settlement contract
    /// uses for `set_settlement_rule` and `set_default_rule`, so both reject
    /// `MIN_FEE_BPS - 1` and `MAX_FEE_BPS + 1` and accept the bounds themselves.
    ///
    pub fn set_fee_config(env: Env, signers: Vec<Address>, config: FeeConfig) {
        assert_admin_and_not_paused(&env, &signers);

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
        assert_admin_and_not_paused(&env, &signers);
        assert_not_zero(&env, &asset, GovernanceError::InvalidAdmin);
        assert_not_zero(&env, &anchor, GovernanceError::InvalidAdmin);
        if asset == anchor {
            panic_with_error!(&env, GovernanceError::InvalidAdmin);
        }
        let key = DataKey::Anchor(asset.clone());
        let old_anchor: Option<Address> = env.storage().persistent().get(&key);
        env.storage().persistent().set(&key, &anchor);
        env.storage()
            .persistent()
            .extend_ttl(&key, ANCHOR_TTL_THRESHOLD, ANCHOR_TTL_BUMP);
        env.events().publish(
            (Symbol::new(&env, events::ANCHOR_UPSERTED_EVENT), asset),
            (old_anchor, anchor),
        );
    }

    pub fn remove_anchor(env: Env, signers: Vec<Address>, asset: Address) {
        assert_admin_and_not_paused(&env, &signers);
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

/// Migration hook keyed to `CURRENT_SCHEMA_VERSION` (issue #984).
///
/// Called by `migrate` once for every version bump, with `from_version` set to
/// the stored version being left behind (so the step converts `from_version`
/// records to `from_version + 1`). `migrate` writes the advanced
/// `SchemaVersion` after each step, so a step only has to convert records.
///
/// A step must be idempotent. To ship a conversion, bump
/// `CURRENT_SCHEMA_VERSION` and add the arm here, e.g.:
///
/// ```ignore
/// if from_version == 1 {
///     migrate_v1_to_v2(env);
/// }
/// ```
///
/// There is no stored-format difference to convert yet, so no step does any
/// work and `migrate` only advances the marker.
fn migrate_step(_env: &Env, from_version: u32) {
    let _ = from_version;
}

/// Advances the stored `SchemaVersion` to `CURRENT_SCHEMA_VERSION`, running
/// [`migrate_step`] for each version crossed. A no-op when already current.
fn run_migrations(env: &Env) {
    let mut version = read_schema_version(env);
    while version < CURRENT_SCHEMA_VERSION {
        migrate_step(env, version);
        version += 1;
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &version);
    }
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

/// Returns the stored admin set, or an empty vector when the admin entry is
/// missing or corrupt. Used only by `execute_recovery`, which must be able
/// to repair a corrupt admin set (issue #514) and must still report the
/// previous admin set on the `admin_transferred` event (issue #977).
fn read_optional_admins(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| Vec::new(env))
}

/// Returns the primary admin address, or the zero-address sentinel when the
/// admin entry is missing or has no primary. Preserved per ADR-004.
#[allow(dead_code)] // Preserved for ADR-004 compatibility
fn read_optional_primary_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Vec<Address>>(&DataKey::Admin)
        .and_then(|admins| storage::primary_admin(&admins))
        .unwrap_or_else(|| storage::zero_address(env))
}

/// Rejects the zero address with a caller-supplied `GovernanceError`.
///
/// Backed by [`bettapay_common::storage::is_zero_address`].
/// In governance entry points (such as `upsert_anchor` and admin validation),
/// zero addresses map to `GovernanceError::InvalidAdmin` because governance
/// lacks a dedicated `ZeroAddress` error code and treats zero address arguments
/// as invalid administrative parameters.
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

/// Enforces that the contract is not paused, then verifies admin multisig authorization.
///
/// Precedence: The pause check takes precedence over multisig authorization.
/// A paused contract immediately panics with [`GovernanceError::Paused`] before evaluating
/// caller signers or returning [`GovernanceError::Unauthorized`].
fn assert_admin_and_not_paused(env: &Env, signers: &Vec<Address>) {
    assert_not_paused(env);
    verify_admin_auth(env, signers, read_threshold(env));
}

/// Shared test setup helper used across the main test module and the anchor_*
/// sub-modules, so a change to `init`'s signature only needs updating here.
#[cfg(test)]
pub(crate) fn setup_with(
    admin_count: u32,
    threshold: u32,
) -> (
    Env,
    GovernanceContractClient<'static>,
    Vec<Address>,
    Address,
) {
    use soroban_sdk::testutils::Address as _;
    let env = Env::default();
    env.mock_all_auths();

    let mut admins = Vec::new(&env);
    for _ in 0..admin_count {
        admins.push_back(Address::generate(&env));
    }
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &threshold, &recovery_address);
    (env, client, admins, recovery_address)
}

#[cfg(test)]
pub(crate) fn setup() -> (Env, GovernanceContractClient<'static>, Vec<Address>) {
    let (env, client, admins, _) = setup_with(1, 1);
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
mod tests;
