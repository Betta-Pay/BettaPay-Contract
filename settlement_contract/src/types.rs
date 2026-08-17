use bettapay_common::constants::BPS_DENOMINATOR;
use soroban_sdk::{contracttype, Address, BytesN};

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


#[derive(Clone)]
#[contracttype]
pub struct FeeSplit {
    /// The total gross amount of the payment.
    /// Mirrors the `amount` parameter passed to `store_payment_reference`.
    pub gross_amount: i128,
    /// Portion of the settlement fee allocated to the platform.
    /// This amount is calculated by applying the platform fee basis points to the gross amount.
    pub platform_fee_amount: i128,
    /// Portion of the settlement fee allocated to the network.
    /// This amount is calculated by applying the network fee basis points to the gross amount.
    pub network_fee_amount: i128,
    /// Net amount allocated to the merchant.
    /// This derived output is calculated as the gross amount minus the rounded platform and network fee amounts.
    pub merchant_amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct PaymentRecord {
    /// The total gross amount of the payment processed.
    /// Set upon payment creation and used to derive the fee split.
    pub amount: i128,
    /// The exact amount deducted for the platform fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub platform_fee_amount: i128,
    /// The exact amount deducted for the network fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub network_fee_amount: i128,
    /// The net payout amount owed to the merchant.
    /// Calculated at payment creation to ensure deterministic settlement value.
    pub merchant_amount: i128,
    /// The platform fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub platform_fee_bps: u32,
    /// The network fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub network_fee_bps: u32,
    /// Ledger sequence timestamp when the payment was recorded.
    /// Used alongside settlement_delay_ledger to verify if the payment is ripe for settlement.
    pub ledger: u32,
    /// The delay period (in ledgers) before settlement can occur.
    /// Sourced from the active settlement rule and used to prevent premature settlement.
    pub settlement_delay_ledger: u32,
    /// Indicates if the payment should participate in automated settlement batches.
    /// Set from the active rule and used by external auto-settlement processes.
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

// Admin, RecoveryAddress, PendingRecovery, and Paused live in
// `bettapay_common::storage::CommonDataKey` instead of here - see that
// type's doc comment for why a shared key type is safe to mix with this
// contract's own storage without a migration.

#[derive(Clone)]
#[contracttype]
pub enum Operation {
    UpdateGovernance(Address),
    CancelRecovery,
    TransferAdmin(Vec<Address>, u32),
    Upgrade(BytesN<32>),
    RegisterMerchant(Address),
    UnregisterMerchant(Address),
    SetSettlementRule(Address, SettlementRule),
    ClearSettlementRule(Address),
    SetDefaultRule(SettlementRule),
}

#[derive(Clone)]
#[contracttype]
pub(crate) enum DataKey {
    /// Instance — singleton, read on every mutating call.
    Admin,
    /// Instance — singleton u32 threshold for multisig operations.
    Threshold,
    /// Instance — singleton address, rarely changes.
    RecoveryAddress,
    /// Instance — singleton boolean flag, read on every mutating call.
    PendingRecovery,
    /// Instance — singleton address, rarely changes.
    Governance,
    /// Persistent — one per merchant, many entries.
    Merchant(Address),
    /// Persistent — one per merchant, may expire.
    Rule(Address),
    /// Persistent — single value but may be updated.
    DefaultRule,
    /// Persistent — one per payment, high volume.
    Payment(BytesN<32>),
    /// Instance — singleton boolean, read on every mutating call.
    Paused,
    /// Storage key for a scheduled operation.
    ScheduledOperation(BytesN<32>),
}
