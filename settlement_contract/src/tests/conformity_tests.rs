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

fn settlement_codes() -> [(&'static str, u32); 27] {
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
