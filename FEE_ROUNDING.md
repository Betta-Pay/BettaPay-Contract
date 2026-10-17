# Fee rounding and split guarantees

Settlement computes the platform and network fee legs independently with
ceiling division through `Bps::calculate_fee_ceil`:

```text
ceil(gross_amount * fee_bps / 10,000)
```

Rounding is therefore upward, not to nearest or downward. For each
uncapped fee leg, the amount collected exceeds its exact fractional value by
at least zero and **strictly less than one base unit**. With two legs, their
combined rounding excess is strictly less than two base units before any
clamp. This is an absolute bound; the same rounding can be a large fraction
of a very small gross amount.

## Network-leg clamp and merchant remainder

After both fee legs are rounded up, `calculate_split` checks whether their sum
exceeds the gross. If it does, settlement reduces the **network** fee to
`max(gross_amount - platform_fee_amount, 0)`. This preserves the platform
leg and prevents the two fee legs from exceeding the gross. The merchant
amount is then computed as the remaining gross and clamped to a minimum of
zero, so it can never be negative.

Fee rates are individually limited to 5,000 bps (50%), so the platform leg
cannot by itself exceed a valid positive gross amount. If the clamp activates,
the network leg may therefore be lower than its independently ceiling-rounded
value, including zero. Fee legs plus the merchant amount still sum to the
gross.

## Why payments have a 100-unit minimum

The fallback `MIN_PAYMENT_AMOUNT` is 100 base units. The independent rounding
excess of each leg is less than one unit, which at a 100-unit minimum is less
than 1% of the gross (100 bps) per leg; above the minimum its relative effect
is smaller. Without that floor, a one-unit payment and a small legal fee rate
could still round up to a full unit, making the effective fee dramatically
higher than the configured rate.

Governance can override the `min_payment` system parameter, including setting
it below 100. The relative distortion bound from the 100-unit floor therefore
applies when the effective configured minimum is at least 100; a lower
override retains the absolute less-than-one-unit rounding bound but removes
the same relative guarantee.

The minimum does not remove the network-leg clamp: when both independently
rounded legs would exceed the gross, the clamp still applies. It also does
not describe off-chain payout rounding. The effective minimum is enforced by
settlement's payment entry points.

Implementation references: `bettapay_common/src/types.rs`
(`Bps::calculate_fee_ceil`), `settlement_contract/src/payments.rs`
(`calculate_split`), and `settlement_contract/src/policy.rs`
(`MIN_PAYMENT_AMOUNT`).
