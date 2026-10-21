# BettaPay Error Code Registry

This document provides the canonical registry of all error codes used across BettaPay contracts. Error codes are returned as numeric discriminants in contract error responses and must remain stable across contract upgrades.

## Purpose

- **Prevent code collisions** between shared and contract-specific errors
- **Document unused ranges** to guide future error allocation
- **Provide a single source of truth** for error code interpretation by indexers and integrators

## Error Code Ranges

| Range | Owner | Purpose |
|-------|-------|---------|
| 1-99 | `bettapay_common` | Shared errors used by both contracts |
| 100-199 | Reserved | Future shared errors |
| 200-299 | `governance_contract` | Governance-specific errors |
| 300-399 | `settlement_contract` | Settlement-specific errors |
| 400+ | Reserved | Future contract-specific ranges |

---

## Shared Error Codes (1-99)

Defined in `bettapay_common/src/error_codes.rs` as `SHARED_CODES`.

| Code | Name | Meaning |
|------|------|---------|
| 1 | `Unauthorized` | Caller lacks required authorization |
| 2 | `AlreadyInitialized` | Contract cannot be re-initialized |
| 3 | `NotInitialized` | Contract must be initialized before use |
| 4 | `InvalidAmount` | Amount value is zero, negative, or exceeds limits |
| 5 | `InvalidAddress` | Address is malformed or zero-address sentinel |
| 6 | `InvalidFee` | Fee configuration violates constraints |
| 7 | `StorageFailure` | Persistent storage operation failed |
| 8 | `ArithmeticOverflow` | Numeric operation would overflow |
| 9 | `DivisionByZero` | Division or modulo by zero |
| 10 | `InvalidTimestamp` | Timestamp is in the past or too far in the future |
| 11 | `DeadlineExpired` | Operation deadline has passed |
| 12 | `InvalidSignature` | Cryptographic signature verification failed |
| 13 | `DuplicateEntry` | Entry already exists (merchant, schedule, etc.) |
| 14 | `EntryNotFound` | Requested entry does not exist |
| 15 | `Paused` | Contract is paused; operation not allowed |
| 16 | `AlreadyUnpaused` | Contract is already unpaused |
| 17 | `AlreadyPaused` | Contract is already paused |
| 18-99 | — | **Reserved for future shared errors** |

**Note on code 16 and 17:** These idempotent pause state checks are intentionally ordered this way in the codebase for historical reasons. New shared errors should use codes 18+.

---

## Governance Contract Errors (200-299)

Defined in `governance_contract/src/lib.rs` as `GovernanceError`.

| Code | Name | Meaning |
|------|------|---------|
| 200 | `AnchorMissing` | Required anchor contract address not set |
| 201 | `InvalidParamValue` | Configuration parameter value out of acceptable range |
| 202 | — | **Reserved** |
| 203 | — | **Reserved** |
| 204 | `SameAdmin` | New admin address is the same as the current admin |
| 205-299 | — | **Reserved for future governance errors** |

---

## Settlement Contract Errors (300-399)

Defined in `settlement_contract/src/errors.rs` as `SettlementError`.

| Code | Name | Meaning |
|------|------|---------|
| 300 | `MerchantNotFound` | Merchant address not registered |
| 301 | — | **Reserved (formerly allocated, now unused)** |
| 302 | `InvalidMerchantConfig` | Merchant configuration violates constraints |
| 303 | `PaymentNotFound` | Payment reference does not exist |
| 304 | `InvalidPaymentState` | Payment state transition not allowed |
| 305 | — | **Reserved (formerly allocated, now unused)** |
| 306 | `InsufficientBalance` | Payer balance too low for payment |
| 307 | `InvalidRecipient` | Recipient address invalid for payment |
| 308 | `FeeCalculationFailed` | Fee split computation failed |
| 309 | `GovernanceCallFailed` | Cross-contract governance call returned error |
| 310 | `ScheduleNotFound` | Scheduled operation does not exist |
| 311 | `ScheduleAlreadyExecuted` | Scheduled operation already executed |
| 312 | `ScheduleNotReady` | Scheduled operation execution time not reached |
| 313 | `RecoveryNotInitiated` | Recovery mode must be initiated before execution |
| 314 | `RecoveryAlreadyActive` | Recovery mode already in progress |
| 315 | `InvalidRecoveryState` | Recovery state transition not allowed |
| 316 | `UpgradeHashMismatch` | Upgrade WASM hash does not match scheduled hash |
| 317 | `AdminSetEmpty` | Stored admin list was empty when resolving primary admin |
| 318 | `ReferenceAlreadyUsed` | Payment reference already exists (duplicate) |
| 319-399 | — | **Reserved for future settlement errors** |

---

## Usage Guidelines

### For Contract Developers

1. **Always allocate from reserved ranges** when adding new errors.
2. **Never reuse or reassign** existing error codes.
3. **Update this registry** immediately when adding a new error variant.
4. **Run the registry coverage test** to detect missing table entries:
   ```bash
   cargo test error_code_registry
   ```

### For Integrators

- **Decode numeric error codes** using this registry when handling contract failures.
- **Do not assume gaps** in the code space are free — all reserved ranges are subject to future allocation.
- **Treat unknown codes** as unhandled errors and log them for investigation.

### For Indexers

- **Map raw error discriminants** to human-readable names using this table.
- **Flag sentinel patterns** such as the zero-address (`GAAAAAA...WHF`) in admin events.
- **Monitor for new codes** appearing in contract events that are not yet documented here.

---

## Collision Prevention

The `bettapay_common` module enforces compile-time collision detection via the `assert_no_code_collisions` test. If a shared code is inadvertently reused in a contract-specific error enum, the build will fail.

---

## Related Documentation

- [bettapay_common/src/error_codes.rs](../bettapay_common/src/error_codes.rs) — Shared error code constants
- [governance_contract/src/lib.rs](../governance_contract/src/lib.rs) — Governance error enum
- [settlement_contract/src/errors.rs](../settlement_contract/src/errors.rs) — Settlement error enum
- [ADR-001: Selective Pause Model](../adr/001-selective-pause-model.md) — Pause-related errors 15-17

---

**Last Updated:** October 2024  
**Maintained By:** BettaPay Core Team
