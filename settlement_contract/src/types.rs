use soroban_sdk::{contracttype, Address, Bytes, BytesN, Vec};

// `Bps` and `SettlementRule` are defined in `bettapay_common::types` (moved
// there so the shared event builders can take a typed payload, see issue
// #491) and re-exported here so every existing `crate::types::*` import in
// this contract keeps working unchanged.
pub use bettapay_common::types::{Bps, SettlementRule};

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
    /// The merchant the payment belongs to.
    ///
    /// Payments are keyed by `(merchant, reference)` (see [`DataKey::Payment`])
    /// and this field makes the ownership explicit on every record so callers
    /// and indexers never have to infer it from the storage key.
    pub merchant: Address,
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

/// The fee configuration schema as returned by the governance contract.
///
/// This type exists solely for decoding cross-contract calls from governance.
/// It is never written to settlement's own storage.
#[derive(Clone)]
#[contracttype]
pub struct GovFeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

/// Storage value for a pending [`Operation`] scheduled via `schedule()`.
///
/// The `sha256` of the operation's XDR encoding is used as the storage key
/// (see `DataKey::ScheduledOperation`) purely as a fixed-size index — it is
/// not trusted as the sole proof of what was scheduled. The full
/// `operation_xdr` is kept alongside `execute_at` so `execute()`/`cancel()`
/// can verify the operation they were given is byte-for-byte the one that
/// was scheduled under that hash, rather than assuming a hash match implies
/// content equality (issue #570: a hash collision must not let an
/// unscheduled operation silently ride an unrelated pending slot).
#[derive(Clone)]
#[contracttype]
pub struct ScheduledOp {
    pub operation_xdr: Bytes,
    pub execute_at: u64,
}

// RecoveryAddress, PendingRecovery, Paused, and Threshold live in
// `bettapay_common::storage::CommonDataKey` instead of here - see that
// type's doc comment for why a shared key type is safe to mix with this
// contract's own storage without a migration, and for what "storage
// separation" means for a key type shared this way. Admin stays local:
// this contract stores it as a multisig `Vec<Address>` under its own
// `DataKey::Admin`, which CommonDataKey does not own.

#[derive(Clone)]
#[contracttype]
pub enum Operation {
    UpdateGovernance(Address),
    CancelRecovery,
    /// Carries the full new admin set and multisig threshold, matching the
    /// shape of the direct `transfer_admin(signers, new_admins, new_threshold)`
    /// entry point exactly. Both paths now accept the same data, closing the
    /// storage-corruption window where the timelocked path could only carry a
    /// single address.
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
    /// Instance — singleton address, rarely changes.
    Governance,
    /// Persistent — one per merchant, many entries.
    Merchant(Address),
    /// Persistent — one per merchant, may expire.
    Rule(Address),
    /// Persistent — tombstone written when a merchant is unregistered.
    ///
    /// Survives re-registration so a merchant can never resurrect the payment
    /// history of an earlier registration (issue #490).
    ArchivedMerchant(Address),
    /// Persistent — single value but may be updated.
    DefaultRule,
    /// Persistent — one per (merchant, reference), high volume.
    ///
    /// Reference uniqueness is scoped to the merchant (issue #493): the same
    /// 32-byte reference may be used by two different merchants, so the key
    /// carries the merchant alongside the reference.
    Payment(Address, BytesN<32>),
    /// Storage key for a scheduled operation.
    ScheduledOperation(BytesN<32>),
}
