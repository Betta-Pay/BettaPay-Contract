# ADR-004: Zero-Address Sentinel

## Status

Accepted

## Context

BettaPay contracts need to emit admin-related events (`admin_transferred`, `recovery_executed`) even when the admin set is missing or corrupt. Returning `None` or omitting the admin field would break event schema compatibility with indexers expecting a Stellar address.

The Stellar zero-address `GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF` (56 A's + WHF) serves as a **sentinel value** representing "admin unknown/not set" in specific contexts.

### Problem

1. **Event schemas require addresses:** Event topics and payloads must be valid Stellar addresses, not `Option<Address>`.
2. **Corrupt storage must not abort operations:** If the admin set becomes corrupted (storage read fails), recovery operations must still proceed and emit events.
3. **Indexers need explicit signal:** Indexers must distinguish "admin is the zero-address user" (impossible in practice) from "admin is not known at this time."

Without a documented sentinel convention, integrators could:
- Mistake the zero-address for a real admin
- Attempt to send transactions to the zero-address
- Fail to detect missing admin scenarios in operational dashboards

## Decision

### Sentinel Value

The zero-address Stellar strkey:

```
GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF
```

is used **exclusively** as a sentinel meaning **"admin not available"** in event payloads.

### Where It Appears

The sentinel is returned **only** by `read_optional_primary_admin()` in two specific scenarios:

1. **Settlement Contract** (`settlement_contract/src/storage.rs`):
   - When building `admin_transferred` events during `transfer_admin()`
   - When building `recovery_executed` events during `execute_recovery()`

2. **Governance Contract** (`governance_contract/src/lib.rs`):
   - When building `admin_transferred` events during `transfer_admin()`
   - When building `recovery_executed` events during `execute_recovery()`

**Implementation:**

```rust
pub fn read_optional_primary_admin(env: &Env) -> Address {
    match env.storage().persistent().get(&DataKey::PrimaryAdmin) {
        Some(admin) => admin,
        None => {
            // Return sentinel instead of panicking
            Address::from_string(&String::from_str(
                env,
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
            ))
        }
    }
}
```

### Where It Does NOT Appear

The zero-address sentinel is **never returned** by:

- `get_admin()` — Public admin getter. Panics if admin not set (fail-fast).
- `require_admin()` — Authorization check. Panics if admin missing.
- `validate_admin_signature()` — Signature verification. Rejects zero-address.
- Any merchant registration or payment authorization logic.

### Rationale: Default-Deny Recovery

During recovery operations, the system must:
1. **Continue operating** even if the admin set is corrupt or missing
2. **Emit a complete event** with all required fields
3. **Not grant admin privileges** to the sentinel address

The zero-address satisfies all three requirements:
- **No valid private key exists** for the zero-address, so it cannot sign transactions
- **Indexers see a valid event** with a recognizable sentinel value
- **Admin checks fail** if an attacker tries to use the sentinel as an admin

This is a **default-deny** security posture: when admin state is unknown, the system emits a safe placeholder rather than aborting or granting unauthorized access.

## Consequences

### Positive

- **Recovery operations never abort** due to corrupt admin storage
- **Event schemas remain consistent** — no `Option<Address>` needed
- **Sentinel is self-describing** — the zero-address is invalid for real operations, making accidental misuse unlikely
- **Indexers can detect missing admin** by checking for the sentinel value

### Negative

- **Sentinel must be documented** — undocumented use could confuse integrators
- **Not type-safe** — Rust cannot distinguish sentinel from real address at compile time
- **Storage corruption creates silent placeholder** — recovery events will not show the real admin if storage is corrupt

### Interpretation Guidelines

#### For Contract Callers

- **Never use the zero-address** in transaction signatures or admin operations.
- **Treat sentinel in events as "admin unknown"**, not "admin is the zero-address user."
- **If you see the sentinel** in a `recovery_executed` event, the admin set was missing or corrupt at the time of recovery.

#### For Indexers

- **Flag the sentinel** in dashboards as "Admin Not Available" rather than displaying the address.
- **Do not count the sentinel** toward unique admin address statistics.
- **Alert on sentinel appearances** in production — they indicate storage corruption or initialization failure.

#### For Operators

- **The sentinel in events is a warning sign.** Investigate admin storage state immediately.
- **Recovery with sentinel succeeds**, but the contract may not have a functioning admin afterward.
- **Re-initialize admin set** after recovery if the sentinel appears.

## Alternatives Considered

### 1. Return `Option<Address>` in Events

**Rejected.** Soroban contract events require concrete types. Returning `Option<Address>` would require wrapping the value in a struct or enum, breaking compatibility with existing indexers expecting a raw `Address` topic.

### 2. Panic on Missing Admin During Recovery

**Rejected.** Recovery operations must succeed even when admin state is corrupt, otherwise the contract becomes permanently bricked. Panicking defeats the purpose of recovery.

### 3. Use a Different Sentinel (e.g., Contract Address)

**Rejected.** The contract's own address is a valid Stellar address and could theoretically be used as an admin (self-administered contract). The zero-address is the only address **guaranteed** to have no private key.

### 4. Emit Event Without Admin Field

**Rejected.** Event schemas must remain stable. Omitting the admin field would break existing event parsers and indexers.

## Detection Helper

The `bettapay_common` module provides a helper function for sentinel detection:

```rust
pub fn is_zero_address(addr: &Address) -> bool {
    addr.to_string() == "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
}
```

Callers should use this helper rather than hard-coding the sentinel string.

## References

- **Implementation:**
  - `settlement_contract/src/storage.rs` (`read_optional_primary_admin`)
  - `governance_contract/src/lib.rs` (`read_optional_primary_admin`)
  - `bettapay_common/src/storage.rs` (`is_zero_address`)

- **Event Usage:**
  - `settlement_contract/src/lib.rs` (`transfer_admin`, `execute_recovery`)
  - `governance_contract/src/lib.rs` (`transfer_admin`, `execute_recovery`)

- **Related ADRs:**
  - [ADR-003: Recovery Mode](./003-recovery-mode.md) — When recovery operations emit the sentinel

---

**Authors:** BettaPay Core Team  
**Date:** October 2024  
**Status:** Accepted
