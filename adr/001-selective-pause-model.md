# ADR 001: Selective Pause Model

**Date:** 2026-07-26

**Status:** Accepted

## Context

When designing the pause mechanism for the BettaPay contracts, we needed to decide between a full freeze (halting all contract operations) and a selective pause (blocking only state-mutating operations while allowing read-only queries). A full freeze would have been simpler to implement but more disruptive to users and downstream integrators.

## Decision

We chose a **selective pause model** where:

- `pause()` and `unpause()` toggle a boolean flag stored in instance storage (`DataKey::Paused`).
- Only state-mutating operations are blocked when paused:
  - `register_merchant`, `unregister_merchant`
  - `set_settlement_rule`, `clear_settlement_rule`
  - `set_default_rule`
  - `store_payment_reference`
  - `update_governance`
- Read-only operations (`get_admin`, `is_merchant_registered`, `get_settlement_rule`, `get_default_rule`, `get_payment_reference`, `is_paused`, `calculate_fee_split`) remain accessible even when paused.
- Administrative operations (`upgrade`, `transfer_admin`, `initiate_recovery`, `cancel_recovery`, `execute_recovery`, `schedule`, `execute`, `cancel`) are intentionally NOT blocked to allow emergency fixes.

## Consequences

- ✅ Read-only integrations (e.g., frontends querying payment status) are not disrupted during an emergency pause.
- ✅ Admin upgrade and governance operations can still be queried externally.
- ❌ Slightly more complex than a global on/off switch.
- ⚠️ The pause flag lives in instance storage and is not archived, ensuring it survives regardless of TTL.
