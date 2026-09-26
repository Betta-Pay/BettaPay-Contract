//! Cross-contract error-code conformity test for issue #517.
//!
//! Governance and settlement error codes used to be numbered independently,
//! so the same code could mean different things in each contract (e.g. code
//! 12 was `AlreadyPaused` in governance but `InvalidPaymentReference` in
//! settlement). Both enums now derive their discriminants from
//! `bettapay_common::error_codes`, and each contract's own module has a
//! `const _: ()` block asserting its enum matches the registry. This test
//! adds the piece those per-crate checks can't: proof that the two full
//! error tables, read together, never disagree about what a code means.

use crate::errors::SettlementError;
use governance_contract::GovernanceError;

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

use super::{register_governance, setup};

fn governance_codes() -> [(&'static str, u32); 17] {
    [
        (
            "AlreadyInitialized",
            GovernanceError::AlreadyInitialized as u32,
        ),
        ("NotInitialized", GovernanceError::NotInitialized as u32),
        ("Unauthorized", GovernanceError::Unauthorized as u32),
        ("InvalidFeeBps", GovernanceError::InvalidFeeBps as u32),
        ("Paused", GovernanceError::Paused as u32),
        ("InvalidAdmin", GovernanceError::InvalidAdmin as u32),
        (
            "InvalidRecoveryAddress",
            GovernanceError::InvalidRecoveryAddress as u32,
        ),
        (
            "RecoveryNotPending",
            GovernanceError::RecoveryNotPending as u32,
        ),
        (
            "RecoveryDelayActive",
            GovernanceError::RecoveryDelayActive as u32,
        ),
        (
            "InvalidWasmInterface",
            GovernanceError::InvalidWasmInterface as u32,
        ),
        ("InvalidThreshold", GovernanceError::InvalidThreshold as u32),
        (
            "RecoveryAlreadyPending",
            GovernanceError::RecoveryAlreadyPending as u32,
        ),
        ("AlreadyPaused", GovernanceError::AlreadyPaused as u32),
        ("AlreadyUnpaused", GovernanceError::AlreadyUnpaused as u32),
        ("AnchorMissing", GovernanceError::AnchorMissing as u32),
        (
            "InvalidParamValue",
            GovernanceError::InvalidParamValue as u32,
        ),
        ("SameAdmin", GovernanceError::SameAdmin as u32),
    ]
}

fn settlement_codes() -> [(&'static str, u32); 28] {
    [
        (
            "AlreadyInitialized",
            SettlementError::AlreadyInitialized as u32,
        ),
        ("NotInitialized", SettlementError::NotInitialized as u32),
        ("Unauthorized", SettlementError::Unauthorized as u32),
        ("InvalidFeeBps", SettlementError::InvalidFeeBps as u32),
        ("Paused", SettlementError::Paused as u32),
        ("InvalidAdmin", SettlementError::InvalidAdmin as u32),
        (
            "InvalidRecoveryAddress",
            SettlementError::InvalidRecoveryAddress as u32,
        ),
        (
            "RecoveryNotPending",
            SettlementError::RecoveryNotPending as u32,
        ),
        (
            "RecoveryDelayActive",
            SettlementError::RecoveryDelayActive as u32,
        ),
        (
            "ExecutionNotReady",
            SettlementError::ExecutionNotReady as u32,
        ),
        (
            "OperationNotScheduled",
            SettlementError::OperationNotScheduled as u32,
        ),
        (
            "OperationAlreadyScheduled",
            SettlementError::OperationAlreadyScheduled as u32,
        ),
        (
            "InvalidWasmInterface",
            SettlementError::InvalidWasmInterface as u32,
        ),
        ("InvalidThreshold", SettlementError::InvalidThreshold as u32),
        (
            "RecoveryAlreadyPending",
            SettlementError::RecoveryAlreadyPending as u32,
        ),
        ("AlreadyPaused", SettlementError::AlreadyPaused as u32),
        ("AlreadyUnpaused", SettlementError::AlreadyUnpaused as u32),
        ("MerchantExists", SettlementError::MerchantExists as u32),
        ("MerchantMissing", SettlementError::MerchantMissing as u32),
        (
            "DuplicatePaymentReference",
            SettlementError::DuplicatePaymentReference as u32,
        ),
        (
            "MerchantRuleNotSet",
            SettlementError::MerchantRuleNotSet as u32,
        ),
        ("ZeroAddress", SettlementError::ZeroAddress as u32),
        (
            "InvalidPaymentReference",
            SettlementError::InvalidPaymentReference as u32,
        ),
        (
            "InvalidSettlementDelay",
            SettlementError::InvalidSettlementDelay as u32,
        ),
        (
            "InvalidGovernance",
            SettlementError::InvalidGovernance as u32,
        ),
        ("AmountOverflow", SettlementError::AmountOverflow as u32),
        ("PaymentOrphaned", SettlementError::PaymentOrphaned as u32),
        (
            "OperationHashCollision",
            SettlementError::OperationHashCollision as u32,
        ),
    ]
}

#[test]
fn shared_registry_codes_are_identical_in_both_contracts() {
    for &(name, code) in bettapay_common::error_codes::SHARED_CODES {
        // Settlement implements every shared concept, so it must carry every
        // shared code. Governance only carries the shared codes for features it
        // actually exposes (e.g. it has no scheduled-operation timelock, so it
        // intentionally omits the `ExecutionNotReady` family); any shared code
        // governance *does* declare must still match the registry.
        let settle = settlement_codes()
            .into_iter()
            .find(|&(n, _)| n == name)
            .unwrap_or_else(|| panic!("settlement_contract has no `{name}` variant"));
        assert_eq!(
            settle.1, code,
            "settlement `{name}` drifted from the registry"
        );
        if let Some((_gov_name, gov_code)) =
            governance_codes().into_iter().find(|&(n, _)| n == name)
        {
            assert_eq!(
                gov_code, code,
                "governance `{name}` drifted from the registry"
            );
        }
    }
}

#[test]
fn governance_and_settlement_error_codes_never_collide() {
    for &(gov_name, gov_code) in governance_codes().iter() {
        for &(settle_name, settle_code) in settlement_codes().iter() {
            if gov_code == settle_code {
                assert_eq!(
                    gov_name, settle_name,
                    "code {gov_code} means `{gov_name}` in governance_contract but \
                     `{settle_name}` in settlement_contract",
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Zero-reference rejection (issue: Add all-zero reference rejection test)
// ---------------------------------------------------------------------------

/// An all-zero 32-byte reference is reserved and must be rejected with
/// `InvalidPaymentReference`. A non-zero reference must succeed.
#[test]
fn zero_ref_store_fails_invalid_payment_reference() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let zero_ref = BytesN::from_array(&env, &[0u8; 32]);
    let result = client.try_store_payment_reference(&merchant, &zero_ref, &1_000);
    assert!(
        matches!(
            result,
            Err(Ok(e)) if e == soroban_sdk::Error::from_contract_error(307)
        ),
        "all-zero reference must fail with InvalidPaymentReference (307)"
    );

    let nonzero_ref = BytesN::from_array(&env, &[1u8; 32]);
    client.store_payment_reference(&merchant, &nonzero_ref, &1_000);
}

// ---------------------------------------------------------------------------
// Batch-too-large boundary (issue: Add batch-too-large boundary test at 100 vs 101)
// ---------------------------------------------------------------------------

/// Exactly 100 refs is within the cap and must succeed; 101 must fail with
/// `BatchTooLarge`.
#[test]
fn batch_at_cap_succeeds_and_above_cap_fails() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // Build 100 distinct refs and store them.
    let mut refs_100 = soroban_sdk::Vec::new(&env);
    for i in 0u8..100 {
        let mut bytes = [0u8; 32];
        bytes[31] = i;
        bytes[0] = 1; // ensure non-zero
        let reference = BytesN::from_array(&env, &bytes);
        client.store_payment_reference(&merchant, &reference, &1_000);
        refs_100.push_back(reference);
    }

    // 100 refs — must succeed.
    client.get_payments(&merchant, &refs_100);

    // Build a 101-element vec by duplicating the last entry (get_payments
    // accepts duplicates; we only need to exceed the batch cap).
    let extra = refs_100.get(0).unwrap();
    let mut refs_101 = refs_100.clone();
    refs_101.push_back(extra);

    let result = client.try_get_payments(&merchant, &refs_101);
    assert!(
        matches!(
            result,
            Err(Ok(e)) if e == soroban_sdk::Error::from_contract_error(314)
        ),
        "101-element batch must fail with BatchTooLarge (314)"
    );
}

// ---------------------------------------------------------------------------
// MIN_PAYMENT_AMOUNT dust boundary (issue #787)
// ---------------------------------------------------------------------------

/// Amount 99 is one stroop below the dust floor and must fail with
/// `AmountTooSmall` (313); amount 100 is exactly at the floor and must
/// succeed.  Pins `MIN_PAYMENT_AMOUNT == 100` at this boundary so any
/// accidental change to the constant is caught immediately.
#[test]
fn min_payment_boundary_99_fails_100_succeeds() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let reference_99 = BytesN::from_array(&env, &{
        let mut b = [0u8; 32];
        b[0] = 0x99;
        b
    });
    let result_99 = client.try_store_payment_reference(&merchant, &reference_99, &99i128);
    assert!(
        matches!(
            result_99,
            Err(Ok(e)) if e == soroban_sdk::Error::from_contract_error(313)
        ),
        "amount 99 must fail with AmountTooSmall (313)"
    );

    let reference_100 = BytesN::from_array(&env, &{
        let mut b = [0u8; 32];
        b[0] = 0xaa;
        b
    });
    client.store_payment_reference(&merchant, &reference_100, &100i128);
}

#[test]
fn contract_specific_codes_stay_in_their_reserved_range() {
    bettapay_common::error_codes::assert_no_code_collisions(
        &governance_codes(),
        bettapay_common::error_codes::GOVERNANCE_RANGE_START,
    );
    bettapay_common::error_codes::assert_no_code_collisions(
        &settlement_codes(),
        bettapay_common::error_codes::SETTLEMENT_RANGE_START,
    );
}

// ---------------------------------------------------------------------------
// State-bloat benchmark: 1000 sequential payments (issue #773)
// ---------------------------------------------------------------------------

/// Stores 1000 sequential payment references for one merchant and reports the
/// ledger footprint. Benchmark only — no production change.
///
/// The test asserts the run completes without trapping; the stored count and
/// the host budget footprint are reported so rent/state-growth cost of mass
/// payment creation stays visible.
#[test]
fn bloat_bench_stores_1000_sequential_payments() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    for i in 0..1000u32 {
        let mut bytes = [0u8; 32];
        bytes[0] = 1; // guarantee non-zero reference
        bytes[28..32].copy_from_slice(&i.to_be_bytes());
        let reference = BytesN::from_array(&env, &bytes);
        client.store_payment_reference(&merchant, &reference, &1_000);
    }

    // Spot-check first and last records resolve within the merchant namespace.
    let mut first_bytes = [0u8; 32];
    first_bytes[0] = 1;
    first_bytes[28..32].copy_from_slice(&0u32.to_be_bytes());
    let first_ref = BytesN::from_array(&env, &first_bytes);
    let mut last_bytes = [0u8; 32];
    last_bytes[0] = 1;
    last_bytes[28..32].copy_from_slice(&999u32.to_be_bytes());
    let last_ref = BytesN::from_array(&env, &last_bytes);
    let empty_signers = soroban_sdk::Vec::new(&env);
    assert!(client
        .get_payment_reference(&merchant, &first_ref, &empty_signers)
        .is_some());
    assert!(client
        .get_payment_reference(&merchant, &last_ref, &empty_signers)
        .is_some());

    // Report the ledger footprint: 1000 persistent `Payment` entries now live
    // under this merchant. `env.budget().print()` emits the host budget
    // consumption (CPU/memory, entry/ledger footprint) to test output for
    // rent-cost inspection; reaching this line proves no trap occurred.
    env.budget().print();
}

// ---------------------------------------------------------------------------
// Gas snapshot: store_payment_reference baseline (issue #801)
// ---------------------------------------------------------------------------

/// Gas snapshot test for `store_payment_reference` with a local merchant rule.
///
/// Records the host resource consumption (CPU instructions and memory bytes)
/// for storing a payment reference under a local rule to establish a baseline
/// and guard against silent gas regressions.
#[test]
fn store_payment_gas_snapshot() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);

    let reference = BytesN::from_array(&env, &[7u8; 32]);
    let amount = 10_000i128;

    env.budget().reset_unlimited();

    client.store_payment_reference(&merchant, &reference, &amount);

    let cpu = env.budget().cpu_instruction_cost();
    let mem = env.budget().memory_bytes_cost();

    env.budget().print();

    assert!(cpu > 0, "CPU instruction count must be positive");
    assert!(mem > 0, "Memory byte count must be positive");
    assert!(cpu < 1_500_000, "CPU instructions ({cpu}) exceeded baseline bound");
    assert!(mem < 300_000, "Memory bytes ({mem}) exceeded baseline bound");
}

