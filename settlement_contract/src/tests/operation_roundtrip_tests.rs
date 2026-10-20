//! Property coverage for scheduled-operation XDR identity (issue #918).

use crate::{Operation, SettlementRule};
use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::{FromXdr, ToXdr};
use soroban_sdk::{Address, BytesN, Env};

fn operation_for_case(
    env: &Env,
    variant: u8,
    payload_len: usize,
    platform_fee_bps: u32,
    network_fee_bps: u32,
    settlement_delay_ledger: u32,
    auto_settle: bool,
    wasm_hash: [u8; 32],
) -> Operation {
    let first = Address::generate(env);
    let mut admins = soroban_sdk::Vec::new(env);
    for _ in 0..payload_len {
        admins.push_back(Address::generate(env));
    }
    let rule = SettlementRule {
        platform_fee_bps,
        network_fee_bps,
        settlement_delay_ledger,
        auto_settle,
    };

    match variant {
        0 => Operation::UpdateGovernance(first.clone()),
        1 => Operation::CancelRecovery,
        2 => Operation::TransferAdmin(admins, payload_len as u32),
        3 => Operation::Upgrade(BytesN::from_array(env, &wasm_hash)),
        4 => Operation::RegisterMerchant(first.clone()),
        5 => Operation::UnregisterMerchant(first.clone()),
        6 => Operation::SetSettlementRule(first.clone(), rule.clone()),
        7 => Operation::ClearSettlementRule(first),
        _ => Operation::SetDefaultRule(rule),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn every_operation_round_trips_with_a_stable_storage_key(
        variant in 0u8..=8,
        payload_len in 0usize..=4,
        platform_fee_bps in any::<u32>(),
        network_fee_bps in any::<u32>(),
        settlement_delay_ledger in any::<u32>(),
        auto_settle in any::<bool>(),
        wasm_hash in prop::array::uniform32(any::<u8>()),
    ) {
        let env = Env::default();
        let operation = operation_for_case(
            &env,
            variant,
            payload_len,
            platform_fee_bps,
            network_fee_bps,
            settlement_delay_ledger,
            auto_settle,
            wasm_hash,
        );
        let encoded = operation.clone().to_xdr(&env);
        let before_hash: BytesN<32> = env.crypto().sha256(&encoded).into();
        let decoded = Operation::from_xdr(&env, &encoded)
            .expect("every Operation variant must deserialize after serialization");
        let reencoded = decoded.to_xdr(&env);
        let after_hash: BytesN<32> = env.crypto().sha256(&reencoded).into();

        // Canonical XDR equality is equality of the contracttype value here;
        // it also covers every payload field without requiring Operation to
        // implement PartialEq.
        prop_assert_eq!(reencoded, encoded);
        prop_assert_eq!(after_hash, before_hash);
    }
}
