//! Canonical error-code registry shared by every BettaPay contract.
//!
//! Soroban's `#[contracterror]` macro requires each contract to define its
//! own error enum (it becomes part of that contract's WASM interface), so
//! this module cannot export a single shared enum. Instead it fixes the
//! *numeric* meaning of every code: each contract's error enum must use
//! these constants as explicit discriminants rather than picking its own
//! numbers. This is what makes a raw error code returned from either
//! contract unambiguous to an off-chain client without it needing to know
//! which contract produced it.
//!
//! Ranges:
//! - `1..=99`   — codes shared by two or more contracts (this module).
//! - `200..=299` — reserved for `governance_contract`-only codes.
//! - `300..=399` — reserved for `settlement_contract`-only codes.
//!
//! Adding a new contract-specific error: pick the next free number in that
//! contract's range. Adding a new *shared* concept: add a constant here in
//! the `1..=99` range and update every contract that needs it. Either way,
//! [`assert_no_code_collisions`] (exercised by both contracts' test suites)
//! will fail the build if two different concepts end up sharing a number.

/// The contract has already been initialized.
pub const ALREADY_INITIALIZED: u32 = 1;
/// The contract has not been initialized yet.
pub const NOT_INITIALIZED: u32 = 2;
/// The caller is not authorized to perform this action.
pub const UNAUTHORIZED: u32 = 3;
/// The provided fee basis points are invalid or exceed the maximum limit.
pub const INVALID_FEE_BPS: u32 = 4;
/// The contract is currently paused and the operation is not allowed.
pub const PAUSED: u32 = 5;
/// The provided admin address is invalid (e.g., zero address or same as current admin).
pub const INVALID_ADMIN: u32 = 6;
/// The provided recovery address is invalid.
pub const INVALID_RECOVERY_ADDRESS: u32 = 7;
/// No admin recovery is currently pending.
pub const RECOVERY_NOT_PENDING: u32 = 8;
/// The admin-recovery timelock has not yet elapsed.
pub const RECOVERY_DELAY_ACTIVE: u32 = 9;
/// The scheduled operation is not yet ready for execution.
pub const EXECUTION_NOT_READY: u32 = 10;
/// The operation has not been scheduled.
pub const OPERATION_NOT_SCHEDULED: u32 = 11;
/// The operation has already been scheduled.
pub const OPERATION_ALREADY_SCHEDULED: u32 = 12;
/// The deployed WASM does not implement the required interface.
pub const INVALID_WASM_INTERFACE: u32 = 13;
/// The provided multisig threshold is invalid.
pub const INVALID_THRESHOLD: u32 = 14;
/// A recovery operation is already pending; a second one cannot be initiated.
pub const RECOVERY_ALREADY_PENDING: u32 = 15;
/// `pause` was called while the contract was already paused.
pub const ALREADY_PAUSED: u32 = 17;
/// `unpause` was called while the contract was already unpaused.
pub const ALREADY_UNPAUSED: u32 = 16;

/// Lowest code reserved for `governance_contract`-only errors.
pub const GOVERNANCE_RANGE_START: u32 = 200;
/// Lowest code reserved for `settlement_contract`-only errors.
pub const SETTLEMENT_RANGE_START: u32 = 300;

/// All codes defined in the shared `1..=99` range, paired with their name,
/// for use by cross-contract conformity tests.
pub const SHARED_CODES: &[(&str, u32)] = &[
    ("AlreadyInitialized", ALREADY_INITIALIZED),
    ("NotInitialized", NOT_INITIALIZED),
    ("Unauthorized", UNAUTHORIZED),
    ("InvalidFeeBps", INVALID_FEE_BPS),
    ("Paused", PAUSED),
    ("InvalidAdmin", INVALID_ADMIN),
    ("InvalidRecoveryAddress", INVALID_RECOVERY_ADDRESS),
    ("RecoveryNotPending", RECOVERY_NOT_PENDING),
    ("RecoveryDelayActive", RECOVERY_DELAY_ACTIVE),
    ("ExecutionNotReady", EXECUTION_NOT_READY),
    ("OperationNotScheduled", OPERATION_NOT_SCHEDULED),
    ("OperationAlreadyScheduled", OPERATION_ALREADY_SCHEDULED),
    ("InvalidWasmInterface", INVALID_WASM_INTERFACE),
    ("InvalidThreshold", INVALID_THRESHOLD),
    ("RecoveryAlreadyPending", RECOVERY_ALREADY_PENDING),
    ("AlreadyPaused", ALREADY_PAUSED),
    ("AlreadyUnpaused", ALREADY_UNPAUSED),
];

/// Asserts that a contract's full `(name, code)` table is internally
/// consistent with the shared registry: every code it reuses from
/// [`SHARED_CODES`] must be attached to the matching name, and every code
/// outside the shared `1..=99` range must fall inside `range_start..`.
///
/// Intended to be called from each contract's own test suite with its full
/// error table, so a future edit that assigns a contract-specific error the
/// same number as a shared code (or vice versa) fails `cargo test --workspace`
/// instead of silently reintroducing issue #517.
pub fn assert_no_code_collisions(contract_codes: &[(&str, u32)], range_start: u32) {
    for &(name, code) in contract_codes {
        if let Some(&(shared_name, _)) = SHARED_CODES.iter().find(|&&(_, c)| c == code) {
            assert_eq!(
                name, shared_name,
                "error code {code} is reserved for shared concept `{shared_name}` \
                 but is also used here for `{name}`",
            );
        } else {
            assert!(
                code >= range_start,
                "error code {code} (`{name}`) is not a shared code and falls \
                 outside this contract's reserved range ({range_start}+)",
            );
        }
    }
}
