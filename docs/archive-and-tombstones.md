# Archive and Tombstone Semantics

## Unregistering a merchant

`unregister_merchant` removes the live `Merchant(Address)` marker and writes an `ArchivedMerchant(Address)` persistent tombstone. The tombstone records that the merchant's existing payment history is orphaned; unregistering does not delete each `Payment(Address, reference)` entry immediately.

The tombstone uses the merchant TTL policy: `MERCHANT_TTL_THRESHOLD` is 14 days in ledgers and `MERCHANT_TTL_BUMP` is 30 days. Unregistering writes the tombstone and extends it to the 30-day bump. The tombstone may therefore expire naturally if the merchant remains unregistered. Payment records have their own retention policy, `PAYMENT_TTL_THRESHOLD` / `PAYMENT_TTL_BUMP`, also 14 days / 30 days. Payment writes and successful single or batch reads refresh payment TTL when the entry is below its threshold.

## Reads while archived

`get_payment_reference` and `get_payments` both call `assert_payments_readable` before looking up records. While the merchant is unregistered, or while its `ArchivedMerchant` tombstone is present, the helper panics with `SettlementError::PaymentOrphaned`. This applies even when the requested reference exists, and it applies to both single-record and batch reads. A missing merchant marker also produces `PaymentOrphaned`, so tombstone expiry does not make an unregistered merchant's history readable.

The read guard does not enumerate or delete orphaned records. Payment entries remain subject to their normal payment TTL and can disappear through ordinary persistent-storage expiry.

## Re-registration

`register_merchant` removes any existing `ArchivedMerchant` tombstone after restoring the merchant marker. Subsequent payment reads are then allowed by `assert_payments_readable`; surviving records from before unregistering and new records are readable, subject to their payment TTL. Re-registration does not restore records that have already expired or been pruned.

## Pruning

`prune_payment` removes the requested payment key after admin authorization. Removing a missing payment is intentionally a no-op, so cleanup jobs can safely retry. Pruning does not remove or bypass an `ArchivedMerchant` tombstone; while the merchant remains archived, payment reads still fail with `PaymentOrphaned`.
