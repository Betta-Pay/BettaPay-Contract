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
//! - `topic[0]` is always the event name as a [`Symbol`], constructed via
//!   [`Symbol::new`]. Indexers filter on this single topic to dispatch by
//!   event type.
//! - `topic[1..n]` carry the entity identifiers that scope the event —
//!   typically an [`Address`] (merchant, asset, admin), but for some events
//!   also a [`BytesN<32>`] (new Wasm hash on `contract_upgraded`, payment
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
//! `store_payment_reference`, `update_governance` all call
//! `assert_not_paused`). The following administrative operations are
//! intentionally NOT blocked during pause, so the admin can fix the root
//! cause of the emergency:
//!
//! - `upgrade` — deploy a fix
//! - `transfer_admin` — rotate compromised keys
//! - `initiate_recovery` / `cancel_recovery` / `execute_recovery` — the
//!   admin-recovery flow itself must keep working while paused
//! - `schedule` / `execute` / `cancel` — scheduled operations must proceed
//!
//! ## Event Convention
//!
//! This contract follows a consistent event emission pattern (see Issue #49):
//!
//! **Topics** carry the fixed event-name symbol and filterable entity
//! identifiers. The first topic is always a [`Symbol`] naming the event type,
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

mod admin;
mod errors;
mod merchant;
mod payments;
mod settlement;
mod storage;
mod types;

#[cfg(test)]
mod tests;

use bettapay_common::constants::MIN_FEE_BPS;
use soroban_sdk::contract;

pub use errors::SettlementError;
pub use types::{
    Bps, FeeSplit, GovFeeConfig, Operation, PaymentRecord, ScheduledOp, SettlementRule,
};

/// Minimum gross payment amount, in the asset's smallest unit.
///
/// Derived as `BPS_DENOMINATOR / 100`. Each fee leg rounds up
/// ([`Bps::calculate_fee_ceil`]) and so over-collects by strictly less than one
/// unit; at 100 units that unit is 1 % of the gross, capping the inflation of a
/// leg's effective rate below 100 bps over its configured bps. Below the floor the
/// distortion is unbounded — 1 unit charged the minimum legal fee (`MIN_FEE_BPS`,
/// 0.05 %) still rounds up to 1 unit, a 100 % effective fee. 100 is also the
/// smallest amount at which [`BOOTSTRAP_DEFAULT_RULE`]'s 100 bps platform fee is
/// exact. Nothing but dust is excluded: on a 7-decimal Soroban asset, 100 base
/// units is 10^-5 of one token.
///
/// Enforced by [`SettlementContract::store_payment_reference`], which panics with
/// [`AmountTooSmall`](SettlementError::AmountTooSmall). It does **not** guarantee a
/// non-negative merchant payout — extreme fee rules can still make the two
/// rounded-up legs exceed the gross; see `calculate_split` for that edge case.
pub(crate) const MIN_PAYMENT_AMOUNT: i128 = 100;
pub(crate) const MAX_SETTLEMENT_DELAY_LEDGER: u32 = 100_000;

/// Default settlement delay period applied as a fallback (1 day equivalent in ledgers)
pub const FALLBACK_SETTLEMENT_DELAY_LEDGER: u32 = 17280;

/// Maximum number of payments that can be retrieved in a single batch lookup
pub const MAX_PAYMENTS_BATCH: u32 = 100;

/// Approximate ledgers per day on Stellar (~5s per ledger).
pub(crate) const LEDGERS_PER_DAY: u32 = 17280;

// TTL thresholds and bumps use LEDGERS_PER_DAY for readability.
pub(crate) const PAYMENT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14; // 14 days
pub(crate) const PAYMENT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30; // 30 days
pub(crate) const RULE_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
pub(crate) const RULE_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;
pub(crate) const MERCHANT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
pub(crate) const MERCHANT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;

pub(crate) const DEFAULT_TIMELOCK_DELAY_SECONDS: u64 = 2 * 24 * 60 * 60; // 48 hours

// Settlement-specific TTL policy for short-lived reads of admin / governance /
// recovery addresses. Deliberately shorter than the protocol defaults so that
// an inactive instance-side entry can still be evicted in days rather than
// weeks — see ADR 003 for the rationale.
pub(crate) const READ_INSTANCE_TTL_THRESHOLD: u32 = 50_000;
pub(crate) const READ_INSTANCE_TTL_BUMP: u32 = 100_000;

// Used until the admin sets a global default settlement rule.
pub(crate) const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
    platform_fee_bps: 100,
    network_fee_bps: MIN_FEE_BPS,
    settlement_delay_ledger: 0,
    auto_settle: false,
};

#[contract]
pub struct SettlementContract;
