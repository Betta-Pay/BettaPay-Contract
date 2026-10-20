# Event Topic Convention

This page defines the stable, indexer-facing event-topic contract for
`governance_contract` and `settlement_contract`. The topic registry is
[`bettapay_common::events`](../bettapay_common/src/events.rs), and its
`ALL_EVENT_TOPICS` slice is the canonical complete set. The registry and
contract tests pin both the spelling and membership of each topic.

## Topic layout

- `topic[0]` is the event name as a Soroban `Symbol`. Its value must come from
  a named `*_EVENT` constant in `bettapay_common::events`.
- `topic[1]` and subsequent topics carry entity identifiers or other values
  intended for filtering (for example an asset, merchant, or operation hash).
  An event without an identifier may have only `topic[0]`.
- The event data payload carries the event details. Do not move payload fields
  into topics or change an existing payload shape as part of a topic rename.
- Contracts must not construct topic `Symbol`s from inline string literals.
  Import the registry constant instead. The topic conformity tests guard
  emitted topics, and the pinned full-set test makes additions deliberate.

## Canonical topics

The value shown is the exact `topic[0]` Symbol value. "Both" means the topic
is emitted by both contracts; other rows identify the only emitting contract.

### Shared topics

| Registry constant | Topic value | Emitting contract(s) |
| --- | --- | --- |
| `ADMIN_TRANSFERRED_EVENT` | `admin_transferred` | `governance_contract`, `settlement_contract` |
| `PAUSED_EVENT` | `paused` | `governance_contract`, `settlement_contract` |
| `UNPAUSED_EVENT` | `unpaused` | `governance_contract`, `settlement_contract` |
| `RECOVERY_INITIATED_EVENT` | `recovery_initiated` | `governance_contract`, `settlement_contract` |
| `RECOVERY_CANCELLED_EVENT` | `recovery_cancelled` | `governance_contract`, `settlement_contract` |
| `RECOVERY_EXECUTED_EVENT` | `recovery_executed` | `governance_contract`, `settlement_contract` |
| `CONTRACT_UPGRADED_EVENT` | `contract_upgraded` | `governance_contract`, `settlement_contract` |
| `THRESHOLD_CHANGED_EVENT` | `threshold_changed` | `governance_contract`, `settlement_contract` |
| `RECOVERY_ADDRESS_UPDATED_EVENT` | `recovery_address_updated` | `governance_contract`, `settlement_contract` |

### Governance-only topics

| Registry constant | Topic value | Emitting contract |
| --- | --- | --- |
| `INITIALIZED_EVENT` | `initialized` | Governance |
| `MIGRATED_EVENT` | `migrated` | Governance |
| `SYS_PARAM_UPDATED_EVENT` | `sys_param_updated` | Governance |
| `FEE_CONFIG_UPDATED_EVENT` | `fee_config_updated` | Governance |
| `ANCHOR_UPSERTED_EVENT` | `anchor_upserted` | Governance |
| `ANCHOR_REMOVED_EVENT` | `anchor_removed` | Governance |

### Settlement-only topics

| Registry constant | Topic value | Emitting contract |
| --- | --- | --- |
| `GOVERNANCE_UPDATED_EVENT` | `governance_updated` | Settlement |
| `MERCHANT_REGISTERED_EVENT` | `merchant_registered` | Settlement |
| `MERCHANT_CONSENT_EVENT` | `merchant_consent` | Settlement |
| `MERCHANT_UNREGISTERED_EVENT` | `merchant_unregistered` | Settlement |
| `SETTLEMENT_RULE_UPDATED_EVENT` | `settlement_rule_updated` | Settlement |
| `SETTLEMENT_RULE_CLEARED_EVENT` | `settlement_rule_cleared` | Settlement |
| `DEFAULT_RULE_UPDATED_EVENT` | `default_rule_updated` | Settlement |
| `BOOTSTRAP_FALLBACK_EVENT` | `bootstrap_fallback` | Settlement |
| `PAYMENT_STORED_EVENT` | `payment_stored` | Settlement |
| `PAYMENTS_PRUNED_EVENT` | `payments_pruned` | Settlement |
| `PAYMENT_PRUNED_EVENT` | `payment_pruned` | Settlement |
| `OP_SCHEDULED_EVENT` | `op_scheduled` | Settlement |
| `OP_EXECUTED_EVENT` | `op_executed` | Settlement |
| `OP_CANCELLED_EVENT` | `op_cancelled` | Settlement |
| `FEE_CEILING_REJECTED_EVENT` | `fee_ceiling_rejected` | Settlement |

### `payment_pruned` payload

`prune_payment` emits exactly one event per actually-removed record with
topics `(Symbol("payment_pruned"), merchant, reference)` and data
`(merchant, reference)`. Pruning a missing record is a no-op and emits
nothing, so an indexer never sees a prune that removed no state (issue #890).

## Stability and changes

`settlement_contract/src/tests/event_topic_registry_tests.rs` pins the complete
ordered topic set in `ALL_EVENT_TOPICS` and checks emitted `topic[0]` values
against that registry. The settlement conformity tests and
`governance_contract/src/anchor_event_tests.rs` also assert event names against
their exact registry constants. Additions require an intentional registry,
test, and documentation update. Existing topic names and payloads are not
renamed by this convention.
