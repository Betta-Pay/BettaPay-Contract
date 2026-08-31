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
use soroban_sdk::Env;

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
