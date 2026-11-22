//! Contract-backed invariants for the behaviors originally tracked by issue #723.
//!
//! This module is included by the settlement contract's unit-test module so
//! it can use the shared initialized-contract setup helper.

use super::mock_governance::{MockGovernanceClient, MockGovMode};
use super::setup;
use crate::*;
use soroban_sdk::Bytes;

#[test]
fn empty_wasm_upgrade_is_rejected_without_replacing_contract_code() {
    let (env, client, admins, _) = setup();
    let empty_wasm = Bytes::from_slice(&env, &[]);
    let empty_wasm_hash = env.deployer().upload_contract_wasm(empty_wasm);

    // Complements admin_tests' rejection coverage by checking the contract
    // remains callable and initialized after the real upgrade attempt.
    assert_eq!(
        client.try_upgrade(&admins, &empty_wasm_hash),
        Err(Ok(SettlementError::InvalidWasmInterface.into())),
        "an empty Wasm module must be rejected by the live contract"
    );
    assert!(
        client.supports_interface(&SUPPORTED_INTERFACE_VERSION),
        "the original contract code must remain callable after rejection"
    );
    assert_eq!(
        client.get_admin(),
        admins,
        "the initialized contract state must remain available after rejection"
    );
}

#[test]
fn failed_governance_call_preserves_the_existing_settlement_rule() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let original_rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &original_rule);

    let governance = client.get_governance();
    MockGovernanceClient::new(&env, &governance).set_mode(&MockGovMode::Trap);

    let attempted_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 75,
        settlement_delay_ledger: 1,
        auto_settle: true,
    };
    assert_eq!(
        client.try_set_settlement_rule(&admins, &merchant, &attempted_rule),
        Err(Ok(SettlementError::GovernanceCallFailed)),
        "the live settlement call must surface the failing governance call"
    );

    let stored_rule = client
        .get_settlement_rule(&merchant)
        .expect("the original settlement rule must remain stored");
    assert_eq!(
        (
            stored_rule.platform_fee_bps,
            stored_rule.network_fee_bps,
            stored_rule.settlement_delay_ledger,
            stored_rule.auto_settle,
        ),
        (
            original_rule.platform_fee_bps,
            original_rule.network_fee_bps,
            original_rule.settlement_delay_ledger,
            original_rule.auto_settle,
        ),
        "a failed cross-contract call must leave the stored rule unchanged"
    );
}
