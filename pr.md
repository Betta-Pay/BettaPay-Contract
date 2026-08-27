# Fix #572 and #570: order-dependent fee decode assumption, unhandled ScheduledOperation hash collisions

## #572 — FeeConfig decode in settlement assumes governance tuple ordering

**Problem:** `read_governance_fee_rule` / `validate_fee_against_governance` in
`settlement_contract/src/storage.rs` decode governance's `FeeConfig` into
settlement's own `GovFeeConfig` via a cross-contract call. The concern raised
in the issue is that this decode is positional (index 0 = platform, index 1 =
network), so reordering the fields in either struct would silently swap which
bps value lands where.

**Finding:** Soroban's `#[contracttype]` derive for named-field structs
(`soroban-sdk-macros::derive_struct::derive_type_struct`) encodes/decodes
structs as an `ScMap` keyed by field **name**, not by declaration order or
positional index — both directions (`Val -> Self` and `Self -> Val`) build
their key/value slices from field names sorted alphabetically, independent of
how the struct is written in source. So the decode was already
order-independent by construction; there was no code change required to make
it so.

**What changed:** Added `settlement_contract/src/tests/fee_config_ordering_tests.rs`,
which locks in that guarantee with an explicit test rather than leaving it as
an implicit (and easy to break by switching to a tuple struct, or to a
different SDK) property:

- `decode_is_order_independent_by_field_name` stands up a governance stub
  (`ReorderedGovernance`) whose fee-config type declares `network_fee_bps`
  *before* `platform_fee_bps` — the opposite order from settlement's
  `GovFeeConfig` — and asserts the cross-contract decode still lands each bps
  value in the correctly named field.
- `calculate_fee_split_uses_correctly_ordered_governance_fees` drives the same
  reordered-fields stub through the real `calculate_fee_split` entry point
  end-to-end and checks the computed platform/network fee amounts.

If a future SDK upgrade or refactor ever made struct decoding positional
again, both tests would fail immediately.

## #570 — ScheduledOperation key uses a hash but collisions aren't handled

**Problem:** `schedule()`/`execute()`/`cancel()` in
`settlement_contract/src/admin.rs` key pending operations by
`sha256(operation.to_xdr())` alone. The stored value was just the `execute_at`
timestamp. A hash match was treated as proof the caller's `operation` *is*
the one that was scheduled — it never actually compared content. In the
(cryptographically remote, but explicitly called out by the issue) case of a
hash collision, this meant:
- `schedule()` would refuse to schedule a genuinely different operation whose
  hash collides, but under the wrong error and for the wrong reason.
- `execute()` would run whatever `Operation` the caller passed in — including
  one that was never actually scheduled or timelocked — the instant a
  colliding hash's slot became ripe, silently borrowing another operation's
  approval and delay.

**Fix:** `DataKey::ScheduledOperation(hash)` now indexes a new
`ScheduledOp { operation_xdr: Bytes, execute_at: u64 }` (`types.rs`) instead
of a bare `u64`. The hash is only used to find the storage slot; the full XDR
bytes of the operation are stored and checked byte-for-byte before schedule,
execute, or cancel proceed:

- `schedule()`: if a slot exists under the computed hash, compare
  `existing.operation_xdr` to the new operation's XDR.
  - Equal → same operation being re-scheduled → existing `OperationAlreadyScheduled` (#12).
  - Different → genuine hash collision → new `OperationHashCollision` (#316).
- `execute()` / `cancel()`: after loading the slot, reject with
  `OperationNotScheduled` (#11) if the stored `operation_xdr` doesn't match
  the operation supplied by the caller, before touching the timelock or
  running any operation effects.

Added `SettlementError::OperationHashCollision = 316`, wired through the
`bettapay_common::error_codes` conformity checks in
`settlement_contract/src/tests/conformity_tests.rs` (range + collision
checks) alongside the existing error codes.

**Collision-handling code path — see
`settlement_contract/src/tests/schedule_collision_tests.rs`:**

A real SHA-256 collision can't be manufactured in a test, so these tests
simulate one by writing a `ScheduledOp` directly into the storage slot that a
given `Operation` hashes to, with unrelated `operation_xdr` bytes standing in
for "a different operation that happens to collide":

- `schedule_detects_collision_with_unrelated_pending_operation` — schedule
  panics with `OperationHashCollision` (#316) instead of silently overwriting
  the planted slot.
- `execute_rejects_operation_that_only_collides_on_hash` — execute panics
  with `OperationNotScheduled` (#11) instead of running the operation, even
  though the planted slot's `execute_at` had already elapsed.
- `cancel_rejects_operation_that_only_collides_on_hash` — same check on the
  cancel path.
- `rescheduling_the_same_operation_is_still_a_plain_duplicate` — confirms the
  ordinary duplicate-schedule case still raises `OperationAlreadyScheduled`
  (#12), so the new collision error doesn't change behavior for the common
  path.

## Acceptance criteria

- [x] Decode is order-independent (verified: SDK encodes/decodes structs by
      field name) and explicitly tested (`fee_config_ordering_tests.rs`).
- [x] Collisions handled: `ScheduledOp` stores the full operation XDR and all
      three entry points (`schedule`/`execute`/`cancel`) verify it against the
      hash-indexed slot before trusting it.
- [x] `cargo test --workspace` passes.
