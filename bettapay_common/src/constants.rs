//! Protocol-wide constants shared by the governance and settlement contracts.
//!
//! Every value here is a pure compile-time constant with no Soroban dependencies,
//! so it can be referenced from both crates without pulling in extra runtime
//! cost.

/// The basis-point denominator: 1 bps is 0.01 %, so `BPS_DENOMINATOR` bps == 100 %.
///
/// Used by both contracts to range-check fee splits and to perform the
/// integer-arithmetic conversion from fee basis points to per-payment fee
/// amounts.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Minimum allowed protocol-wide fee, in basis points (0.05 %).
///
/// Both the governance `FeeConfig` and the settlement `SettlementRule`
/// enforce this lower bound. The two contracts used to declare this constant
/// independently, with the settlement one carrying a comment that it had to
/// match the governance one — the duplication was a real source of risk.
pub const MIN_FEE_BPS: u32 = 5;

/// Maximum allowed protocol-wide fee, in basis points (50 %).
///
/// Enforced by both the governance `FeeConfig` and the settlement
/// `SettlementRule`/default-rule setters, independent of whether a
/// governance `FeeConfig` has been set yet — settlement's separate
/// `validate_fee_against_governance` check only tightens the ceiling
/// further once governance configures one, it never has to be the thing
/// that first caps a per-fee value.
pub const MAX_FEE_BPS: u32 = 5_000;

/// Approximate number of Soroban ledgers closed per day, given the 5-second
/// close time on public networks.
///
/// Used together with [`TTL_THRESHOLD_LEDGERS`] and [`TTL_BUMP_LEDGERS`] to
/// derive human-readable TTL policies from a single day-count constant.
pub const LEDGERS_PER_DAY: u32 = 17_280;

/// Persistent-storage TTL threshold (in ledgers) below which a read or write
/// should bump the entry's remaining lifetime.
///
/// Set to roughly 14 days. Anything above this is considered "still warm" and
/// is left alone to keep `extend_ttl` out of the hot path.
pub const TTL_THRESHOLD_LEDGERS: u32 = LEDGERS_PER_DAY * 14;

/// Persistent-storage TTL bump (in ledgers) applied on reads or writes.
///
/// Set to roughly 30 days so an active entry only needs to be touched once a
/// month to stay reachable. Pairs with [`TTL_THRESHOLD_LEDGERS`].
pub const TTL_BUMP_LEDGERS: u32 = LEDGERS_PER_DAY * 30;

/// Cooldown between `initiate_recovery` and `execute_recovery`: seven days,
/// expressed in seconds. Scheduled settlement administrative operations use a
/// delay of at least this long. This ordering is part of the threat model:
/// recovery must be able to veto compromised-admin upgrades and admin
/// transfers before they execute.
pub const RECOVERY_DELAY_SECONDS: u64 = 7 * 24 * 60 * 60;
