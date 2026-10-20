# Development Guide

Notes for contributors changing contract code that is already deployed. For
build, test and deploy instructions see [CONTRIBUTING.md](CONTRIBUTING.md) and
the [README](README.md).

## Table of Contents

- [Contract Upgrades](#contract-upgrades)
- [Operator Runbook: Probe, Upgrade, Migrate, and Verify](#operator-runbook-probe-upgrade-migrate-and-verify)
- [Storage Layout](#storage-layout)
- [Lifecycle & Architecture Specifications](#lifecycle--architecture-specifications)
- [Two Constraints That Shape Every Migration](#two-constraints-that-shape-every-migration)
- [The Migration Pattern](#the-migration-pattern)
- [Migrating Entries You Cannot Enumerate](#migrating-entries-you-cannot-enumerate)
- [Worked Example: Adding a Field to `PaymentRecord`](#worked-example-adding-a-field-to-paymentrecord)
- [Worked Example: Changing a `DataKey` Variant](#worked-example-changing-a-datakey-variant)
- [TTL and Archived Entries](#ttl-and-archived-entries)
- [Fuzzing Storage-Key Decoding](#fuzzing-storage-key-decoding)
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

## Operator Runbook: Probe, Upgrade, Migrate, and Verify

Follow these steps for each contract independently. The order is deliberate:
probe the candidate Wasm, upgrade the contract, call its migration entry
point, then verify before moving on. The upgrade entry point also probes the
candidate itself and refuses a Wasm that does not implement the required
interface.

Set the network, IDs, and signer values for the deployment. `ADMIN_SIGNERS`
must be a JSON array containing enough current admin addresses to meet that
contract's stored multisig threshold; those admins must authorize the
transaction. `DEPLOYER` is the funded CLI identity that submits and pays for
the operator transactions.

```bash
export RPC="https://soroban-rpc.example"
export PASS="network passphrase"
export DEPLOYER="funded-operator-identity"
export ADMIN_SIGNERS='["G...ADMIN_1","G...ADMIN_2"]'
export SETTLEMENT_ID="C...SETTLEMENT"
export GOVERNANCE_ID="C...GOVERNANCE"
```

1. **Build and upload the candidate Wasm.** Keep the previous deployed Wasm
   artifacts and hashes for rollback analysis.

   ```bash
   make optimize
   SETTLEMENT_WASM_HASH=$(soroban contract upload \
     --wasm target/optimized/settlement_contract_opt.wasm \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS")
   GOVERNANCE_WASM_HASH=$(soroban contract upload \
     --wasm target/optimized/governance_contract_opt.wasm \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS")
   ```

2. **Probe each candidate.** Deploy a disposable instance of each uploaded
   candidate, then invoke `supports_interface` on it without calling `init`.
   The current required interface is `1` for both contracts:
   `settlement_contract::SUPPORTED_INTERFACE_VERSION` and the governance
   contract's `SUPPORTED_INTERFACE_VERSION`. Continue only when both calls
   return `true`. The `upgrade` calls below repeat this check against the
   uploaded Wasm before changing the live executable through
   `bettapay_common::upgrade::probe_supports_interface`.

   ```bash
   SETTLEMENT_PROBE_ID=$(soroban contract deploy \
     --wasm target/optimized/settlement_contract_opt.wasm \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS")
   soroban contract invoke --id "$SETTLEMENT_PROBE_ID" \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     supports_interface --version 1

   GOVERNANCE_PROBE_ID=$(soroban contract deploy \
     --wasm target/optimized/governance_contract_opt.wasm \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS")
   soroban contract invoke --id "$GOVERNANCE_PROBE_ID" \
     --source-account "$DEPLOYER" --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     supports_interface --version 1
   ```

3. **Optionally pause writes before the upgrade.** Each `pause` call requires
   the contract's current admin threshold. Settlement takes a bounded
   `reason`; governance uses its unspecified-reason default.

   ```bash
   soroban contract invoke --id "$SETTLEMENT_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     pause --signers "$ADMIN_SIGNERS" --reason 1
   soroban contract invoke --id "$GOVERNANCE_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     pause --signers "$ADMIN_SIGNERS"
   ```

   `upgrade` is permitted while paused, but both contracts reject `migrate`
   while paused. If you pause, unpause after the code swap and before the
   migration; arrange an external write freeze if writes must remain stopped
   across that interval.

4. **Upgrade the live contracts.** Each entry point is named `upgrade` and
   requires a `signers` vector meeting the current admin threshold.

   ```bash
   soroban contract invoke --id "$SETTLEMENT_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     upgrade --signers "$ADMIN_SIGNERS" --new_wasm_hash "$SETTLEMENT_WASM_HASH"
   soroban contract invoke --id "$GOVERNANCE_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     upgrade --signers "$ADMIN_SIGNERS" --new_wasm_hash "$GOVERNANCE_WASM_HASH"
   ```

5. **Unpause if step 3 was used, then migrate each contract.** The
   admin-gated `migrate(signers)` entry point must be called on each contract,
   with enough current admins to satisfy its threshold. It advances that
   contract's instance-storage `SchemaVersion` marker through
   `CURRENT_SCHEMA_VERSION` and emits `migrated` with the resulting version.

   ```bash
   soroban contract invoke --id "$SETTLEMENT_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     unpause --signers "$ADMIN_SIGNERS"
   soroban contract invoke --id "$GOVERNANCE_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     unpause --signers "$ADMIN_SIGNERS"

   soroban contract invoke --id "$SETTLEMENT_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     migrate --signers "$ADMIN_SIGNERS"
   soroban contract invoke --id "$GOVERNANCE_ID" --source-account "$DEPLOYER" \
     --rpc-url "$RPC" --network-passphrase "$PASS" -- \
     migrate --signers "$ADMIN_SIGNERS"
   ```

   Run the `unpause` calls only for contracts paused in step 3.

6. **Verify before declaring success.** Confirm each migration invocation
   succeeded and inspect its `migrated` event data for the expected schema
   version. `version` reports the deployed build's schema version; it is not
   a read of the stored marker. Read representative contract state through
   the new public getters and compare it with pre-upgrade records. Confirm
   both contracts' expected behavior before proceeding with unrelated
   deployments or upgrades.

7. **Rollback deliberately.** Keep the old Wasm and its hash available.
   Rolling code back is another admin-threshold `upgrade` to the old hash,
   and is safe only if that old code can read the current storage schema.
   Storage writes made by `migrate` are not automatically undone. If the
   schema has changed incompatibly, do not blindly restore old Wasm; use a
   compatible forward fix or a reviewed reverse migration first.

## Storage Layout

What is on the ledger today, and which storage kind holds it. See [`docs/storage-key-ownership.md`](docs/storage-key-ownership.md) for authoritative cross-crate ownership and TTL policies.

### Settlement contract

| Key | Storage | Holds |
| --- | --- | --- |
| `Admin` | instance | `Vec<Address>` |
| `Initializing` | instance | sentinel set during `init` to prevent reentrancy, removed once complete |
| `Governance` | instance | `Address` |
| `Deployer` | instance | `Address` (gates `init` front-running prevention, issue #684) |
| `SchemaVersion` | instance | `u32` (schema version written at `init`, issue #507, #704) |
| `CachedGovRule` | instance | `Option<SettlementRule>` (cached governance fee rule, issue #744) |
| `DefaultRule` | instance | `SettlementRule` (global default settlement rule) |
| `Merchant(Address)` | persistent | `bool` (merchant registration status) |
| `Rule(Address)` | persistent | `SettlementRule` (merchant-specific settlement rule) |
| `ArchivedMerchant(Address)` | persistent | `bool` (tombstone written when merchant unregisters, issue #490) |
| `Payment(Address, BytesN<32>)` | persistent | `PaymentRecord` (scoped by merchant and reference, issue #493) |
| `ScheduledOperation(BytesN<32>)` | persistent | `ScheduledOp` (timelocked operation payload and execute_at) |
| `PendingOperationCount` | instance | `u32` (pending timelock queue depth, issue #887) |

#### Shared Keys (`CommonDataKey`)
The following instance keys are shared across contracts via `bettapay_common::storage::CommonDataKey`:

| Key | Storage | Holds |
| --- | --- | --- |
| `RecoveryAddress` | instance (`CommonDataKey`) | `Address` (authorised recovery address) |
| `PendingRecovery` | instance (`CommonDataKey`) | `PendingRecovery` (in-flight recovery record) |
| `Paused` | instance (`CommonDataKey`) | `bool` (pause flag) |
| `Threshold` | instance (`CommonDataKey`) | `u32` (multisig admin threshold) |

Note that `RecoveryAddress`, `PendingRecovery`, `Paused`, and `Threshold` are shared through `CommonDataKey` (from `bettapay_common::storage`), while `Admin` remains local (`Vec<Address>`) under each contract's own `DataKey`. See [Storage Key Ownership](docs/storage-key-ownership.md) and [Recovery Lifecycle](docs/recovery-lifecycle.md).

The payment reference's fixed `BytesN<32>` length is named as
`MAX_PAYMENT_REFERENCE_LEN` (32) rather than left as a bare literal.

### Governance contract

| Key | Storage | Holds |
| --- | --- | --- |
| `Admin` | instance | `Vec<Address>` |
| `FeeConfig` | instance | `FeeConfig` (protocol fee ceilings) |
| `SchemaVersion` | instance | `u32` (written at `init`, issue #507) |
| `Deployer` | instance | `Address` (gates `init` front-running prevention, issue #684) |
| `Anchor(Address)` | persistent | `Address` (anchor address per asset) |
| `SystemParam(Symbol)` | persistent | `i128` (numeric system parameter) |

#### Shared Keys (`CommonDataKey`)
The following instance keys are shared across contracts via `bettapay_common::storage::CommonDataKey`:

| Key | Storage | Holds |
| --- | --- | --- |
| `RecoveryAddress` | instance (`CommonDataKey`) | `Address` (authorised recovery address) |
| `PendingRecovery` | instance (`CommonDataKey`) | `PendingRecovery` (in-flight recovery record) |
| `Paused` | instance (`CommonDataKey`) | `bool` (pause flag) |
| `Threshold` | instance (`CommonDataKey`) | `u32` (multisig admin threshold) |

The split matters for migrations:

- **Fixed keys** (`Admin`, `Paused`, `FeeConfig`, `DefaultRule`, …) — there is
  exactly one of each and the code already knows the key. Migrating these is
  a handful of read-transform-write statements.
- **Parameterised keys** (`Payment(_)`, `Merchant(_)`, `Rule(_)`, `Anchor(_)`,
  `SystemParam(_)`) — one entry per address, reference or symbol, and the set
  of live keys is not recorded anywhere on-chain. These are the hard case, and
  the [section below](#migrating-entries-you-cannot-enumerate) is about them.

## Lifecycle & Architecture Specifications

For detailed architectural lifecycles, timing rules, and collision models:
- **[Scheduled Operation Lifecycle](docs/scheduled-operation-lifecycle.md)**: Defines states (`Unscheduled`, `Scheduled`, `Executed`, `Cancelled`), SHA-256 indexing with stored byte checks, duplicate reschedule (`OperationAlreadyScheduled` 12) vs. genuine hash collision (`OperationHashCollision` 316), minimum 7-day delay (`ExecutionNotReady` 10), recovery veto, permissionless execution, and the 9 operation variants.
- **[Admin Recovery Lifecycle](docs/recovery-lifecycle.md)**: End-to-end admin recovery sequence across actors (recovery address, admin multisig, permissionless executor), 7-day `RECOVERY_DELAY_SECONDS`, cancellation rules, failure modes (`RecoveryNotPending` 8, `RecoveryDelayActive` 9, `RecoveryAlreadyPending` 15), and timelock veto semantics.
- **[Bootstrap Fallback Semantics](docs/bootstrap-fallback.md)**: Details the 4-tier settlement fallback chain (merchant → default → governance → bootstrap), trigger conditions, constant values (`100` platform bps, `5` network bps, `0` delay, `auto_settle` false), and event emission rules.
- **[Storage Key Ownership and TTL Policies](docs/storage-key-ownership.md)**: Comprehensive mapping of all storage keys across `bettapay_common`, `settlement_contract`, and `governance_contract`.

## Two Constraints That Shape Every Migration

Both are properties of Soroban, not of this codebase. They rule out approaches
that look reasonable on paper, so they are worth stating before any plan.

### 1. Storage cannot be enumerated

`soroban_sdk::storage` (21.7.7) exposes `has`, `get`, `set`, `update`,
`try_update`, `extend_ttl` and `remove`. There is no iterator, no key listing,
no "scan the prefix". Every read requires the caller to already know the exact
key.

This is visible in the existing API surface: `get_payments` takes
`merchant` plus `references: Vec<BytesN<32>>` from the caller rather than
returning all payments, because returning all payments is not something the
contract can do.

So "read the old-format data from storage" is only a well-defined instruction
for fixed keys. For `Payment(Address, BytesN<32>)` the contract does not and
cannot know
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

*Note: Both contracts ship a `SchemaVersion` marker (written at `init`) and an admin-gated, idempotent `migrate` entry point (issues #507, #704). Since issue #984, `migrate` is rejected while paused and, when the stored version is below `CURRENT_SCHEMA_VERSION`, calls a `migrate_step(env, from_version)` hook once per version crossed and then advances the marker. At `CURRENT_SCHEMA_VERSION` it is a no-op. To ship a conversion, bump `CURRENT_SCHEMA_VERSION` and add its step to that hook in each affected contract. The order is always: upgrade the Wasm, call `migrate`, verify.*

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
soroban contract invoke --id "$SETTLEMENT_ID" -- upgrade \
  --signers '["G...ADMIN"]' \
  --new_wasm_hash "$NEW_WASM_HASH"
```

Between this step and the next, the contract is running new code over
old-format data. Keep the window short, and see
[step 6](#6-consider-pausing-for-the-window) on pausing.

### 4. Migrate

```bash
soroban contract invoke --id "$SETTLEMENT_ID" -- migrate
```

`migrate` reads with the old type, writes with the new one, removes superseded
keys, and sets `SchemaVersion` to the new value. It is idempotent; calling it
again when the marker is current is a no-op — see the example below.

### 5. Verify before moving on

Read back a representative sample through the new getters and confirm the
values match what was there before. The `migrated` event includes the schema
version written by the call; `version()` reports the deployed build's
compile-time version, not the stored marker. Only treat the migration as done
after checking the event and representative state.

### 6. Consider pausing for the window

Both contracts have `pause`/`unpause`. If the old and new formats cannot
coexist safely during the upgrade, pause before it. `migrate` is rejected
while paused, so unpause before migrating; arrange an external write freeze if
mutations must remain blocked through the migration. See the
[operator runbook](#operator-runbook-probe-upgrade-migrate-and-verify) for
both contracts' commands.

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

## PaymentRecord Schema Versioning

As of issue #967, `PaymentRecord` includes a `version: u32` field written at
storage time. New records are written with `version = 1`.

**Old records:** Entries stored before this field existed have no `version` key
and are detected by the field's absence during deserialization. Callers must
treat missing `version` as `version = 0`. The field is encoded by name in the
`#[contracttype]` map, so older contract versions will not decode the entry at
all if the field is present (name-based encoding protects against silent
corruption).

**Future migrations:** The `version` field supports deterministic evolution
without renaming the struct. When adding new fields, increment the version and
pair it with a conversion function that maps old shapes to new ones,
following the lazy-migration pattern shown in the next section.

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
pub fn get_payment_reference(env: Env, merchant: Address, reference: BytesN<32>) -> Option<PaymentRecord> {
    let key = DataKey::Payment(merchant, reference);

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

## Fuzzing Storage-Key Decoding

The settlement contract's DataKey decode regression harness checks every
`DataKey` and `CommonDataKey` variant for an XDR round-trip, exercises
arbitrary byte strings generated by proptest, and mutates valid encodings.
Decode failures are expected for malformed input; a panic is not, and reports
the offending input in hexadecimal.

Run the harness with:

```bash
cargo test -p settlement_contract data_key_decode
```

Proptest reports the seed and case needed to reproduce a random counterexample;
the deterministic mutation test reports its exact input.

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
5. Assert `migrate` a second time leaves the schema marker unchanged and
   emits one `migrated` event for that invocation.
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
