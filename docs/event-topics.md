# Event topic registry

Every BettaPay event uses the `Symbol` constants defined in
[`bettapay_common::events`](../bettapay_common/src/events.rs) as `topic[0]`.
Neither contract constructs a topic name from an inline string literal; this
file and the shared constants are the single source of truth for indexers.

`topic[1..n]` and the data payload are fixed per event. Where a payload type is
shared between contracts (`AdminTransferred`, `PendingRecovery`), it lives in
`bettapay_common::events` as well.

## Emitted by both contracts

| Topic | Emitted by | Data |
|---|---|---|
| `initialized` | `governance_contract::init`, `settlement_contract::init` | Primary admin `Address` |
| `admin_transferred` | `transfer_admin`, `execute_recovery` | `AdminTransferred { old_admin, new_admin }` |
| `threshold_changed` | `change_threshold` | `(old_threshold, new_threshold)` |
| `paused` | `pause` | `(admin, true)` |
| `unpaused` | `unpause` | `(admin, false)` |
| `recovery_initiated` | `initiate_recovery` | `(recovery_address, new_admin, execute_after)` |
| `recovery_cancelled` | `cancel_recovery` | `admin` |
| `recovery_executed` | `execute_recovery` | `AdminTransferred { old_admin, new_admin }` |
| `contract_upgraded` | `upgrade` | `admin` (Wasm hash in `topic[1]`) |

## `governance_contract` only

| Topic | Trigger |
|---|---|
| `sys_param_updated` | System parameter updated |
| `fee_config_updated` | Fee configuration changed |
| `anchor_upserted` | Anchor created or replaced for an asset |
| `anchor_removed` | Anchor removed for an asset |

## `settlement_contract` only

| Topic | Trigger |
|---|---|
| `governance_updated` | `update_governance` stored a new governance address |
| `merchant_registered` | Merchant registered |
| `merchant_unregistered` | Merchant unregistered |
| `settlement_rule_updated` | Merchant rule set or replaced |
| `settlement_rule_cleared` | Merchant rule cleared (also on unregister) |
| `default_rule_updated` | Global default rule updated |
| `bootstrap_fallback` | `read_rule_or_default` fell back to the bootstrap rule |
| `payment_stored` | Payment reference stored |
| `op_scheduled` | Timelocked operation scheduled |
| `op_executed` | Timelocked operation executed |
| `op_cancelled` | Timelocked operation cancelled |

## Conformity tests

Topic names are enforced by the contracts' test suites:

- `governance_contract::tests` (initialized/init event assertions)
- `settlement_contract::tests::event_topic_conformity_tests`
