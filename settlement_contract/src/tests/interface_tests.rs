//! Tests for `supports_interface` — issue #48.
//!
//! These tests pin the version semantics of `supports_interface` so that:
//!
//! 1. Exactly one version (`SUPPORTED_INTERFACE_VERSION`, currently 1) is
//!    acknowledged.
//! 2. All other versions — zero, adjacent values, and a large sentinel — are
//!    explicitly rejected.
//!
//! The `upgrade` flow depends on the probe returning `true` only for the
//! current interface version; a change to that behaviour must be deliberate
//! and reflected here.

use crate::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env};

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// supports_interface
// ---------------------------------------------------------------------------

/// Version 1 is the current interface version; `supports_interface(1)` must
/// return `true`.
#[test]
fn supports_interface_returns_true_for_current_version() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    assert!(
        client.supports_interface(&SUPPORTED_INTERFACE_VERSION),
        "supports_interface must return true for the current interface version ({})",
        SUPPORTED_INTERFACE_VERSION,
    );
}

/// Version 0 has never been a valid interface version; it must be rejected.
#[test]
fn supports_interface_returns_false_for_version_zero() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&0u32),
        "supports_interface must return false for version 0 (never a valid version)",
    );
}

/// Version 2 is a hypothetical future version that this Wasm does not yet
/// implement; it must be rejected so callers can distinguish old from new.
#[test]
fn supports_interface_returns_false_for_unknown_future_version() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&(SUPPORTED_INTERFACE_VERSION + 1)),
        "supports_interface must return false for a future version not yet implemented",
    );
}

/// A large sentinel value must also be rejected — the function must not
/// degenerate into an always-true stub (issue #48).
#[test]
fn supports_interface_returns_false_for_large_sentinel() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&u32::MAX),
        "supports_interface must return false for a large out-of-range version",
    );
}

// ---------------------------------------------------------------------------
// Pause matrix: upgrade exempt, payment blocked (issue: Add pause-allows-upgrade
// test; Add pause-blocks-payment test)
// ---------------------------------------------------------------------------

/// While the contract is paused, `upgrade` must NOT be blocked by the pause
/// guard (Paused = 5). It may fail for other reasons (e.g. interface check),
/// but the error must not be `Paused`. `store_payment_reference` in the same
/// paused state must be blocked with `Paused`.
#[test]
fn pause_allows_upgrade_and_blocks_payment() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    client.pause(&admins);

    // Upload empty bytes — produces a hash for a wasm with no exports.
    // upgrade must fail with InvalidWasmInterface (13), not Paused (5).
    let empty_wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let bad_hash = env.deployer().upload_contract_wasm(empty_wasm);
    let upgrade_result = client.try_upgrade(&admins, &bad_hash);
    match &upgrade_result {
        Err(Ok(e)) => {
            assert_ne!(
                *e,
                soroban_sdk::Error::from_contract_error(5),
                "upgrade must not be blocked by Paused (5) while paused"
            );
        }
        Ok(_) => {}
        Err(Err(_)) => {}
    }

    // store_payment_reference must be blocked with Paused (5).
    let reference = BytesN::from_array(&env, &[2u8; 32]);
    let pay_result = client.try_store_payment_reference(&merchant, &reference, &1_000);
    assert!(
        matches!(
            pay_result,
            Err(Ok(soroban_sdk::Error::from_contract_error(5)))
        ),
        "store_payment_reference must fail with Paused (5) while paused"
    );
}

/// Paused contract blocks `store_payment_reference` with `Paused`; after
/// unpause the same call must succeed.
#[test]
fn pause_blocks_payment_and_unpaused_succeeds() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);
    client.pause(&admins);

    let reference = BytesN::from_array(&env, &[3u8; 32]);
    let result = client.try_store_payment_reference(&merchant, &reference, &1_000);
    assert!(
        matches!(
            result,
            Err(Ok(soroban_sdk::Error::from_contract_error(5)))
        ),
        "store_payment_reference must fail with Paused (5) while paused"
    );

    client.unpause(&admins);
    client.store_payment_reference(&merchant, &reference, &1_000);
}
