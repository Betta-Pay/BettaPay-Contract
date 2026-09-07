//! Shared Wasm upgrade probe helper.
//!
//! Both contracts replace their running executable via
//! `env.deployer().update_current_contract_wasm`, which accepts any
//! `BytesN<32>` and blindly trusts it. To keep the two contracts from
//! drifting, the probe-and-verify step that must run **before** the code swap
//! lives here: deploy a throwaway probe instance of the candidate Wasm and
//! ask it whether it implements the required interface version.
//!
//! See `probe_supports_interface` for the exact contract of the check and for
//! the one case (a never-uploaded Wasm hash) that the protocol does not let
//! contract code turn into a typed error.

use soroban_sdk::{BytesN, Env, IntoVal, Symbol, Vec};

/// Deploys a probe instance of `new_wasm_hash` and verifies it supports the
/// BettaPay interface.
///
/// The probe is deployed with the candidate hash reused as the deploy salt,
/// so its address is deterministic and collision-free. `supports_interface`
/// is then invoked on the probe with `version` as the only argument.
///
/// # Returns
///
/// - `true` — the probe deployed and its `supports_interface(version)` call
///   returned `true`.
/// - `false` — the probe deployed but the call failed (missing export, wrong
///   signature), or the probe reported it does not support the interface.
///   Callers raise their contract-specific `InvalidWasmInterface` variant.
///
/// # Never-uploaded hashes
///
/// If `new_wasm_hash` was never uploaded (no `upload_contract_wasm` before
/// this call), the probe deployment itself fails with a host-level
/// `Storage`/`MissingValue` error ("Wasm does not exist") that traps the
/// transaction before this function can return. Protocol 21 exposes no way
/// for contract code to test whether a Wasm hash exists, so that case
/// surfaces as a host error rather than as `false` here. Callers should
/// document that behavior in tests; it is the protocol's own guard against
/// upgrading to Wasm that was never staged on-chain.
pub fn probe_supports_interface(env: &Env, new_wasm_hash: &BytesN<32>, version: u32) -> bool {
    let probe = env
        .deployer()
        .with_current_contract(new_wasm_hash.clone())
        .deploy(new_wasm_hash.clone());

    let version_args: Vec<u32> = soroban_sdk::vec![env, version];
    match env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &probe,
        &Symbol::new(env, "supports_interface"),
        version_args.into_val(env),
    ) {
        Ok(Ok(supports)) => supports,
        _ => false,
    }
}
