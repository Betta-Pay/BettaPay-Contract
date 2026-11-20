# ADR-005: Cross-Contract Fee Trust Boundary

## Status

Accepted

## Context

BettaPay's settlement contract performs cross-contract calls to the governance contract to fetch fee configuration rules via `get_fee_config()`. The governance contract is fully autonomous and controlled by its own admin set, making this a **trust boundary** where settlement must decide:

1. What aspects of the governance response to validate
2. What to do when governance is unavailable or returns malformed data
3. Whether to fail open (allow operations) or fail closed (reject operations) when validation fails

This trust model is security-relevant because:
- **Governance can set arbitrary fee values** within its own min/max constraints
- **Settlement cannot cryptographically verify** governance's internal state or fee calculation logic
- **Malformed responses** could cause settlement to miscalculate fees or reject valid payments

Without explicit documentation of what settlement trusts and validates, future changes to either contract could inadvertently violate assumptions and introduce vulnerabilities.

## Decision

### What Settlement Trusts

**Settlement fully trusts the governance contract** to:
- Set fee configurations within `MIN_FEE_BPS` (0) and `MAX_FEE_BPS` (10,000 basis points = 100%)
- Honor the `BPS_DENOMINATOR` of 10,000 when computing basis point percentages
- Return fee configs in the expected two-field shape: `(platform_fee_bps: u32, merchant_fee_bps: u32)`
- Not return malicious or nonsensical values that would cause arithmetic overflow or underflow

**Settlement does NOT verify:**
- Whether governance's internal fee rules are fair, reasonable, or consistent with policy
- Whether the governance admin set is legitimate or compromised
- Whether governance correctly enforces its own fee ceiling constraints

### What Settlement Validates

Settlement performs **shape validation only** in `classify_governance_fee_config()`:

```rust
fn classify_governance_fee_config(
    result: Result<(u32, u32), GovernanceError>
) -> GovernanceFeeClassification {
    match result {
        Ok((platform_bps, merchant_bps)) if is_valid_fee_shape(platform_bps, merchant_bps) => {
            GovernanceFeeClassification::Valid { platform_bps, merchant_bps }
        },
        Ok(_) => GovernanceFeeClassification::Malformed,
        Err(_) => GovernanceFeeClassification::CallFailed,
    }
}
```

**Validation checks:**
- Both `platform_fee_bps` and `merchant_fee_bps` are `<= MAX_FEE_BPS` (10,000)
- The sum `platform_fee_bps + merchant_fee_bps <= MAX_FEE_BPS` (total fees cannot exceed 100%)

**Settlement does NOT check:**
- Whether the fees match governance's internal policy or historical values
- Whether the governance contract itself is paused or in recovery mode
- Whether the fee values have been recently changed (no staleness detection)

### Failure Handling

Settlement uses a **dual-mode** failure strategy depending on the operation:

#### Read Path (Fail-Open)

When **reading** fee configuration for informational purposes (e.g., merchant dashboard queries), settlement falls back to `BOOTSTRAP_DEFAULT_RULE`:

```rust
pub fn read_governance_fee_rule(env: &Env) -> FeeRule {
    match classify_governance_fee_config(call_governance_get_fee_config(env)) {
        GovernanceFeeClassification::Valid { platform_bps, merchant_bps } => {
            FeeRule { platform_bps, merchant_bps }
        },
        _ => BOOTSTRAP_DEFAULT_RULE, // Fail open with safe default
    }
}
```

**Rationale:** An unavailable governance contract should not block read-only queries. The default rule (0% platform, 0% merchant) is safe for informational display.

#### Write Path (Fail-Closed)

When **validating a payment** that will move funds, settlement rejects the transaction with `GovernanceCallFailed`:

```rust
pub fn validate_fee_against_governance(env: &Env, proposed_fee_bps: u32) -> Result<(), SettlementError> {
    let classification = classify_governance_fee_config(call_governance_get_fee_config(env));
    
    match classification {
        GovernanceFeeClassification::Valid { platform_bps, merchant_bps } => {
            let total_bps = platform_bps + merchant_bps;
            if proposed_fee_bps <= total_bps {
                Ok(())
            } else {
                Err(SettlementError::InvalidFee)
            }
        },
        _ => Err(SettlementError::GovernanceCallFailed), // Fail closed
    }
}
```

**Rationale:** If governance is unreachable or returns malformed data during a payment, the payment must be rejected rather than processed with potentially incorrect fees.

### Bypass Mechanisms

Settlement provides two escape hatches when governance is unavailable:

1. **Circuit Breaker Flag:** The admin can set `bypass_gov_fees = true` in persistent storage, causing settlement to skip governance validation entirely and accept merchant-proposed fees (up to `MAX_FEE_BPS`). This is a **temporary emergency measure** for governance downtime.

2. **Cached Governance Rule:** Settlement caches the last successfully retrieved governance fee rule. If governance becomes unavailable, settlement can optionally fall back to the cached value (not currently implemented, reserved for issue #743).

**Security Implication:** The bypass flag gives the settlement admin unilateral control over fee validation, bypassing governance entirely. This is acceptable because:
- Settlement admin and governance admin are the same trusted party in the current deployment model
- The bypass is an opt-in admin action, not automatic
- Fee ceiling (`MAX_FEE_BPS`) is still enforced

## Consequences

### Positive

- **Clear trust boundary:** Integrators know that settlement does not audit governance's fee logic, only validates response shape
- **Fail-open reads:** Dashboards and analytics remain available even when governance is down
- **Fail-closed writes:** Payments are never processed with unverified fees
- **Emergency override:** Circuit breaker allows business continuity during governance incidents

### Negative

- **Governance can set any fee within ceiling:** Settlement has no policy enforcement beyond technical constraints
- **No staleness detection:** If governance is compromised and fee is changed maliciously, settlement will accept the new value immediately
- **Per-call latency:** Every payment incurs a cross-contract governance call (no caching in current implementation)
- **Bypass flag is powerful:** If the settlement admin is compromised, they can disable governance fee checks entirely

### Alternatives Considered

#### 1. Cryptographic Verification of Governance State

**Rejected.** Soroban does not provide cross-contract state proofs. Settlement would need to fetch the entire governance contract state and verify it locally, which is expensive and still requires trusting the governance contract's logic.

#### 2. Fail-Closed on All Paths

**Rejected.** Read-only queries should not be blocked by governance downtime. The fail-open default (0% fees) is safe for display purposes.

#### 3. Hard-Coded Fee Ceilings in Settlement

**Rejected.** This would duplicate fee policy across two contracts and create a governance vs. settlement conflict when the ceiling needs to change.

## References

- **Implementation:**
  - `settlement_contract/src/storage.rs` (`read_governance_fee_rule`, `validate_fee_against_governance`, `classify_governance_fee_config`)
  - `governance_contract/src/lib.rs` (`set_fee_config`, `get_fee_config`)
  - `bettapay_common/src/constants.rs` (`MIN_FEE_BPS`, `MAX_FEE_BPS`, `BOOTSTRAP_DEFAULT_RULE`)

- **Related Issues:**
  - #741 — Governance fee rule caching
  - #742 — Fee staleness detection
  - #743 — Cached fallback on governance downtime
  - #744 — Per-merchant fee override policies

- **Related ADRs:**
  - [ADR-002: Fee Split Model](./002-fee-split-model.md) — How fees are calculated and distributed

---

**Authors:** BettaPay Core Team  
**Date:** October 2024  
**Status:** Accepted
