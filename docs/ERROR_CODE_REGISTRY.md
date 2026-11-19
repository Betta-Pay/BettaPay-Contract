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
| 1 | `ALREADY_INITIALIZED` | The contract has already been initialized |
| 2 | `NOT_INITIALIZED` | The contract has not been initialized yet |
| 3 | `UNAUTHORIZED` | The caller is not authorized to perform this action |
| 4 | `INVALID_FEE_BPS` | The provided fee basis points are invalid or exceed the maximum limit |
| 5 | `PAUSED` | The contract is currently paused and the operation is not allowed |
| 6 | `INVALID_ADMIN` | The provided admin address is invalid (e.g., zero address or same as current admin) |
| 7 | `INVALID_RECOVERY_ADDRESS` | The provided recovery address is invalid |
| 8 | `RECOVERY_NOT_PENDING` | No admin recovery is currently pending |
| 9 | `RECOVERY_DELAY_ACTIVE` | The admin-recovery timelock has not yet elapsed |
| 10 | `EXECUTION_NOT_READY` | The scheduled operation is not yet ready for execution |
| 11 | `OPERATION_NOT_SCHEDULED` | The operation has not been scheduled |
| 12 | `OPERATION_ALREADY_SCHEDULED` | The operation has already been scheduled |
| 13 | `INVALID_WASM_INTERFACE` | The deployed WASM does not implement the required interface |
| 14 | `INVALID_THRESHOLD` | The provided multisig threshold is invalid |
| 15 | `RECOVERY_ALREADY_PENDING` | A recovery operation is already pending; a second one cannot be initiated |
| 16 | `ALREADY_UNPAUSED` | `unpause` was called while the contract was already unpaused |
| 17 | `ALREADY_PAUSED` | `pause` was called while the contract was already paused |
| 18 | `SAME_ADMIN` | The new admin set and threshold are identical to the current ones |
| 19-99 | — | **Reserved for future shared errors** |

**Note on code 16 and 17:** In `error_codes.rs`, `ALREADY_PAUSED` is 17 and `ALREADY_UNPAUSED` is 16. This ordering is intentional for historical reasons. New shared errors should use codes 19+.

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
| 300 | `MerchantExists` | `register_merchant` was called for an address that is already registered |
| 301 | — | **Reserved (unassigned historical gap; reservation removed, issue #881)** |
| 302 | `MerchantMissing` | The target merchant address is not registered |
| 303 | `DuplicatePaymentReference` | `store_payment_reference` was called with a 32-byte reference that already exists |
| 304 | `MerchantRuleNotSet` | No merchant-specific rule has been set; the default rule or bootstrap fallback applies |
| 305 | — | **Reserved (unassigned historical gap; reservation removed, issue #881)** |
| 306 | `ZeroAddress` | The supplied address is the zero-address |
| 307 | `InvalidPaymentReference` | `store_payment_reference` was called with an all-zero (reserved) 32-byte reference |
| 308 | `InvalidSettlementDelay` | `settlement_delay_ledger` exceeds `MAX_SETTLEMENT_DELAY_LEDGER` |
| 309 | `InvalidGovernance` | Governance contract configuration is invalid |
| 310 | `AmountOverflow` | Payment amount is large enough that multiplying by a fee's basis points would overflow `i128` |
| 311 | `GovernanceCallFailed` | Cross-contract governance call failed |
| 312 | `FeeExceedsGovernanceConfig` | Fee exceeds the governance-configured limit |
| 313 | `AmountTooSmall` | Payment amount is below the minimum |
| 314 | `BatchTooLarge` | Batch exceeds the maximum allowed size |
| 315 | `PaymentOrphaned` | Merchant's payment records are orphaned while the merchant is unregistered or its archive tombstone is present |
| 316 | `OperationHashCollision` | `schedule()` found a `sha256(operation)` key already holding a different pending operation, or `execute()`/`cancel()` was given an operation that does not match the stored one |
| 317 | `AdminSetEmpty` | Stored admin list was empty when resolving the primary admin |
| 318 | `PayloadTooLarge` | Scheduled operation's encoded XDR payload exceeds `MAX_SCHEDULED_OPERATION_PAYLOAD_LEN` (issue #993) |
| 319 | `ExecutionDelayTooLong` | `schedule()` `execute_in` delay exceeds `MAX_TIMELOCK_DELAY_SECONDS` (issue #992) |
| 320 | `InvalidPauseReason` | `pause()` reason code is above `MAX_PAUSE_REASON` (issue #991) |
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
