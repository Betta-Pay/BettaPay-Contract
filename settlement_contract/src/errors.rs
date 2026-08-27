use bettapay_common::error_codes;
use soroban_sdk::contracterror;

// Discriminants below are pinned to `bettapay_common::error_codes` so that a
// numeric error code means the same thing in both contracts (issue #517).
// Shared concepts use the registry's constant value directly; codes with no
// governance_contract equivalent are contract-specific and live in the
// `300..=399` range reserved for this contract. `settlement_error_codes_match_registry`
// below fails the build if these literals ever drift from the registry.
#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementError {
    /// `init()` has already been called. Only one initialization is permitted.
    AlreadyInitialized = 1,
    /// `init()` has not been called. All admin-guarded functions require prior initialization.
    NotInitialized = 2,
    /// The caller does not match the stored admin address.
    Unauthorized = 3,
    /// Either fee BPS value is below `MIN_FEE_BPS` (5) or above `MAX_FEE_BPS`
    /// (5 000), or their sum exceeds `BPS_DENOMINATOR` (10 000).
    /// Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidFeeBps = 4,
    /// The contract is paused. Most state‑mutating operations are blocked.
    Paused = 5,
    /// `transfer_admin` was called with the current admin address as the
    /// new admin. The new admin must be different.
    InvalidAdmin = 6,
    InvalidRecoveryAddress = 7,
    RecoveryNotPending = 8,
    RecoveryDelayActive = 9,
    /// The scheduled operation is not yet ready for execution.
    ExecutionNotReady = 10,
    /// The operation has not been scheduled.
    OperationNotScheduled = 11,
    /// The operation has already been scheduled.
    OperationAlreadyScheduled = 12,
    /// The deployed WASM does not implement the required interface.
    InvalidWasmInterface = 13,
    /// The provided multisig threshold is invalid.
    InvalidThreshold = 14,
    /// A recovery is already pending; initiate_recovery cannot overwrite it.
    RecoveryAlreadyPending = 15,
    /// `register_merchant` was called for an address that is already registered.
    MerchantExists = 300,
    /// The target merchant address is not registered. Raised by
    /// `set_settlement_rule`, `store_payment_reference`, `calculate_fee_split`,
    /// and `unregister_merchant` when the merchant is missing.
    MerchantMissing = 302,
    /// `store_payment_reference` was called with a 32‑byte reference that
    /// already exists in storage.
    DuplicatePaymentReference = 303,
    /// No merchant-specific rule has been set. The merchant will use the default rule or bootstrap fallback.
    MerchantRuleNotSet = 304,
    /// The supplied address is an empty string.
    /// Raised by `register_merchant` and `transfer_admin`.
    EmptyAddress = 305,
    /// The supplied address is the zero‑address.
    /// Raised by `register_merchant` and `transfer_admin`.
    ZeroAddress = 306,
    /// `store_payment_reference` was called with an all‑zero 32‑byte
    /// reference, which is reserved.
    InvalidPaymentReference = 307,
    /// `settlement_delay_ledger` exceeds `MAX_SETTLEMENT_DELAY_LEDGER`
    /// (100 000). Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidSettlementDelay = 308,
    InvalidGovernance = 309,
    /// The payment amount is large enough that multiplying it by a fee's
    /// basis points would overflow `i128`. Raised by `calculate_split`
    /// before the multiplication is attempted.
    AmountOverflow = 310,
    GovernanceCallFailed = 311,
    FeeExceedsGovernanceConfig = 312,
    AmountTooSmall = 313,
    BatchTooLarge = 314,
    /// The merchant's payment records are orphaned: the merchant was
    /// unregistered (or never registered), so its payment history is no
    /// longer readable even if the merchant is later re-registered.
    /// Raised by `get_payment_reference` and `get_payments`.
    PaymentOrphaned = 315,
    /// `schedule()` computed a `sha256(operation)` key that already holds a
    /// *different* pending operation's data — an actual hash collision
    /// rather than a duplicate schedule of the same operation. Also raised
    /// by `execute()`/`cancel()` if the operation supplied does not
    /// byte-for-byte match the operation stored under that hash.
    OperationHashCollision = 316,
}

const _: () = {
    assert!(SettlementError::AlreadyInitialized as u32 == error_codes::ALREADY_INITIALIZED);
    assert!(SettlementError::NotInitialized as u32 == error_codes::NOT_INITIALIZED);
    assert!(SettlementError::Unauthorized as u32 == error_codes::UNAUTHORIZED);
    assert!(SettlementError::InvalidFeeBps as u32 == error_codes::INVALID_FEE_BPS);
    assert!(SettlementError::Paused as u32 == error_codes::PAUSED);
    assert!(SettlementError::InvalidAdmin as u32 == error_codes::INVALID_ADMIN);
    assert!(
        SettlementError::InvalidRecoveryAddress as u32 == error_codes::INVALID_RECOVERY_ADDRESS
    );
    assert!(SettlementError::RecoveryNotPending as u32 == error_codes::RECOVERY_NOT_PENDING);
    assert!(SettlementError::RecoveryDelayActive as u32 == error_codes::RECOVERY_DELAY_ACTIVE);
    assert!(SettlementError::ExecutionNotReady as u32 == error_codes::EXECUTION_NOT_READY);
    assert!(SettlementError::OperationNotScheduled as u32 == error_codes::OPERATION_NOT_SCHEDULED);
    assert!(
        SettlementError::OperationAlreadyScheduled as u32
            == error_codes::OPERATION_ALREADY_SCHEDULED
    );
    assert!(SettlementError::InvalidWasmInterface as u32 == error_codes::INVALID_WASM_INTERFACE);
    assert!(SettlementError::InvalidThreshold as u32 == error_codes::INVALID_THRESHOLD);
    assert!(
        SettlementError::RecoveryAlreadyPending as u32 == error_codes::RECOVERY_ALREADY_PENDING
    );
    assert!(SettlementError::MerchantExists as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::MerchantMissing as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(
        SettlementError::DuplicatePaymentReference as u32 >= error_codes::SETTLEMENT_RANGE_START
    );
    assert!(SettlementError::MerchantRuleNotSet as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::EmptyAddress as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::ZeroAddress as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::InvalidPaymentReference as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::InvalidSettlementDelay as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::InvalidGovernance as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::AmountOverflow as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::GovernanceCallFailed as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(
        SettlementError::FeeExceedsGovernanceConfig as u32 >= error_codes::SETTLEMENT_RANGE_START
    );
    assert!(SettlementError::AmountTooSmall as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::BatchTooLarge as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::PaymentOrphaned as u32 >= error_codes::SETTLEMENT_RANGE_START);
    assert!(SettlementError::OperationHashCollision as u32 >= error_codes::SETTLEMENT_RANGE_START);
};
