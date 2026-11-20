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
| 1 | `AlreadyInitialized` | Contract has already been initialized |
| 2 | `NotInitialized` | Contract has not been initialized |
| 3 | `Unauthorized` | Caller lacks required authorization |
| 4 | `InvalidFeeBps` | Fee basis points are invalid |
| 5 | `Paused` | Contract is paused; operation not allowed |
| 6 | `InvalidAdmin` | Proposed admin set or address is invalid |
| 7 | `InvalidRecoveryAddress` | Recovery address is invalid |
| 8 | `RecoveryNotPending` | No pending recovery operation exists |
| 9 | `RecoveryDelayActive` | Recovery timelock has not elapsed |
| 10 | `ExecutionNotReady` | Scheduled operation timelock has not elapsed |
| 11 | `OperationNotScheduled` | Scheduled operation does not exist |
| 12 | `OperationAlreadyScheduled` | Operation is already scheduled |
| 13 | `InvalidWasmInterface` | Deployed WASM interface is unsupported |
| 14 | `InvalidThreshold` | Admin threshold is invalid |
| 15 | `RecoveryAlreadyPending` | A recovery operation is already pending |
| 16 | `AlreadyUnpaused` | Contract is already unpaused |
| 17 | `AlreadyPaused` | Contract is already paused |
| 18 | `SameAdmin` | Proposed admin set and threshold are unchanged |
| 19-99 | — | **Reserved for future shared errors** |

**Note on code 16 and 17:** These idempotent pause state checks are intentionally ordered this way in the codebase for historical reasons. New shared errors should use codes 19+.

---

## Governance Contract Errors (200-299)

Defined in `governance_contract/src/lib.rs` as `GovernanceError`.

| Code | Name | Meaning |
|------|------|---------|
| 200 | `AnchorMissing` | Required anchor contract address not set |
| 201 | `InvalidParamValue` | Configuration parameter value out of acceptable range |
| 202 | — | **Reserved** |
| 203 | — | **Reserved** |
| 202-299 | — | **Reserved for future governance errors** |

---

## Settlement Contract Errors (300-399)

Defined in `settlement_contract/src/errors.rs` as `SettlementError`.

| Code | Name | Meaning |
|------|------|---------|
| 300 | `MerchantExists` | Merchant address is already registered |
| 301 | — | **Reserved (formerly allocated, now unused)** |
| 302 | `MerchantMissing` | Merchant address is not registered |
| 303 | `DuplicatePaymentReference` | Payment reference already exists |
| 304 | `MerchantRuleNotSet` | Merchant has no merchant-specific rule |
| 305 | — | **Reserved (formerly allocated, now unused)** |
| 306 | `ZeroAddress` | Supplied address is the zero-address sentinel |
| 307 | `InvalidPaymentReference` | Payment reference is invalid or reserved |
| 308 | `InvalidSettlementDelay` | Settlement delay exceeds the configured maximum |
| 309 | `InvalidGovernance` | Governance address or contract state is invalid |
| 310 | `AmountOverflow` | Fee calculation would overflow |
| 311 | `GovernanceCallFailed` | Cross-contract governance call returned error |
| 312 | `FeeExceedsGovernanceConfig` | Settlement fee exceeds governance limits |
| 313 | `AmountTooSmall` | Payment amount is below the minimum |
| 314 | `BatchTooLarge` | Cancellation batch exceeds the configured maximum |
| 315 | `PaymentOrphaned` | Payment records are orphaned by merchant removal |
| 316 | `OperationHashCollision` | Stored operation hash collided or payload mismatched |
| 317 | `AdminSetEmpty` | Stored admin list was empty when resolving primary admin |
| 318 | `PayloadTooLarge` | Scheduled operation payload exceeds the size limit |
| 319 | `ExecutionDelayTooLong` | Scheduled execution delay exceeds the maximum |
| 320 | `InvalidPauseReason` | Pause reason exceeds the maximum value |
| 321-399 | — | **Reserved for future settlement errors** |

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
