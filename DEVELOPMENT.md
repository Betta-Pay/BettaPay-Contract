# Development Guide

Notes for contributors changing contract code that is already deployed. For
build, test and deploy instructions see [CONTRIBUTING.md](CONTRIBUTING.md) and
the [README](README.md).

## Table of Contents

- [Contract Upgrades](#contract-upgrades)
- [Storage Layout](#storage-layout)
- [Two Constraints That Shape Every Migration](#two-constraints-that-shape-every-migration)
- [The Migration Pattern](#the-migration-pattern)
- [Migrating Entries You Cannot Enumerate](#migrating-entries-you-cannot-enumerate)
- [Worked Example: Adding a Field to `PaymentRecord`](#worked-example-adding-a-field-to-paymentrecord)
- [Worked Example: Changing a `DataKey` Variant](#worked-example-changing-a-datakey-variant)
- [TTL and Archived Entries](#ttl-and-archived-entries)
- [Testing a Migration](#testing-a-migration)
- [Checklist](#checklist)

## Contract Upgrades

Both contracts expose an admin-only `upgrade` entry point:

- `SettlementContract::upgrade(env, signers, new_wasm_hash)`
- `GovernanceContract::upgrade(env, signers, new_wasm_hash)`

Each calls `env.deployer().update_current_contract_wasm(new_wasm_hash)`.

**What that does:** replaces the executable Wasm. The contract ID, its address,
and every storage entry it owns are untouched. The new code takes over on the
next invocation.

**What it does not do:** anything at all to storage. There is no schema, no
implicit conversion, and no validation that the data already on the ledger
still matches the types the new code expects. If the new Wasm reads an entry
with a type that no longer matches how it was written, that read fails at
runtime — after the upgrade has already landed.

That is the whole reason this document exists. `upgrade` is safe precisely
because it ignores storage, which means changing storage is a separate problem
you have to solve deliberately.

## Storage Layout

What is on the ledger today, and which storage kind holds it.

### Settlement contract

| Key | Storage | Holds |
| --- | --- | --- |
| `Admin` | instance | `Address` |
| `RecoveryAddress` | instance | `Address` |
| `PendingRecovery` | instance | `PendingRecovery` |
| `Governance` | instance | `Address` |
| `Paused` | instance | `bool` |
| `DefaultRule` | persistent | `SettlementRule` |
| `Merchant(Address)` | persistent | merchant registration |
| `Rule(Address)` | persistent | `SettlementRule` |
| `Payment(BytesN<32>)` | persistent | `PaymentRecord` |

### Governance contract

| Key | Storage | Holds |
| --- | --- | --- |
| `Admin` | instance | `Address` |
| `RecoveryAddress` | instance | `Address` |
| `PendingRecovery` | instance | `PendingRecovery` |
| `Paused` | instance | `bool` |
| `FeeConfig` | persistent | fee configuration |
| `Anchor(Address)` | persistent | anchor address per asset |
| `SystemParam(Symbol)` | persistent | numeric system parameter |

The split matters for migrations:

- **Fixed keys** (`Admin`, `Paused`, `FeeConfig`, `DefaultRule`, …) — there is
  exactly one of each and the code already knows the key. Migrating these is
  a handful of read-transform-write statements.
- **Parameterised keys** (`Payment(_)`, `Merchant(_)`, `Rule(_)`, `Anchor(_)`,
  `SystemParam(_)`) — one entry per address, reference or symbol, and the set
  of live keys is not recorded anywhere on-chain. These are the hard case, and
  the [section below](#migrating-entries-you-cannot-enumerate) is about them.

## Two Constraints That Shape Every Migration

Both are properties of Soroban, not of this codebase. They rule out approaches
that look reasonable on paper, so they are worth stating before any plan.

### 1. Storage cannot be enumerated

`soroban_sdk::storage` (21.7.7) exposes `has`, `get`, `set`, `update`,
`try_update`, `extend_ttl` and `remove`. There is no iterator, no key listing,
no "scan the prefix". Every read requires the caller to already know the exact
key.

This is visible in the existing API surface: `get_payments` takes
`references: Vec<BytesN<32>>` from the caller rather than returning all
payments, because returning all payments is not something the contract can do.

So "read the old-format data from storage" is only a well-defined instruction
for fixed keys. For `Payment(BytesN<32>)` the contract does not and cannot know
which references exist.

### 2. A contract can only touch its own storage

`env.storage()` is always the storage of the contract currently executing.
There is no API for reading or writing another contract's entries. A
cross-contract call invokes a function; it does not reach into the callee's
ledger entries.

**This rules out the "deploy a temporary migration contract" approach.** A
separate contract cannot read the settlement contract's `Payment` entries or
write new ones — it has no access path. The only way it could participate is by
calling entry points that the settlement contract itself exposes, which means
the settlement contract's Wasm must already contain the migration logic. At
which point the separate contract adds a deployment and an authorisation
boundary while doing nothing the main contract is not already doing.

The migration therefore lives **inside the upgraded contract**, as an
admin-only entry point. This is also what the existing doc comments on both
`upgrade` functions already say: *"a separate storage-migration function should
be written and called after the upgrade"* — a function, in this contract, not a
contract of its own.

## The Migration Pattern

*Note: Neither `migrate` nor `SchemaVersion` is currently implemented in the contracts. This section serves as a theoretical template for contributors when the first breaking change arises.*

Ordering is the point: the code that can read both formats has to be deployed
before anything is rewritten.

### 1. Add a schema version marker

Before the first migration, add a version key so the contract can tell which
format is on the ledger and refuse to run a migration twice.

```rust,ignore
enum DataKey {
    // ...
    SchemaVersion,      // instance storage, u32
}

const CURRENT_SCHEMA_VERSION: u32 = 2;

fn read_schema_version(env: &Env) -> u32 {
    // Entries written before this key existed are version 1 by definition.
    env.storage()
        .instance()
        .get(&DataKey::SchemaVersion)
        .unwrap_or(1)
}
```

### 2. Write a Wasm that understands both formats

The new build keeps the old type definition alongside the new one, and adds an
admin-gated `migrate` entry point. Both must ship in the same Wasm: the old
type is what makes existing entries readable, and the new type is what they
become.

### 3. Upgrade

```bash
stellar contract invoke --id "$SETTLEMENT_ID" -- upgrade \
  --signers '["G...ADMIN"]' \
  --new_wasm_hash "$NEW_WASM_HASH"
```

Between this step and the next, the contract is running new code over
old-format data. Keep the window short, and see
[step 6](#6-consider-pausing-for-the-window) on pausing.

### 4. Migrate

```bash
stellar contract invoke --id "$SETTLEMENT_ID" -- migrate
```

`migrate` reads with the old type, writes with the new one, removes superseded
keys, and sets `SchemaVersion` to the new value. It must be idempotent and must
reject a re-run — see the example below.

### 5. Verify before moving on

Read back a representative sample through the new getters and confirm the
values match what was there before. Confirm `SchemaVersion` advanced. Only then
treat the migration as done.

### 6. Consider pausing for the window

Both contracts have `pause`/`unpause`. If the old and new formats cannot
coexist safely for the duration — for instance if a write in the old format
during the window would be missed by the migration — pause before the upgrade
and unpause after verification.

### 7. Strip the migration code in a later upgrade

Once verified, a subsequent upgrade removes the old type and the `migrate`
entry point, so a stale migration cannot be invoked against current data. This
is the step that replaces "removing the migration contract" in a
separate-contract design.

## Migrating Entries You Cannot Enumerate

For `Payment(_)`, `Merchant(_)`, `Rule(_)`, `Anchor(_)` and `SystemParam(_)`,
the contract cannot produce the list of keys. Three workable approaches:

### Lazy migration on read (preferred)

Leave old entries alone. On each read, if the entry is in the old format,
convert it, write it back, and return the new form. Entries migrate as they are
touched; untouched ones stay valid because the reader still understands them.

- **Good:** no key list needed, no big transaction, no downtime.
- **Cost:** the old type and the conversion stay in the Wasm until you are
  satisfied every live entry has been touched — potentially indefinitely for
  cold data, and a read becomes a write on first touch.

### Admin-supplied batches

`migrate_payments(env, references: Vec<BytesN<32>>)` — the admin passes the
keys in, in batches sized to fit the transaction limits. This mirrors how
`get_payments` already works.

- **Good:** explicit, verifiable, finishes.
- **Cost:** the key list has to come from somewhere. Which means:

### Reconstruct the key set off-chain

Every payment reference appears in the events emitted by
`store_payment_reference`; merchants appear in registration events. An indexer
replaying contract events can rebuild the live key set, which then feeds the
batch calls above.

- **Good:** the only way to get a *complete* list.
- **Cost:** needs event history from contract genesis, and events are subject
  to retention limits on public networks — so confirm the range you need is
  actually still retrievable before depending on this.

**Rule of thumb:** use lazy migration for parameterised keys and an eager
`migrate` for fixed keys. They compose — one `migrate` call for the singletons,
lazy conversion for the long tail.

## Worked Example: Adding a Field to `PaymentRecord`

Say `PaymentRecord` (defined in `settlement_contract/src/types.rs`) gains `settled: bool`.

A `#[contracttype]` struct is encoded as a map keyed by field name. An entry
written before the field existed has no `settled` key, so deserialising it into
the new struct **fails** — the read panics. Existing rows do not silently pick
up a default. This is why the old type has to stay in the Wasm.

```rust,ignore
/// Pre-v2 shape retained in Wasm so existing entries remain readable during lazy migration.
#[contracttype]
#[derive(Clone)]
pub struct PaymentRecordV1 {
    pub amount: i128,
    pub platform_fee_amount: i128,
    pub network_fee_amount: i128,
    pub merchant_amount: i128,
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
    pub ledger: u32,
    pub settlement_delay_ledger: u32,
    pub auto_settle: bool,
}

/// Updated PaymentRecord shape with the new `settled` field.
#[contracttype]
#[derive(Clone)]
pub struct PaymentRecord {
    pub amount: i128,
    pub platform_fee_amount: i128,
    pub network_fee_amount: i128,
    pub merchant_amount: i128,
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
    pub ledger: u32,
    pub settlement_delay_ledger: u32,
    pub auto_settle: bool,
    pub settled: bool,
}

impl PaymentRecordV1 {
    /// The upgrade decision in one place: existing payments predate settlement
    /// tracking, so they are recorded as unsettled.
    fn into_v2(self) -> PaymentRecord {
        PaymentRecord {
            amount: self.amount,
            platform_fee_amount: self.platform_fee_amount,
            network_fee_amount: self.network_fee_amount,
            merchant_amount: self.merchant_amount,
            platform_fee_bps: self.platform_fee_bps,
            network_fee_bps: self.network_fee_bps,
            ledger: self.ledger,
            settlement_delay_ledger: self.settlement_delay_ledger,
            auto_settle: self.auto_settle,
            settled: false,
        }
    }
}
```

Updating the actual contract getter entry point [`get_payment_reference`](settlement_contract/src/payments.rs) to convert in place on read:

```rust,ignore
pub fn get_payment_reference(env: Env, reference: BytesN<32>) -> Option<PaymentRecord> {
    let key = DataKey::Payment(reference);

    // New format first: after conversion this is the only branch taken.
    if let Some(record) = env.storage().persistent().get::<_, PaymentRecord>(&key) {
        env.storage()
            .persistent()
            .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
        return Some(record);
    }

    // Old format: convert, persist, and carry the TTL over so migrating an
    // entry never shortens its life.
    let legacy: PaymentRecordV1 = env.storage().persistent().get(&key)?;
    let migrated = legacy.into_v2();
    env.storage().persistent().set(&key, &migrated);
    env.storage()
        .persistent()
        .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
    Some(migrated)
}
```

The eager half, for fixed keys, gated by admin authentication and idempotent:

```rust,ignore
/// Migrates singleton entries to schema version 2. Admin only.
pub fn migrate(env: Env, signers: Vec<Address>) {
    assert_not_paused(&env);
    verify_admin_auth(&env, &signers, read_threshold(&env));
    let admin = read_admin(&env);

    // ... convert fixed-key entries here ...

    env.events()
        .publish((Symbol::new(&env, "migrated"),), admin);
}
```

## Worked Example: Changing a `DataKey` Variant

A `#[contracttype]` enum encodes the **variant name** as part of the key. So:

- **Adding** a variant is safe. Existing keys are unaffected.
- **Renaming** a variant, or changing its payload types, produces a different
  key. The old entries are not deleted — they become unaddressable by the new
  code, which is worse than deletion because they still consume ledger state
  and still count for TTL.
- **Reordering** variants is safe (the name is encoded, not the index), but
  don't rely on it; keep additions at the end so review stays simple.

To change a key format, keep the old variant long enough to read through it:

```rust,ignore
enum DataKey {
    // ...
    Payment(BytesN<32>),           // v1 key, retained for migration reads
    PaymentV2(BytesN<32>, u32),    // v2 key: reference + settlement epoch
}
```

Read `Payment(r)`, write `PaymentV2(r, epoch)`, then `remove` the old key in the
same call so the entry is never live under both. Drop the `Payment` variant in
the later cleanup upgrade.

## TTL and Archived Entries

Persistent entries expire and are archived if their TTL lapses. Two
consequences for migrations:

1. **Migrating an entry resets nothing by itself.** `set` on an existing key
   does not extend its TTL. Call `extend_ttl` as part of the conversion, using
   the same constants the normal write path uses (`PAYMENT_TTL_*`,
   `RULE_TTL_*`, `MERCHANT_TTL_*`) so migrated entries are not left with a
   shorter life than untouched ones.
2. **An archived entry cannot be read or written** until it is restored. A
   batch migration that includes an archived key fails on that key. Restore
   first, or skip and record it — do not let one archived entry abort a batch
   that would otherwise succeed.

## Testing a Migration

The test that matters writes data in the old format and reads it through the
new code. In `soroban_sdk::testutils` terms:

1. Register the **old** contract, initialise it, and write representative
   entries — at least one of every key shape being changed.
2. Upgrade to the new Wasm in-test.
3. Call `migrate`, and exercise the lazy path by reading an entry that
   `migrate` did not touch.
4. Assert values survived: not just that reads succeed, but that amounts,
   addresses and fee splits are unchanged. A migration that returns defaults
   everywhere also "succeeds".
5. Assert `migrate` a second time panics rather than re-applying.
6. Assert an entry that was already in the new format is left alone.

Point 4 is the one worth insisting on. A conversion that quietly zeroes an
`i128` passes any test that only checks the read did not panic.

## Checklist

Before opening a PR that changes a stored type or a `DataKey` variant:

- [ ] Does existing data still deserialise? If not, the old type ships in the
      same Wasm.
- [ ] Is there a schema version marker, and does `migrate` refuse to re-run?
- [ ] Are parameterised keys handled — lazy, batched, or explicitly documented
      as not needing migration?
- [ ] Is `extend_ttl` called on every entry the migration rewrites?
- [ ] Is `migrate` admin-gated with `require_auth`?
- [ ] Is there a test that writes old-format data and reads it back through the
      new code, asserting values rather than absence of panic?
- [ ] Is the cleanup upgrade that removes the migration code planned?
- [ ] Does the PR say what the operator has to run, in order?
