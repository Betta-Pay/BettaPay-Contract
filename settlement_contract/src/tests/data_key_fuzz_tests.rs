//! Fuzz-like regression coverage for decoding contract storage keys.
//!
//! Run with `cargo test -p settlement_contract data_key_decode`.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::panic::{catch_unwind, AssertUnwindSafe};

use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::{FromXdr, ToXdr};
use soroban_sdk::{Address, Bytes, BytesN, Env};

use bettapay_common::storage::CommonDataKey;

use crate::types::DataKey;

fn key_samples(env: &Env) -> Vec<DataKey> {
    let address = Address::generate(env);
    let hash = BytesN::from_array(env, &[0x5a; 32]);
    vec![
        DataKey::Admin,
        DataKey::Initializing,
        DataKey::Governance,
        DataKey::Merchant(address.clone()),
        DataKey::Rule(address.clone()),
        DataKey::ArchivedMerchant(address.clone()),
        DataKey::DefaultRule,
        DataKey::Payment(address, hash.clone()),
        DataKey::ScheduledOperation(hash),
        DataKey::Deployer,
        DataKey::SchemaVersion,
        DataKey::CachedGovRule,
    ]
}

fn assert_decode_does_not_panic(input: &[u8]) {
    let env = Env::default();
    let bytes = Bytes::from_slice(&env, input);
    let result = catch_unwind(AssertUnwindSafe(|| DataKey::from_xdr(&env, &bytes)));
    match result {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {} // Malformed encodings are ordinary decode failures.
        Err(_) => panic!(
            "DataKey XDR decode panicked for input 0x{}",
            input
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
    }
}

#[test]
fn data_key_decode_round_trips_every_variant() {
    let env = Env::default();
    for key in key_samples(&env) {
        let encoded = key.clone().to_xdr(&env);
        let decoded =
            DataKey::from_xdr(&env, &encoded).expect("valid DataKey encoding must decode");
        assert_eq!(decoded.to_xdr(&env), encoded, "DataKey variant changed");
    }

    for key in [
        CommonDataKey::RecoveryAddress,
        CommonDataKey::PendingRecovery,
        CommonDataKey::Paused,
        CommonDataKey::Threshold,
    ] {
        let encoded = key.to_xdr(&env);
        let decoded = CommonDataKey::from_xdr(&env, &encoded)
            .expect("valid CommonDataKey encoding must decode");
        assert_eq!(
            decoded.to_xdr(&env),
            encoded,
            "CommonDataKey variant changed"
        );
    }
}

#[test]
fn data_key_decode_never_panics_for_mutated_valid_encodings() {
    let env = Env::default();
    for key in key_samples(&env) {
        let encoded = key.to_xdr(&env);
        let seed: Vec<u8> = encoded.iter().collect();

        for length in 0..seed.len() {
            assert_decode_does_not_panic(&seed[..length]);
        }

        for index in 0..seed.len() {
            for replacement in [0, u8::MAX, seed[index] ^ 0x80] {
                let mut mutated = seed.clone();
                mutated[index] = replacement;
                assert_decode_does_not_panic(&mutated);
            }
        }

        let mut extended = seed.clone();
        extended.push(0xff);
        assert_decode_does_not_panic(&extended);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn data_key_decode_never_panics_for_arbitrary_bytes(
        input in prop::collection::vec(any::<u8>(), 0..512)
    ) {
        assert_decode_does_not_panic(&input);
    }
}
