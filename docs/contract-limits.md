# Contract Limits

This is the reference for payment, batch, settlement-delay, and fee bounds. The values below are sourced from the contract constants; `make check_docs` fails if a source value changes without a documentation update.

| Constant | Value | Rationale | Enforcement point |
|---|---:|---|---|
| `MIN_PAYMENT_AMOUNT` | `100` smallest asset units | Prevents fee-rounding distortion from turning a dust payment into an effectively 100% fee. | `store_payment_reference` and `calculate_fee_split`; both reject smaller amounts with `AmountTooSmall`. |
| `MAX_SETTLEMENT_DELAY_LEDGER` | `100000` ledgers | Places a finite upper bound on how long a payment may be delayed before settlement. | `validate_rule_bounds`, called by `set_settlement_rule`; the equivalent checks in `set_default_rule` enforce the default rule. Both raise `InvalidSettlementDelay`. |
| `MAX_PAYMENTS_BATCH` | `100` references | Bounds the work and returned data for one batch lookup. | `get_payments`; more than 100 references raises `BatchTooLarge`. |
| `MAX_PAYMENTS_PER_MERCHANT` | `10000` payment records | Reserves a documented ceiling for per-merchant persistent state and rent exposure. | **Documented but not yet enforced.** No contract entry point currently rejects the 10001st payment on this basis. |
| `MIN_FEE_BPS` | `5` bps (`0.05%`) | Prevents fee legs from being configured below the protocol minimum. | `validate_rule_bounds` for merchant rules, `set_default_rule` for the default rule, and governance fee configuration validation. |
| `MAX_FEE_BPS` | `5000` bps (`50%`) | Prevents either individual fee leg from taking more than half of a payment. | `validate_rule_bounds` for merchant rules, `set_default_rule` for the default rule, and governance fee configuration validation. |
| `BPS_DENOMINATOR` | `10000` bps (`100%`) | Defines the basis-point scale and prevents the combined platform and network fees from exceeding the whole payment. | `validate_rule_bounds`, `set_default_rule`, governance fee validation, and fee calculation arithmetic. The rule setters reject a combined sum above 10000. |

`MIN_PAYMENT_AMOUNT`, `MAX_SETTLEMENT_DELAY_LEDGER`, `MAX_PAYMENTS_BATCH`, and `MAX_PAYMENTS_PER_MERCHANT` are defined in `settlement_contract/src/policy.rs`; the settlement contract re-exports the public payment caps. The fee constants are shared from `bettapay_common/src/constants.rs` so governance and settlement cannot silently use different values.

A constant with no enforcing code path is intentionally not treated as an active limit. In particular, `MAX_PAYMENTS_PER_MERCHANT` is a policy/documentation marker only until a separate enforcement change is made.
