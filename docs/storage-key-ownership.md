# Storage Key Ownership and TTL Policies

This document serves as the authoritative specification of storage key ownership, storage kinds (persistent vs. instance), TTL policies, and key-sharing conventions across the BettaPay contract ecosystem (`bettapay_common`, `settlement_contract`, and `governance_contract`).

---

## 1. Master Storage Key Ownership Table

The following master table maps every key variant to its owning crate, storage kind, underlying data payload, and TTL management policy:

| Storage Key Variant | Owning Crate | Storage Kind | Payload Type | TTL Threshold / Bump | Sharing & Ownership Notes |
|---|---|---|---|---|---|
| **`CommonDataKey::RecoveryAddress`** | `bettapay_common` | Instance | `Address` | Instance TTL (`50k` / `100k` ledgers) | **Shared**: Defined in `bettapay_common`. Used identically by both contracts. |
| **`CommonDataKey::PendingRecovery`** | `bettapay_common` | Instance | `PendingRecovery` | Instance TTL | **Shared**: Defined in `bettapay_common`. Set during recovery; acts as timelock veto in settlement. |
| **`CommonDataKey::Paused`** | `bettapay_common` | Instance | `bool` | Instance TTL | **Shared**: Defined in `bettapay_common`. Controls emergency pause state in both contracts. |
| **`CommonDataKey::Threshold`** | `bettapay_common` | Instance | `u32` | Instance TTL (`50k` / `100k` ledgers) | **Shared**: Defined in `bettapay_common`. Multisig threshold for admin signatures. |
| **`settlement_contract::DataKey::Admin`** | `settlement_contract` | Instance | `Vec<Address>` | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local**: Multisig admin set for settlement contract only. |
| **`settlement_contract::DataKey::Initializing`** | `settlement_contract` | Instance | `()` (sentinel) | Ephemeral | **Contract-Local / Settlement-Only**: Written during `init()` to block reentrancy; removed upon completion. |
| **`settlement_contract::DataKey::Governance`** | `settlement_contract` | Instance | `Address` | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local / Settlement-Only**: Address of the trusted governance contract. |
| **`settlement_contract::DataKey::Deployer`** | `settlement_contract` | Instance | `Address` | Instance TTL | **Contract-Local**: Front-running protection marker recorded at contract deployment. |
| **`settlement_contract::DataKey::SchemaVersion`** | `settlement_contract` | Instance | `u32` | Instance TTL | **Contract-Local**: Storage schema version tracking for settlement contract migrations. |
| **`settlement_contract::DataKey::CachedGovRule`** | `settlement_contract` | Instance | `Option<SettlementRule>` | Instance TTL | **Contract-Local / Settlement-Only**: Cached governance fee rule to avoid redundant cross-contract calls. |
| **`settlement_contract::DataKey::DefaultRule`** | `settlement_contract` | Instance | `SettlementRule` | Instance TTL | **Contract-Local / Settlement-Only**: Global default settlement rule set by settlement admin. |
| **`settlement_contract::DataKey::Merchant(Address)`** | `settlement_contract` | Persistent | `bool` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Merchant registration record. Extended only on authenticated writes. |
| **`settlement_contract::DataKey::Rule(Address)`** | `settlement_contract` | Persistent | `SettlementRule` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Merchant-specific rule override. Extended on rule writes and authenticated reads. |
| **`settlement_contract::DataKey::ArchivedMerchant(Address)`** | `settlement_contract` | Persistent | `bool` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Tombstone written on unregister to orphan payment history. |
| **`settlement_contract::DataKey::Payment(Address, BytesN<32>)`** | `settlement_contract` | Persistent | `PaymentRecord` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Payment record scoped by merchant and 32-byte reference. |
| **`settlement_contract::DataKey::ScheduledOperation(BytesN<32>)`** | `settlement_contract` | Persistent | `ScheduledOp` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Settlement-Only**: Pending timelocked operation payload and `execute_at` timestamp. |
| **`settlement_contract::DataKey::PendingOperationCount`** | `settlement_contract` | Instance | `u32` | Instance TTL | **Contract-Local / Settlement-Only**: Queue-depth metric for pending timelocked operations. Incremented by `schedule`; decremented by `execute`, `cancel`, and `cancel_all`. Absence means zero. |
| **`governance_contract::DataKey::Admin`** | `governance_contract` | Instance | `Vec<Address>` | Instance TTL (`50k` / `100k` ledgers) | **Contract-Local**: Multisig admin set for governance contract only. |
| **`governance_contract::DataKey::FeeConfig`** | `governance_contract` | Instance | `FeeConfig` | Instance TTL | **Contract-Local / Governance-Only**: Protocol fee ceilings (`platform_fee_bps`, `network_fee_bps`). |
| **`governance_contract::DataKey::SchemaVersion`** | `governance_contract` | Instance | `u32` | Instance TTL | **Contract-Local**: Storage schema version tracking for governance contract migrations. |
| **`governance_contract::DataKey::Deployer`** | `governance_contract` | Instance | `Address` | Instance TTL | **Contract-Local**: Deployer address recorded at `init()` for governance front-running defense. |
| **`governance_contract::DataKey::Anchor(Address)`** | `governance_contract` | Persistent | `Address` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Governance-Only**: Trusted off-chain anchor address for an asset. Extended on read/write. |
| **`governance_contract::DataKey::SystemParam(Symbol)`** | `governance_contract` | Persistent | `i128` | `14` days / `30` days (`241,920` / `518,400` ledgers) | **Contract-Local / Governance-Only**: Dynamic protocol configuration knobs (e.g. `bypass_gov_fees`). Extended on read/write. |

---

## 2. Key Segregation and Sharing Principles

### 1. Shared Keys via `CommonDataKey`
The keys `RecoveryAddress`, `PendingRecovery`, `Paused`, and `Threshold` are declared once in `bettapay_common::storage::CommonDataKey`.
- **SCVal Symbol Encoding**: In Soroban, enum variants serialize by variant name only (`SCVal::Symbol`), ignoring the parent enum name. Thus, `CommonDataKey` entries are binary-compatible across contracts without requiring migrations.
- **Shared Semantics**: Any logic operating on these keys behaves identically in both contracts.

### 2. Contract-Local `Admin` Keys
`Admin` is intentionally **not** shared via `CommonDataKey`. Each contract declares `Admin` inside its own private `DataKey` enum:
- Although both contracts store a multisig set (`Vec<Address>`), keeping `Admin` local ensures strict autonomy and independent administration of each contract instance.

### 3. Independently Declared Local Keys
- **`SchemaVersion`**: Declared locally in both contracts to allow independent schema versioning and lifecycle migrations.
- **`Deployer`**: Declared locally in both contracts to gate initialization front-running prevention independently.

### 4. Settlement-Only Keys
The settlement contract possesses several keys unique to payment processing:
- `CachedGovRule`: Minimizes cross-contract call overhead by caching valid fee configs.
- `DefaultRule`: Singleton default rule stored in instance storage.
- `Merchant`, `Rule`, `ArchivedMerchant`, `Payment`: High-volume merchant and payment tracking records.
- `ScheduledOperation`: Fixed-size SHA-256 keyed queue for administrative timelocks.
- `PendingOperationCount`: Instance-storage counter mirroring the number of `ScheduledOperation` entries, used as the queue-depth metric (issue #887).
- `Initializing` & `Governance`: Ephemeral init guard and governance address reference.

### 5. Governance-Only Keys
- `FeeConfig`: Global protocol fee caps.
- `Anchor(Address)`: Registry of off-chain asset anchors.
- `SystemParam(Symbol)`: Extensible numerical parameter knobs.

---

## 3. Persistent Storage TTL Policy Summary

Persistent storage entries require proactive TTL management:
- **Standard Threshold**: `TTL_THRESHOLD_LEDGERS` = $17,280 \times 14 = 241,920$ ledgers (~14 days).
- **Standard Bump**: `TTL_BUMP_LEDGERS` = $17,280 \times 30 = 518,400$ ledgers (~30 days).
- **Enforcement**:
  - `Payment`: Extended on creation and authenticated reads.
  - `Merchant` / `ArchivedMerchant`: Extended on merchant state transitions.
  - `Rule`: Extended on rule writes and authenticated reads; unauthenticated queries (`calculate_fee_split`, `get_effective_rule`) are strictly TTL-neutral.
  - `ScheduledOperation`: Extended on `schedule()`; removed on `execute()` or `cancel()`.
  - `Anchor` / `SystemParam`: Extended on reads and writes.

Instance storage keys are tied to the contract instance lifecycle and do not expire independently as long as the contract instance remains active. `PendingOperationCount` therefore needs no separate TTL bump; it is kept in sync by `schedule`, `execute`, `cancel`, and `cancel_all`.
