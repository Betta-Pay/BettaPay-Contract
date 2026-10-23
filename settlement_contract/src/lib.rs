//! # BettaPay Settlement Contract
//!
//! This module provides the core implementation of the settlement contract for BettaPay.
//! It is responsible for managing merchant registration, settlement rules, and the payment storage architecture.
//!
//! ## Merchant Rules
//!
//! The contract maintains a registry of authorized merchants. For each registered merchant,
//! an admin can configure specific settlement rules defined by the `SettlementRule` struct.
//! A settlement rule dictates:
//! - **Platform Fee (BPS)**: The fee collected by the platform.
//! - **Network Fee (BPS)**: The fee collected by the network.
//! - **Settlement Delay**: The delay in ledger sequences before a settlement can occur.
//! - **Auto-settle**: A flag indicating whether auto-settlement is enabled.
//!
//! If a merchant lacks a specific rule, the system falls back to an admin-configured global default rule,
//! and ultimately to a hardcoded bootstrap default rule if necessary.
//!
//! ## Payment Storage Architecture
//!
//! Payments are tracked and stored using a unique 32-byte reference (`BytesN<32>`).
//! When `store_payment_reference` is invoked, the contract calculates the exact fee split
//! (platform fee, network fee, and net merchant amount) based on the merchant's effective settlement rule.
//!
//! The resulting data is persisted in a `PaymentRecord`, which encapsulates:
//! - The calculated amounts and fee BPS.
//! - The ledger sequence of the transaction.
//! - Settlement delay and auto-settle configurations.
//!
//! The contract leverages different `DataKey` variants (`Admin`, `Merchant`, `Rule`, `Payment`, etc.)
//! to securely organize persistent and instance storage, while applying TTL extensions to ensure
//! active records remain available and do not expire prematurely.
//!
//! ### Important: Key Enumeration Workaround
//! Soroban cannot enumerate persistent storage keys. The `get_payments` method does not
//! iterate over all stored payments — instead, callers **must supply references** observed
//! from `payment_stored` events (or other indexer-sourced sources). Each reference is then
//! looked up individually via `get_payment_reference`.
//!
//! ## Storage Key Ownership (issues #772, #930)
//!
//! Authoritative mapping of storage keys across the contract ecosystem, their owning
//! crates, storage kinds, and TTL policies:
//!
//! | Key | Owning Crate | Storage Kind | TTL Policy | Notes |
//! |---|---|---|---|---|
//! | `CommonDataKey::RecoveryAddress` | `bettapay_common` | Instance | Instance TTL (`50k` / `100k` ledgers) | **Shared**: Singleton recovery address |
//! | `CommonDataKey::PendingRecovery` | `bettapay_common` | Instance | Instance TTL | **Shared**: Active recovery state; acts as timelock veto in settlement |
//! | `CommonDataKey::Paused` | `bettapay_common` | Instance | Instance TTL | **Shared**: Global emergency pause flag |
//! | `CommonDataKey::Threshold` | `bettapay_common` | Instance | Instance TTL (`50k` / `100k` ledgers) | **Shared**: Multisig admin signature threshold |
//! | `DataKey::Admin` | `settlement_contract` | Instance | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local**: Multisig `Vec<Address>` (governance has independent `Admin`) |
//! | `DataKey::Initializing` | `settlement_contract` | Instance | Ephemeral | **Contract-Local / Settlement-Only**: Sentinel guard against init reentrancy |
//! | `DataKey::Governance` | `settlement_contract` | Instance | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local / Settlement-Only**: Address of governance contract |
//! | `DataKey::Deployer` | `settlement_contract` | Instance | Instance TTL | **Contract-Local**: Deployer address gating init front-running |
//! | `DataKey::SchemaVersion` | `settlement_contract` | Instance | Instance TTL | **Contract-Local**: Storage schema version (mirrored locally in governance) |
//! | `DataKey::CachedGovRule` | `settlement_contract` | Instance | Instance TTL | **Contract-Local / Settlement-Only**: Caches last-good governance fee config |
//! | `DataKey::DefaultRule` | `settlement_contract` | Instance | Instance TTL | **Contract-Local / Settlement-Only**: Singleton global default rule |
//! | `DataKey::Merchant(Address)` | `settlement_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Bumped on merchant/admin writes only |
//! | `DataKey::Rule(Address)` | `settlement_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Bumped on rule writes and authenticated reads |
//! | `DataKey::ArchivedMerchant(Address)` | `settlement_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Tombstone written on unregister |
//! | `DataKey::Payment(Address, BytesN<32>)` | `settlement_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Scoped by merchant and payment reference |
//! | `DataKey::ScheduledOperation(BytesN<32>)` | `settlement_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Timelocked operation payload and execute_at |
//! | `DataKey::PendingOperationCount` | `settlement_contract` | Instance | Instance TTL | **Contract-Local / Settlement-Only**: Pending timelock queue depth, incremented by `schedule` and decremented by `execute` / `cancel` / `cancel_all` |
//! | `governance_contract::DataKey::Admin` | `governance_contract` | Instance | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local**: Governance admin set |
//! | `governance_contract::DataKey::FeeConfig` | `governance_contract` | Instance | Instance TTL | **Contract-Local / Governance-Only**: Protocol fee ceilings |
//! | `governance_contract::DataKey::SchemaVersion` | `governance_contract` | Instance | Instance TTL | **Contract-Local**: Governance schema version |
//! | `governance_contract::DataKey::Deployer` | `governance_contract` | Instance | Instance TTL | **Contract-Local**: Governance deployer front-running guard |
//! | `governance_contract::DataKey::Anchor(Address)` | `governance_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Governance-Only**: Anchor address per asset |
//! | `governance_contract::DataKey::SystemParam(Symbol)` | `governance_contract` | Persistent | `14d` threshold / `30d` bump (`241,920` / `518,400` ledgers) | **Contract-Local / Governance-Only**: Numeric system configuration parameter |
//!
//! ### Key Segregation and Sharing Callouts
//! - **Shared Keys**: `Threshold`, `Paused`, `RecoveryAddress`, and `PendingRecovery` are declared
//!   in `bettapay_common::storage::CommonDataKey` and shared across both contracts without redeclaration.
//! - **Contract-Local Admin**: `Admin` is contract-local (`Vec<Address>`) declared in each contract's
//!   own `DataKey` enum, preserving administrative separation.
//! - **Settlement-Only Keys**: `CachedGovRule`, `Initializing`, `Governance`, `DefaultRule`, `Merchant`,
//!   `Rule`, `ArchivedMerchant`, `Payment`, `ScheduledOperation`, and `PendingOperationCount`.
//! - **Governance-Only Keys**: `FeeConfig`, `Anchor`, and `SystemParam`.
//! - **Local Duplications**: `SchemaVersion` and `Deployer` are declared locally in each contract
//!   to manage contract-specific migration baselines and deployer authorization independently.
//!
//! Rule of thumb: instance keys are singletons tied to the contract instance
//! lifetime (no independent expiry); persistent keys are per-entity records
//! that must be kept warm with `extend_ttl` or they are evicted.
//! See also `docs/storage-key-ownership.md` for full cross-crate documentation.
//!
//! ## Payment Reads
//!
//! Single-record payment reads are authorized as either the owning merchant
//! or an admin. Pass an empty `signers` vector to use the merchant path, which
//! requires the merchant's authorization. Pass a non-empty admin signer vector
//! to use the admin path, which requires the configured signature threshold.
//! A payment reference identifies a record; it does not grant read access.
//!
//! ## Settlement Boundary (Off-Chain Execution)
//!
//! This contract calculates and securely locks the fee split for each payment in a `PaymentRecord` and emits a
//! `payment_stored` event. It does **not** transfer tokens, hold funds, or expose an in-contract `settle` function.
//!
//! Settlement execution is intentionally designed to be **off-chain**:
//! 1. **Indexers** listen to `payment_stored` events and read the `PaymentRecord` state.
//! 2. **Readiness** is verified off-chain by evaluating if the current ledger sequence satisfies the delay:
//!    `current_ledger >= record.ledger + record.settlement_delay_ledger`.
//! 3. **Execution** happens via a separate off-chain payout engine that processes transfers (batching where
//!    appropriate based on `auto_settle` preferences) and tracks settlement state externally.
//!
//! The in-contract flags (`settlement_delay_ledger`, `auto_settle`) are strictly informational directives
//! enforcing standardized agreement parameters for off-chain consumers; they do not trigger on-chain state transitions.
//!
//! ## Event Conventions
//!
//! Events are emitted via [`soroban_sdk::Env::events`]. To give off-chain
//! indexers a predictable topic layout, every event in this contract follows
//! the same conventions:
//!
//! - `topic[0]` is always the event name as a [`Symbol`](soroban_sdk::Symbol), constructed via
//!   [`Symbol::new`](soroban_sdk::Symbol::new). Indexers filter on this single topic to dispatch by
//!   event type.
//! - `topic[1..n]` carry the entity identifiers that scope the event —
//!   typically an [`Address`](soroban_sdk::Address) (merchant, asset, admin), but for some events
//!   also a [`BytesN<32>`](soroban_sdk::BytesN) (new Wasm hash on `contract_upgraded`, payment
//!   reference on `payment_stored`). The exact shape of `topic[1..n]` is
//!   fixed per event.
//! - The **data payload** carries the values describing the state change.
//!   Its shape is event-specific: a single value (`admin` for
//!   `merchant_registered`), a tuple (e.g. `(admin, prev, rule)` for
//!   `settlement_rule_updated`, or `(admin, true)` for `paused`), a typed
//!   struct such as the `SettlementRule` emitted on `bootstrap_fallback` or
//!   the shared [`bettapay_common::events::AdminTransferred`] payload on
//!   `admin_transferred` / `recovery_executed`, or `()`.
//!
//! Events shared with the governance contract (`admin_transferred`, `paused`,
//! `unpaused`, `recovery_initiated`, `recovery_cancelled`, `recovery_executed`,
//! `threshold_changed`, `contract_upgraded`) use the canonical topic names and
//! payload shapes defined in [`bettapay_common::events`] so an indexer can
//! decode them identically no matter which contract published them.
//! - Each entry point emits exactly the events tied to the state change it
//!   performs. Usually, no two events emitted by the same call describe the
//!   same logical change, though exceptions exist (e.g., `_set_settlement_rule`
//!   emits both `bootstrap_fallback` and `settlement_rule_updated` when falling back).
//!
//! ## Upgrade Process
//!
//! [`SettlementContract::upgrade`] replaces the Wasm and nothing else. That is
//! what makes it safe, and also why changing a stored type is a separate
//! problem: nothing converts existing entries, and nothing checks that they
//! still match the types the new code expects. A mismatched read fails at
//! runtime, after the upgrade has already landed.
//!
//! 1. Wasm upgrades replace code only; every storage entry survives untouched.
//! 2. Storage migrations run **inside the upgraded contract**, as an
//!    admin-gated `migrate` entry point — not from a separate migration
//!    contract. A contract can only reach its own storage, so another contract
//!    has no access path to `Payment`, `Merchant` or `Rule` entries.
//! 3. Ship the old type definition in the same Wasm as the new one. A
//!    `#[contracttype]` struct is encoded by field name, so a `PaymentRecord`
//!    written before a new field existed will not deserialise into the new
//!    struct — the old type is what keeps those entries readable.
//! 4. Order is: upgrade the Wasm, then call `migrate`, then verify the
//!    post-upgrade state, then remove the migration code in a later upgrade.
//!    `migrate` is admin-gated, rejected while paused, and idempotent: at
//!    `CURRENT_SCHEMA_VERSION` it is a no-op, and below it, it runs the
//!    `migrate_step` hook once per version crossed and advances the stored
//!    `SchemaVersion`. A real conversion bumps `CURRENT_SCHEMA_VERSION` and adds
//!    its step to that hook; `governance_contract` has the same hook.
//! 5. `Payment(Address, BytesN<32>)`, `Merchant(Address)` and `Rule(Address)`
//!    are keyed by value and Soroban cannot enumerate storage keys — which is
//!    why [`SettlementContract::get_payments`] takes the merchant and the
//!    references from the caller. Convert these lazily on read, or pass the
//!    keys in explicitly.
//! 6. Call `extend_ttl` on anything the migration rewrites: `set` alone does
//!    not extend an entry's life, so a migrated record would otherwise expire
//!    sooner than an untouched one.
//!
//! Full guidance, including worked examples and how to test a migration, is in
//! [`DEVELOPMENT.md`](https://github.com/Betta-Pay/BettaPay-Contract/blob/main/DEVELOPMENT.md).
//!
//! ## Pause Model
//! The pause flag blocks payment-processing and merchant-management
//! operations (`register_merchant`, `unregister_merchant`,
//! `set_settlement_rule`, `clear_settlement_rule`, `set_default_rule`,
//! `store_payment_reference`, `prune_payment`, `update_governance` all call
//! `assert_not_paused`). The following administrative operations are
//! intentionally NOT blocked during pause, so the admin can fix the root
//! cause of the emergency:
//!
//! - `upgrade` — deploy a fix
//! - `transfer_admin` — rotate compromised keys
//! - `initiate_recovery` / `cancel_recovery` / `execute_recovery` — the
//!   admin-recovery flow itself must keep working while paused
//!
//! The timelocked queue is the opposite: `schedule`, `execute` and `cancel`
//! all call `assert_not_paused` and are **rejected with
//! [`SettlementError::Paused`] while the contract is paused**. A queued
//! operation is not lost by this: it stays in storage untouched and can be
//! executed or cancelled once the contract is unpaused. The rationale is that
//! the timelock is a deferred, planned path, whereas an incident is handled
//! through the direct admin operations listed above (plus `unpause`), which
//! stay available. Freezing the queue also stops a previously scheduled
//! operation from firing in the middle of an incident; recovery additionally
//! vetoes the queue (see `initiate_recovery`). Pinned by
//! `paused_timelock_operations_are_rejected_and_queue_survives`.
//!
//! `pause`/`unpause` themselves are guarded to be idempotent, mirroring
//! governance: pausing while already paused panics with `AlreadyPaused`, and
//! unpausing while not paused panics with `AlreadyUnpaused`, so a `paused`/
//! `unpaused` event is only ever emitted on an actual state transition.
//!
//! ## Fail-Open vs Fail-Closed Policy Under Governance Degradation (Issue #749)
//!
//! When the external governance contract is degraded, unreachable, or traps,
//! the settlement contract follows a strict fail-open vs fail-closed policy per
//! entry point:
//!
//! - **Reads and payments fail open**: Core payment ingestion and query paths
//!   must remain operational. If governance fee resolution is unavailable,
//!   `store_payment_reference` falls back to [`BOOTSTRAP_DEFAULT_RULE`].
//! - **Admin setters fail closed**: Administrative operations, rule updates,
//!   and contract modifications must never execute with unverified governance
//!   parameters and reject with [`SettlementError::GovernanceCallFailed`].
//!
//! | Entry Point | Policy | Behavior on Governance Outage / Trap |
//! |---|---|---|
//! | `store_payment_reference` | fail-open to bootstrap | Degrades to `BOOTSTRAP_DEFAULT_RULE` and emits `bootstrap_fallback` |
//! | `get_payment` / `get_payments` | fail-open | Storage read only; unaffected by governance availability |
//! | `get_payment_reference` | fail-open | Storage read only; unaffected by governance availability |
//! | `get_rule` / `get_default_rule` | fail-open | Storage read only; unaffected by governance availability |
//! | `is_merchant_registered` | fail-open | Storage read only; unaffected by governance availability |
//! | `get_min_payment_amount` | fail-open | Storage read only; unaffected by governance availability |
//! | `get_version` | fail-open | Storage read only; unaffected by governance availability |
//! | `set_settlement_rule` | fail-closed GovernanceCallFailed | Reverts with `SettlementError::GovernanceCallFailed` |
//! | `set_default_rule` | fail-closed GovernanceCallFailed | Reverts with `SettlementError::GovernanceCallFailed` |
//! | `update_governance` | fail-closed GovernanceCallFailed | Reverts if new governance contract fails interface checks |
//! | `register_merchant` / `unregister_merchant` | fail-closed | Requires valid admin multisig authorization |
//! | `clear_settlement_rule` | fail-closed | Requires valid admin multisig authorization |
//! | `set_min_payment_amount` | fail-closed | Requires valid admin multisig authorization |
//! | `transfer_admin` / `change_threshold` | fail-closed | Requires valid admin multisig authorization |
//! | `pause` / `unpause` | fail-closed | Requires valid admin multisig authorization |
//! | `upgrade` | fail-closed | Requires valid admin multisig authorization and interface check |
//! | `schedule` | fail-closed GovernanceCallFailed | Probes governance `get_fee_config` before writing; an unreachable governance reverts with `SettlementError::GovernanceCallFailed` and writes nothing (issue #994) |
//! | `schedule` / `execute` / `cancel` | fail-closed | Requires valid admin multisig authorization and timelock (`schedule`/`cancel`); also rejected with `Paused` while the contract is paused |
//!
//! ## Event Convention
//!
//! This contract follows a consistent event emission pattern (see Issue #49):
//!
//! **Topics** carry the fixed event-name symbol and filterable entity
//! identifiers. The first topic is always a [`Symbol`](soroban_sdk::Symbol) naming the event type,
//! enabling indexers to filter by event kind. Subsequent topics hold the
//! primary identifiers relevant to the event (e.g., merchant address, payment
//! reference), so listeners can subscribe to events for a specific entity
//! without scanning all events.
//!
//! **Data** carries caller context and event-specific details — information
//! that is useful once the event has been matched by topic but is not needed
//! for filtering. Typically this includes the caller's address (admin) and
//! any before/after values or configuration data.
//!
//! ### Canonical Example
//!
//! [`SettlementContract::register_merchant`] is the canonical example of this convention:
//!
//! | Role   | Value                                                       |
//! |--------|-------------------------------------------------------------|
//! | Topics | `(Symbol("merchant_registered"), Address merchant)`         |
//! | Data   | `Address caller` (the admin who authorized the registration)|
//!
//! New events should follow the same pattern: filterable identifiers in
//! topics, caller context and details in data.
//!
//! ## Module Layout
//!
//! - [`types`] — shared contract data types (`SettlementRule`, `PaymentRecord`, `FeeSplit`, `Operation`, `DataKey`, ...).
//! - [`policy`] — protocol constants, TTL thresholds/bumps, timelock delays, and default rule configurations.
//! - [`errors`] — the [`SettlementError`] enum.
//! - [`storage`] — low-level storage read/validate helpers shared across entry points.
//! - [`admin`] — init, admin transfer/recovery, pause/unpause, upgrade, scheduled-operation execution.
//! - [`merchant`] — merchant registration.
//! - [`settlement`] — settlement rule configuration (per-merchant and global default).
//! - [`payments`] — payment reference storage and fee-split calculation.
//!
//! Each of `admin`, `merchant`, `settlement`, and `payments` contributes its own
//! `#[contractimpl] impl SettlementContract { ... }` block; Soroban merges these
//! into a single generated `SettlementContractClient`, so callers see one
//! unified contract API regardless of how the implementation is split across files.

#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

mod admin;
mod errors;
mod merchant;
mod payments;
pub mod policy;
mod settlement;
mod storage;
mod types;

#[cfg(test)]
mod tests;

use soroban_sdk::contract;

pub use errors::SettlementError;
pub(crate) use policy::*;
pub use policy::{
    FALLBACK_SETTLEMENT_DELAY_LEDGER, MAX_PAYMENTS_BATCH, MAX_PAYMENTS_PER_MERCHANT,
    MAX_TIMELOCK_DELAY_SECONDS,
};
pub use storage::{assert_admin_and_not_paused, validate_nonzero_address};
pub use types::{
    Bps, EffectiveRule, FeeSplit, GovFeeConfig, Operation, PaymentRecord, PaymentsPage,
    RuleProvenance, ScheduledOp, SettlementRule,
};

#[contract]
pub struct SettlementContract;
