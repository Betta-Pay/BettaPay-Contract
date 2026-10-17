//! Protocol constants, TTL policies, and default configurations for the settlement contract.

use crate::types::SettlementRule;
use bettapay_common::constants::MIN_FEE_BPS;

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
/// [`AmountTooSmall`](crate::errors::SettlementError::AmountTooSmall). It does **not** guarantee a
/// non-negative merchant payout — extreme fee rules can still make the two
/// rounded-up legs exceed the gross; see `calculate_split` for that edge case.
pub const MIN_PAYMENT_AMOUNT: i128 = 100;
pub const MAX_SETTLEMENT_DELAY_LEDGER: u32 = 100_000;

/// Default settlement delay period applied as a fallback (1 day equivalent in ledgers)
pub const FALLBACK_SETTLEMENT_DELAY_LEDGER: u32 = 17280;

/// Maximum number of payments that can be retrieved in a single batch lookup
pub const MAX_PAYMENTS_BATCH: u32 = 100;

/// Fixed byte length of a payment reference (`BytesN<32>`), named so callers
/// and docs have a canonical value instead of a bare `32`. Enforced
/// structurally by the `BytesN<32>` type, not by a runtime check.
pub const MAX_PAYMENT_REFERENCE_LEN: u32 = 32;

/// Maximum number of payment records that may be stored per merchant.
///
/// Rationale: payment entries live in persistent storage (`DataKey::Payment`)
/// and each entry accrues rent via TTL extension, so an unbounded per-merchant
/// history would let a single merchant bloat ledger state without bound.
/// A single named cap gives tests and docs one canonical value to assert
/// against. Enforcement (rejecting `store_payment_reference` past this cap)
/// is intentionally left to a separate standalone change (issue #775 scope:
/// constant plus docs only, no enforcement logic here).
pub const MAX_PAYMENTS_PER_MERCHANT: u32 = 10_000;

/// Approximate ledgers per day on Stellar (~5s per ledger).
pub const LEDGERS_PER_DAY: u32 = 17280;

// TTL thresholds and bumps use LEDGERS_PER_DAY for readability.
pub const PAYMENT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14; // 14 days
pub const PAYMENT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30; // 30 days
pub const RULE_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
pub const RULE_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;
pub const MERCHANT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
pub const MERCHANT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;
pub const SCHEDULED_OP_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14; // 14 days
pub const SCHEDULED_OP_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30; // 30 days

/// Minimum delay for scheduled administrative operations.
///
/// This matches the seven-day recovery window so the recovery address has
/// time to replace compromised admins before a scheduled upgrade or admin
/// transfer can execute. The recovery path is the veto authority for pending
/// schedules; ordinary admin cancellation remains available as well.
pub const DEFAULT_TIMELOCK_DELAY_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Minimum enforceable delay for any scheduled administrative operation.
///
/// `schedule()` rejects any `execute_in` below this floor with
/// [`SettlementError::ExecutionNotReady`](crate::errors::SettlementError::ExecutionNotReady),
/// keeping the timelock guarantee auditable and documented and ensuring the recovery veto window cannot
/// silently shrink. The floor is intentionally equal to
/// [`DEFAULT_TIMELOCK_DELAY_SECONDS`] (seven days) so the recovery address
/// always has time to replace compromised admins before any scheduled
/// operation can execute.
pub const MIN_TIMELOCK_DELAY_SECONDS: u64 = DEFAULT_TIMELOCK_DELAY_SECONDS;

/// The single interface version advertised by `supports_interface`.
///
/// `upgrade` probes the incoming Wasm with `supports_interface(SUPPORTED_INTERFACE_VERSION)`
/// before committing the swap. Any Wasm that returns `false` (or traps) is
/// rejected with `InvalidWasmInterface`. Increment this constant in a future
/// Wasm update when a breaking API change requires callers to distinguish the
/// new contract from this one (issue #48).
pub const SUPPORTED_INTERFACE_VERSION: u32 = 1;

/// The schema version this build expects. `init` writes this value and
/// `migrate` advances any stored value below it. Mirrors
/// governance_contract's `CURRENT_SCHEMA_VERSION` (issue #507, issue #704).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The stable set of capability flag names reported by `get_capabilities`
/// (issue #986).
///
/// Each name maps one-to-one to a feature this build actually implements:
///
/// | Flag                     | Backing feature |
/// |--------------------------|-----------------|
/// | `timelock`               | `schedule` / `execute` / `cancel` |
/// | `multisig_admin`         | threshold admin set (`change_threshold`, `transfer_admin`) |
/// | `admin_recovery`         | `initiate_recovery` / `execute_recovery` / `cancel_recovery` |
/// | `pause`                  | `pause` / `unpause` / `is_paused` |
/// | `wasm_upgrade`           | `upgrade`, guarded by the `supports_interface` probe |
/// | `schema_migration`       | `migrate` |
/// | `merchant_rules`         | per-merchant `set_settlement_rule` / `clear_settlement_rule` |
/// | `default_rule`           | global `set_default_rule` / `get_default_rule` |
/// | `governance_fee_ceiling` | fee rules validated against governance's `FeeConfig` |
/// | `fee_split_preview`      | `calculate_fee_split` |
/// | `batch_payment_read`     | `get_payments` |
/// | `payment_pruning`        | `prune_payment` |
///
/// The set is fixed at compile time: flags are never toggled at runtime, and
/// every listed flag is reported as `true`. Adding or removing a flag is a
/// deliberate API change and must update the capability test.
pub const CAPABILITY_FLAGS: [&str; 12] = [
    "timelock",
    "multisig_admin",
    "admin_recovery",
    "pause",
    "wasm_upgrade",
    "schema_migration",
    "merchant_rules",
    "default_rule",
    "governance_fee_ceiling",
    "fee_split_preview",
    "batch_payment_read",
    "payment_pruning",
];

/// 50k/100k per ADR-003: inactive instance entries evictable in days, not weeks.
///
/// Settlement-specific TTL policy for short-lived reads of admin / governance /
/// recovery addresses. Deliberately shorter than the protocol defaults so that
/// an inactive instance-side entry can still be evicted in days rather than
/// weeks — see ADR 003 for the rationale.
pub const READ_INSTANCE_TTL_THRESHOLD: u32 = 50_000;
pub const READ_INSTANCE_TTL_BUMP: u32 = 100_000;

// Used until the admin sets a global default settlement rule.
pub const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
    platform_fee_bps: 100,
    network_fee_bps: MIN_FEE_BPS,
    settlement_delay_ledger: 0,
    auto_settle: false,
};
