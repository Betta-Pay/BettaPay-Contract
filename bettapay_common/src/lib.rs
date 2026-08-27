//! # BettaPay Common
//!
//! Shared types, constants, and helpers for the BettaPay protocol's Soroban
//! contracts. Both the governance contract and the settlement contract depend
//! on this crate to avoid duplicating protocol-wide constants and the small set
//! of storage/event idioms every contract needs.
//!
//! ## Modules
//!
//! - [`constants`] — protocol-wide numeric constants (basis-point denominators,
//!   fee bounds, TTL helpers, recovery delay).
//! - [`storage`] — common storage helpers (`is_paused`, `set_paused`,
//!   `is_zero_address`) and the [`CommonDataKey`] enum, the single
//!   declaration of the key shapes (`Paused`, `RecoveryAddress`,
//!   `PendingRecovery`, `Threshold`) every contract writes into its own,
//!   separate instance storage — see [`storage`]'s doc comment for what
//!   "storage separation" means here.
//! - [`events`] — shared event shapes (such as [`events::AdminTransferred`] and
//!   [`events::PendingRecovery`]) and the canonical event-name strings.
//! - [`error_codes`] — the canonical numeric error-code registry. Every
//!   contract's `#[contracterror]` enum must use these constants as
//!   discriminants for shared error concepts, and stay within its reserved
//!   range for contract-specific ones, so that a raw error code means the
//!   same thing regardless of which contract raised it. See issue #517.
//!
//! ## Why this crate exists
//!
//! Without it, the two contracts each defined their own copy of `MIN_FEE_BPS`,
//! `BPS_DENOMINATOR`, `RECOVERY_DELAY_SECONDS`, the `Admin`/`Paused`/`RecoveryAddress`
//! storage accessors, and the `AdminTransferred` event payload — easy to let
//! drift out of sync on a quiet PR. See issue #315 for the original
//! motivation; further module extraction (event helpers, error mapping) is
//! tracked by issue #84.

#![no_std]

pub mod constants;
pub mod error_codes;
pub mod events;
pub mod storage;
pub mod types;
