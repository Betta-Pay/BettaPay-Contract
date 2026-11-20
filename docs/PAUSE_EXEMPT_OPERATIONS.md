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
| `init()` | **Allowed** | One-time setup; pause state is initialized here | No |
| `register_merchant()` | **Blocked** | Business operation; merchants cannot be added while paused | Yes |
| `set_settlement_rule()` | **Blocked** | Business operation; merchant rules cannot change while paused | Yes |
| `store_payment_reference()` | **Blocked** | Business operation; no new payment references while paused | Yes |
| `schedule()` | **Blocked** | Timelocked operations cannot be created while paused | Yes |
| `execute()` | **Blocked** | Timelocked operations cannot execute while paused | Yes |
| `cancel()` | **Blocked** | Timelocked operations cannot be cancelled while paused | Yes |
| `cancel_all()` | **Blocked** | Timelocked batch cancellation is blocked while paused | Yes |
| `migrate()` | **Blocked** | Storage migration is blocked while paused | Yes |
| `pause()` | **Blocked on repeat** | First pause succeeds; pausing again panics with `AlreadyPaused` | No |
| `unpause()` | **Allowed** | Required to exit pause state | No |
| `transfer_admin()` | **Allowed** | Admin transfer must work during incident recovery | No |
| `initiate_recovery()` | **Allowed** | Recovery mode is the escape hatch for emergencies | No |
| `cancel_recovery()` | **Allowed** | Pending recovery can be cancelled during pause | No |
| `execute_recovery()` | **Allowed** | Recovery completes even if the contract is paused | No |
| `upgrade()` | **Allowed** | Critical security patches must be deployable during pause | No |
| `get_merchant()` | **Allowed** | Read-only; does not modify state | No |
| `get_payment_reference()` | **Allowed** | Read-only; does not modify state | No |
| `get_admin()` | **Allowed** | Read-only; always allowed | No |
| `is_paused()` | **Allowed** | Read-only; returns pause state | No |

---

## Pause Idempotency

The `pause()` and `unpause()` operations are **state-transition guards**:

- **`pause()` when already paused:** Panics with `AlreadyPaused` (error code 17); no duplicate event is emitted.
- **`unpause()` when already unpaused:** Panics with `AlreadyUnpaused` (error code 16); no duplicate event is emitted.

**Rationale:** Repeated pause/unpause transitions must be rejected so monitoring scripts cannot emit misleading duplicate events.

**Error Codes:**
- `Paused` (5) — Returned when a blocked operation is called while paused
- `AlreadyPaused` (17) — Returned when `pause()` is called while already paused
- `AlreadyUnpaused` (16) — Returned when `unpause()` is called while already unpaused

---

## Implementation: `assert_not_paused()`

The `assert_not_paused()` helper in `settlement_contract/src/storage.rs` is called at the **start** of each blocked entry point:

```rust
pub fn assert_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}
```

**Entry points that call this function:**
- `register_merchant()`
- `set_settlement_rule()`
- `store_payment_reference()`
- `schedule()`
- `execute()`
- `cancel()`
- `cancel_all()`
- `migrate()`

**Entry points that do NOT call this function** (allowed/exempt):
- `pause()`
- `unpause()`
- `transfer_admin()`
- `initiate_recovery()`
- `cancel_recovery()`
- `execute_recovery()`
- `upgrade()`
- All read-only getters

---

## Testing

The pause gating behavior is tested in `settlement_contract/src/tests/admin_tests.rs`, with timelock coverage in `settlement_contract/src/tests/timelock_tests.rs`:

### Coverage Requirements

1. **Blocked operations return `Paused` error** when called while paused
2. **Exempt operations succeed** when called while paused
3. **Pause state-transition guards:**
   - `pause()` while already paused panics with `AlreadyPaused`
   - `unpause()` while already unpaused panics with `AlreadyUnpaused`
4. **Unpause restores normal operation:**
   - Blocked operations succeed after `unpause()`

### Test Assertions

- `merchant_registration_blocked_when_paused`
- `paused_timelock_operations_are_rejected_and_queue_survives`
- `double_pause_is_rejected`
- `double_unpause_is_rejected`
- `pause_then_upgrade_then_unpause`
- `migrate_rejected_while_paused`

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
