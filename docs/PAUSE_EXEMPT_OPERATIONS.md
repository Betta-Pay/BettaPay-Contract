# Settlement Contract Pause-Exempt Operations

This document enumerates every settlement contract entry point and its behavior when the contract is paused, providing a machine-checkable matrix of pause gating.

## Purpose

The settlement contract implements a **selective pause model** (documented in [ADR-001](../adr/001-selective-pause-model.md)) where:
- **Critical admin operations remain available** during pause (upgrade, recovery, admin transfer)
- **Business operations are blocked** (payments, merchant registration)
- **Idempotent state transitions** (pause/unpause) are always allowed

This table ensures the pause model is correctly implemented and cannot drift from documentation.

---

## Pause Behavior Matrix

| Entry Point | Paused Behavior | Rationale | `assert_not_paused()` Called |
|-------------|-----------------|-----------|------------------------------|
| `initialize()` | **Allowed** | One-time setup; pause state is initialized here | No |
| `register_merchant()` | **Blocked** | Business operation; merchants cannot be added while paused | Yes |
| `update_merchant_config()` | **Blocked** | Business operation; config changes blocked during pause | Yes |
| `create_payment()` | **Blocked** | Business operation; no new payments during incident response | Yes |
| `execute_payment()` | **Blocked** | Business operation; no fund movement during pause | Yes |
| `refund_payment()` | **Blocked** | Business operation; refunds require investigation during pause | Yes |
| `schedule_operation()` | **Exempt** | Admin may schedule recovery actions during pause | No |
| `execute_scheduled_operation()` | **Exempt** | Scheduled operations (like upgrade) must execute even if paused | No |
| `cancel_scheduled_operation()` | **Exempt** | Admin may cancel bad schedules during incident | No |
| `pause()` | **Allowed** | Idempotent; calling pause while paused is a no-op | No |
| `unpause()` | **Allowed** | Required to exit pause state | No |
| `transfer_admin()` | **Exempt** | Admin transfer must work during incident recovery | No |
| `initiate_recovery()` | **Exempt** | Recovery mode is the escape hatch for emergencies | No |
| `execute_recovery()` | **Exempt** | Recovery completes even if contract is paused | No |
| `upgrade()` | **Exempt** | Critical security patches must be deployable during pause | No |
| `get_merchant()` | **Allowed** | Read-only; does not modify state | No |
| `get_payment()` | **Allowed** | Read-only; does not modify state | No |
| `get_admin()` | **Allowed** | Read-only; always allowed | No |
| `is_paused()` | **Allowed** | Read-only; returns pause state | No |

---

## Pause Idempotency

The `pause()` and `unpause()` operations are **idempotent state transitions**:

- **`pause()` when already paused:** Returns success immediately without re-emitting the `contract_paused` event (idempotent no-op).
- **`unpause()` when already unpaused:** Returns error `AlreadyUnpaused` (error code 16), indicating the operation was unnecessary.

**Rationale:** Pause/unpause are typically called by automated monitoring scripts. Idempotency prevents double-pause bugs and allows retry-safe operations.

**Error Codes:**
- `Paused` (15) — Returned when a blocked operation is called while paused
- `AlreadyPaused` (17) — Returned when `pause()` is called while already paused
- `AlreadyUnpaused` (16) — Returned when `unpause()` is called while already unpaused

---

## Implementation: `assert_not_paused()`

The `assert_not_paused()` helper in `settlement_contract/src/storage.rs` is called at the **start** of each blocked entry point:

```rust
pub fn assert_not_paused(env: &Env) -> Result<(), SettlementError> {
    if is_paused(env) {
        return Err(SettlementError::Paused);
    }
    Ok(())
}
```

**Entry points that call this function:**
- `register_merchant()`
- `update_merchant_config()`
- `create_payment()`
- `execute_payment()`
- `refund_payment()`

**Entry points that do NOT call this function** (exempt):
- `schedule_operation()`
- `execute_scheduled_operation()`
- `cancel_scheduled_operation()`
- `transfer_admin()`
- `initiate_recovery()`
- `execute_recovery()`
- `upgrade()`
- All read-only getters

---

## Testing

The pause gating behavior is tested in `settlement_contract/src/tests/admin_tests.rs`:

### Coverage Requirements

1. **Blocked operations return `Paused` error** when called while paused
2. **Exempt operations succeed** when called while paused
3. **Pause idempotency:**
   - `pause()` → `pause()` returns success without re-emitting event
   - `unpause()` → `unpause()` returns `AlreadyUnpaused` error
4. **Unpause restores normal operation:**
   - Blocked operations succeed after `unpause()`

### Test Assertions

```rust
#[test]
fn test_payment_blocked_when_paused() {
    let (env, contract) = setup_contract();
    contract.pause();
    
    let result = contract.create_payment(...);
    assert_eq!(result.unwrap_err(), SettlementError::Paused);
}

#[test]
fn test_recovery_exempt_when_paused() {
    let (env, contract) = setup_contract();
    contract.pause();
    
    let result = contract.initiate_recovery(...);
    assert!(result.is_ok()); // Recovery works even when paused
}

#[test]
fn test_pause_idempotency() {
    let (env, contract) = setup_contract();
    contract.pause();
    
    let result = contract.pause();
    assert!(result.is_ok()); // Second pause is idempotent
    
    // Event should only be emitted once
    assert_eq!(count_pause_events(&env), 1);
}
```

---

## Governance Contract Pause Behavior

The **governance contract** has its own independent pause mechanism with a similar selective model. The governance pause matrix is documented separately in the governance contract's documentation.

**Key Difference:** Governance and settlement pause states are **independent**. A paused settlement does not pause governance, and vice versa. This allows governance fee updates during settlement incidents.

---

## Related Documentation

- [ADR-001: Selective Pause Model](../adr/001-selective-pause-model.md) — Rationale for selective pause design
- [settlement_contract/src/lib.rs](../settlement_contract/src/lib.rs) — Entry point implementations
- [settlement_contract/src/storage.rs](../settlement_contract/src/storage.rs) — `assert_not_paused()` implementation
- [settlement_contract/src/tests/admin_tests.rs](../settlement_contract/src/tests/admin_tests.rs) — Pause gating tests
- [ERROR_CODE_REGISTRY.md](./ERROR_CODE_REGISTRY.md) — Error codes 15, 16, 17

---

**Last Updated:** October 2024  
**Maintained By:** BettaPay Core Team
