# ADR 002: Bootstrap vs. Init Fee Defaults

**Date:** 2026-07-26

**Status:** Accepted

## Context

The settlement contract requires a default settlement rule to calculate fee splits for merchants. We needed to choose how and when this default is established: at initialization time (stored during `init()`), or lazily via a hardcoded bootstrap fallback that applies until an admin explicitly sets a global default via `set_default_rule()`.

## Decision

We chose a **bootstrap fallback approach**:

- A hardcoded `BOOTSTRAP_DEFAULT_RULE` constant is defined in the contract:
  ```rust
  const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
      platform_fee_bps: 100,  // 1%
      network_fee_bps: 5,   // MIN_FEE_BPS
      settlement_delay_ledger: 0,
      auto_settle: false,
  };
  ```
- The `init()` function does **not** require or store a default rule.
- When resolving the effective rule for a merchant, the contract checks, in order (the 4-tier fallback chain):
  1. Merchant-specific rule → stored per-merchant in persistent storage via `set_settlement_rule`
  2. Global default rule → stored globally in instance storage via `set_default_rule`
  3. Governance fee config → protocol fee ceilings fetched from governance contract (`read_governance_fee_rule`)
  4. Bootstrap fallback → hardcoded in contract code as `BOOTSTRAP_DEFAULT_RULE`
- Event emission semantics:
  - Mutating payment path: `store_payment_reference` emits a `bootstrap_fallback` event when the resolved rule equals `BOOTSTRAP_DEFAULT_RULE`, alerting operators and indexers of an unconfigured deployment.
  - Read-only paths: `calculate_fee_split` and `get_effective_rule` are pure queries and never emit `bootstrap_fallback`.
  - Administrative clearing: `clear_settlement_rule` and `unregister_merchant` resolve fallback via `read_fallback_rule` and do not emit a duplicate bootstrap event.
- See [`docs/bootstrap-fallback.md`](../docs/bootstrap-fallback.md) for full specification and semantics.

## Consequences

- ✅ Deployments with zero configuration: a contract can be initialized and immediately start processing payments without requiring a `set_default_rule` call.
- ✅ The admin can postpone configuring the default rule; the 1% platform fee (100 bps) is a reasonable production default.
- ✅ Avoids an extra `init()` parameter, keeping the initialization interface simple.
- ❌ If the desired default differs from 100 bps / 5 bps, a separate `set_default_rule()` call is needed after init.
- ⚠️ The `bootstrap_fallback` event must be monitored in production to detect when the fallback is triggered, as it may indicate a configuration gap.
