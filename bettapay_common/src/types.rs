//! Shared contract types used by more than one contract or by
//! `bettapay_common`'s own event builders.
//!
//! `Bps` and `SettlementRule` were originally defined in
//! `settlement_contract`; they live here so the shared event helpers in
//! [`crate::events`] can take a typed `SettlementRule` payload instead of a
//! hand-rolled tuple. `settlement_contract` re-exports them from its own
//! `types` module, so nothing downstream needs to change its import paths.

use soroban_sdk::contracttype;

use crate::constants::BPS_DENOMINATOR;

/// A type-safe wrapper around basis points (`u32`).
///
/// Provides explicit conversion methods and fee arithmetic helpers to prevent
/// ad-hoc inline casting (`as i128`) and potential truncation or calculation errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[contracttype]
pub struct Bps(pub u32);

impl Bps {
    /// Constructs a new `Bps` instance.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the underlying `u32` basis point value.
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Converts basis points to `i128` for safe fee arithmetic.
    pub const fn as_i128(self) -> i128 {
        self.0 as i128
    }

    /// Calculates ceil-rounded fee amount for a given gross amount:
    /// `ceil(amount * bps / BPS_DENOMINATOR) = (amount * bps + BPS_DENOMINATOR - 1) / BPS_DENOMINATOR`.
    pub fn calculate_fee_ceil(self, amount: i128) -> i128 {
        let denom = BPS_DENOMINATOR as i128;
        (amount * self.as_i128() + denom - 1) / denom
    }
}

impl From<u32> for Bps {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Bps> for u32 {
    fn from(bps: Bps) -> Self {
        bps.0
    }
}

/// Configuration governing how merchant payments are settled.
///
/// This struct defines the fee allocation and settlement timing for a merchant,
/// including the platform and network fee shares as well as whether
/// settlement is processed automatically after a delay.
#[derive(Clone)]
#[contracttype]
pub struct SettlementRule {
    /// Platform fee charged on each payment, expressed in basis points.
    ///
    /// One basis point is 0.01%, and 100 basis points equals 1%.
    /// This value is used when calculating the platform's share of a payment.
    pub platform_fee_bps: u32,
    /// Network fee charged on each payment, expressed in basis points.
    ///
    /// This represents the portion reserved for network or protocol-related
    /// costs and is combined with other fees as validated elsewhere in the contract.
    pub network_fee_bps: u32,
    /// Number of ledger closes to wait before settlement becomes eligible.
    ///
    /// A value of `0` enables immediate settlement, while larger values delay
    /// settlement until the specified number of ledgers has elapsed.
    pub settlement_delay_ledger: u32,
    /// Indicates whether settlement should occur automatically.
    ///
    /// When set to `true`, settlements may be processed automatically after
    /// the configured settlement delay has elapsed; when `false`, settlement
    /// requires manual or external triggering.
    pub auto_settle: bool,
}

impl SettlementRule {
    /// Returns the platform fee as a typed `Bps` wrapper.
    pub fn platform_bps(&self) -> Bps {
        Bps::new(self.platform_fee_bps)
    }

    /// Returns the network fee as a typed `Bps` wrapper.
    pub fn network_bps(&self) -> Bps {
        Bps::new(self.network_fee_bps)
    }
}
