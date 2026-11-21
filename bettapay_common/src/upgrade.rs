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

use soroban_sdk::{contracttype, Bytes, BytesN, Env, IntoVal, Symbol, Vec};

#[derive(Clone)]
#[contracttype]
enum ProbeStorageKey {
    Nonce,
}

/// Deploys a probe instance of `new_wasm_hash` and verifies it supports the
/// BettaPay interface.
///
/// The probe salt combines the candidate hash, current ledger sequence, and a
/// per-contract nonce so repeated probes cannot collide, including within one
/// ledger. `supports_interface` is then invoked on the probe with `version`
/// as the only argument.
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
    let nonce: u64 = env
        .storage()
        .instance()
        .get(&ProbeStorageKey::Nonce)
        .unwrap_or(0);
    let next_nonce = nonce
        .checked_add(1)
        .unwrap_or_else(|| panic!("upgrade probe nonce overflow"));
    env.storage()
        .instance()
        .set(&ProbeStorageKey::Nonce, &next_nonce);

    let mut salt_preimage = [0u8; 44];
    salt_preimage[..32].copy_from_slice(&new_wasm_hash.to_array());
    salt_preimage[32..36].copy_from_slice(&env.ledger().sequence().to_be_bytes());
    salt_preimage[36..].copy_from_slice(&nonce.to_be_bytes());
    let salt = env.crypto().sha256(&Bytes::from_array(env, &salt_preimage));
    let probe = env
        .deployer()
        .with_current_contract(new_wasm_hash.clone())
        .deploy(salt);

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

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Ledger;
    use soroban_sdk::{contract, testutils::Address as _};

    #[contract]
    struct DummyContract;

    #[test]
    fn probing_the_same_wasm_does_not_collide() {
        let env = Env::default();
        let contract_id = env.register_contract(None, DummyContract);
        let wasm_hash = env
            .deployer()
            .upload_contract_wasm(Bytes::from_slice(&env, &[]));

        env.as_contract(&contract_id, || {
            assert!(!probe_supports_interface(&env, &wasm_hash, 1));
            assert!(!probe_supports_interface(&env, &wasm_hash, 1));
            env.ledger().with_mut(|ledger| ledger.sequence_number += 1);
            assert!(!probe_supports_interface(&env, &wasm_hash, 1));
        });
    }
}
