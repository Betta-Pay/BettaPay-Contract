//! Cross-contract lifecycle integration tests.
//!
//! These tests exercise the full end-to-end interaction between the
//! `governance_contract` and `settlement_contract` deployed side-by-side in
//! the same Soroban test environment. The goal is to verify the cross-contract
//! interface (fee-config propagation, governance-address validation, fee
//! ceiling enforcement, coordinated admin operations, etc.) rather than each
//! contract in isolation — that coverage is already provided by the unit-test
//! suites in `admin_tests.rs` and the governance crate's own modules.

use crate::*;
use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol, TryFromVal, Vec};

use bettapay_common::constants::{BPS_DENOMINATOR, RECOVERY_DELAY_SECONDS};
use bettapay_common::events::AdminTransferred;

use governance_contract::{
    FeeConfig as GovFeeConfig, GovernanceContract, GovernanceContractClient,
};

/// Deploys a *real* `GovernanceContract` and initializes it with the supplied
/// admin set. Returns the test environment plus the governance client and the
/// admin vector for convenience.
#[allow(dead_code)]
pub fn setup_governance() -> (
    Env,
    GovernanceContractClient<'static>,
    Vec<Address>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery_address);
    (env, client, admins, recovery_address)
}

/// Deploys both contracts in the same `Env`, initializes governance, and
/// wires settlement's governance pointer to the real instance.
///
/// Returns: `(env, gov_client, gov_admins, settlement_client, settlement_admins, merchant)`
pub fn setup_both() -> (
    Env,
    GovernanceContractClient<'static>,
    Vec<Address>,
    SettlementContractClient<'static>,
    Vec<Address>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let gov_admin = Address::generate(&env);
    let gov_recovery = Address::generate(&env);
    let gov_admins = soroban_sdk::vec![&env, gov_admin.clone()];
    let gov_id = env.register_contract(None, GovernanceContract);
    let gov_client = GovernanceContractClient::new(&env, &gov_id);
    let deployer = Address::generate(&env);
    gov_client.init(&deployer, &gov_admins, &1, &gov_recovery);

    let settle_admin = Address::generate(&env);
    let settle_recovery = Address::generate(&env);
    let settle_admins = soroban_sdk::vec![&env, settle_admin.clone()];
    let merchant = Address::generate(&env);
    let settle_id = env.register_contract(None, SettlementContract);
    let settle_client = SettlementContractClient::new(&env, &settle_id);
    let deployer = Address::generate(&env);
    settle_client.init(&deployer, &settle_admins, &1, &gov_id, &settle_recovery);

    (
        env,
        gov_client,
        gov_admins,
        settle_client,
        settle_admins,
        merchant,
    )
}

// ---------------------------------------------------------------------------
// Initialization & cross-contract wiring
// ---------------------------------------------------------------------------

#[test]
fn settlement_init_accepts_real_governance_address() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, _merchant) = setup_both();

    assert!(gov_client.is_initialized());
    assert!(settle_client.is_initialized());
    assert_eq!(settle_client.get_admin(), settle_admins);
    assert_eq!(settle_client.get_governance(), gov_client.address);
    assert_eq!(gov_client.get_admin(), gov_admins);
    let _ = env;
}

#[test]
fn settlement_falls_back_to_bootstrap_without_governance_fee_config() {
    let (_env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let split = settle_client.calculate_fee_split(&merchant, &10_000);
    // Bootstrap default is 100 bps platform, 5 network — fee = 105, merchant = 9895.
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 5);
    assert_eq!(split.merchant_amount, 9_895);
}

// ---------------------------------------------------------------------------
// Governance fee-config propagation to settlement
// ---------------------------------------------------------------------------

#[test]
fn governance_fee_config_propagates_via_effective_settlement_rule() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();

    settle_client.register_merchant(&settle_admins, &merchant);

    let cfg = GovFeeConfig {
        platform_fee_bps: 250,
        network_fee_bps: 50,
    };
    gov_client.set_fee_config(&gov_admins, &cfg);

    let split = settle_client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 250, "250 bps of 10_000");
    assert_eq!(split.network_fee_amount, 50, "50 bps of 10_000");
    assert_eq!(split.merchant_amount, 9_700);

    let stored = gov_client.get_fee_config().unwrap();
    assert_eq!(stored.platform_fee_bps, 250);
    assert_eq!(stored.network_fee_bps, 50);
    let _ = env;
}

#[test]
fn governance_fee_config_acts_as_ceiling_for_merchant_rules() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    gov_client.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 500,
            network_fee_bps: 500,
        },
    );

    let ok_rule = SettlementRule {
        platform_fee_bps: 400,
        network_fee_bps: 400,
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    settle_client.set_settlement_rule(&settle_admins, &merchant, &ok_rule);

    let bad_rule = SettlementRule {
        platform_fee_bps: 600,
        network_fee_bps: 100,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let result = settle_client.try_set_settlement_rule(&settle_admins, &merchant, &bad_rule);
    assert!(
        result.is_err(),
        "rule exceeding governance ceiling must panic"
    );

    let _ = env;
}

#[test]
fn global_default_rule_respects_governance_fee_ceiling() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, _merchant) = setup_both();

    gov_client.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 200,
            network_fee_bps: 100,
        },
    );

    let ok_default = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 80,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    settle_client.set_default_rule(&settle_admins, &ok_default);

    let bad_default = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 50,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    let result = settle_client.try_set_default_rule(&settle_admins, &bad_default);
    assert!(
        result.is_err(),
        "default exceeding governance ceiling must panic"
    );

    let _ = env;
}

#[test]
fn stored_payment_record_uses_propagated_governance_fees_when_no_explicit_rule() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    gov_client.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 200,
            network_fee_bps: 100,
        },
    );

    let payment_ref = BytesN::<32>::from_array(&env, &[7u8; 32]);
    let amount: i128 = 10_000;
    let split = settle_client.store_payment_reference(&merchant, &payment_ref, &amount);

    assert_eq!(split.platform_fee_amount, 200);
    assert_eq!(split.network_fee_amount, 100);
    assert_eq!(split.merchant_amount, 9_700);

    let record = settle_client
        .get_payment_reference(&merchant, &payment_ref)
        .expect("payment record must exist");
    assert_eq!(record.amount, amount);
    assert_eq!(record.platform_fee_amount, 200);
    assert_eq!(record.network_fee_amount, 100);
    assert_eq!(record.merchant_amount, 9_700);
    assert_eq!(record.platform_fee_bps, 200);
    assert_eq!(record.network_fee_bps, 100);
}

// ---------------------------------------------------------------------------
// `update_governance` re-wires settlement to a new governance instance
// ---------------------------------------------------------------------------

#[test]
fn update_governance_switches_fee_source_to_new_instance() {
    let env = Env::default();
    env.mock_all_auths();

    let gov_admin = Address::generate(&env);
    let gov_recovery = Address::generate(&env);
    let gov_admins = soroban_sdk::vec![&env, gov_admin.clone()];

    let old_gov_id = env.register_contract(None, GovernanceContract);
    let old_gov = GovernanceContractClient::new(&env, &old_gov_id);
    let deployer = Address::generate(&env);
    old_gov.init(&deployer, &gov_admins, &1, &gov_recovery);
    old_gov.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 20,
        },
    );

    let new_gov_id = env.register_contract(None, GovernanceContract);
    let new_gov = GovernanceContractClient::new(&env, &new_gov_id);
    let deployer = Address::generate(&env);
    new_gov.init(&deployer, &gov_admins, &1, &gov_recovery);
    new_gov.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 500,
            network_fee_bps: 50,
        },
    );

    let settle_admin = Address::generate(&env);
    let settle_recovery = Address::generate(&env);
    let settle_admins = soroban_sdk::vec![&env, settle_admin.clone()];
    let merchant = Address::generate(&env);
    let settle_id = env.register_contract(None, SettlementContract);
    let settle_client = SettlementContractClient::new(&env, &settle_id);
    let deployer = Address::generate(&env);
    settle_client.init(&deployer, &settle_admins, &1, &old_gov_id, &settle_recovery);
    settle_client.register_merchant(&settle_admins, &merchant);

    let before = settle_client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(before.platform_fee_amount, 100);

    settle_client.update_governance(&settle_admins, &new_gov_id);

    let after = settle_client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(
        after.platform_fee_amount, 500,
        "must use new governance BPS"
    );
    assert_eq!(after.network_fee_amount, 50, "must use new governance BPS");
}

// ---------------------------------------------------------------------------
// Pause coordination — each contract's pause flag is independent
// ---------------------------------------------------------------------------

#[test]
fn settlement_and_governance_pause_flags_are_independent() {
    let (_env, gov_client, gov_admins, settle_client, settle_admins, _merchant) = setup_both();

    assert!(!gov_client.is_paused());
    assert!(!settle_client.is_paused());

    settle_client.pause(&settle_admins);
    assert!(settle_client.is_paused());
    assert!(!gov_client.is_paused());

    gov_client.pause(&gov_admins);
    assert!(gov_client.is_paused());
    assert!(settle_client.is_paused());

    settle_client.unpause(&settle_admins);
    assert!(!settle_client.is_paused());
    assert!(gov_client.is_paused());

    gov_client.unpause(&gov_admins);
    assert!(!gov_client.is_paused());
    assert!(!settle_client.is_paused());
}

#[test]
fn pausing_settlement_does_not_block_governance_writes_and_vice_versa() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    settle_client.pause(&settle_admins);
    gov_client.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 300,
            network_fee_bps: 60,
        },
    );
    assert_eq!(gov_client.get_fee_config().unwrap().platform_fee_bps, 300);
    settle_client.unpause(&settle_admins);

    gov_client.pause(&gov_admins);
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 40,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    settle_client.set_default_rule(&settle_admins, &rule);
    let stored = settle_client.get_default_rule().unwrap();
    assert_eq!(stored.platform_fee_bps, 200);
    let _ = env;
}

// ---------------------------------------------------------------------------
// Recovery lifecycle — both contracts support the same recovery flow
// ---------------------------------------------------------------------------

#[test]
fn recovery_flows_execute_independently_on_both_contracts() {
    let (env, gov_client, _gov_admins, settle_client, _settle_admins, _merchant) = setup_both();

    let new_gov_admin = Address::generate(&env);
    let new_settle_admin = Address::generate(&env);

    gov_client.initiate_recovery(&new_gov_admin);
    settle_client.initiate_recovery(&new_settle_admin);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);

    gov_client.execute_recovery();
    settle_client.execute_recovery();

    assert_eq!(
        gov_client.get_admin(),
        soroban_sdk::vec![&env, new_gov_admin]
    );
    assert_eq!(
        settle_client.get_admin(),
        soroban_sdk::vec![&env, new_settle_admin]
    );
}

#[test]
fn recovery_events_follow_shared_convention_on_both_contracts() {
    let (env, gov_client, _gov_admins, settle_client, _settle_admins, _merchant) = setup_both();

    let new_gov = Address::generate(&env);
    let new_settle = Address::generate(&env);

    gov_client.initiate_recovery(&new_gov);
    settle_client.initiate_recovery(&new_settle);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);

    gov_client.execute_recovery();
    settle_client.execute_recovery();

    let events = env.events().all();
    let mut found_gov = false;
    let mut found_settle = false;
    for i in 0..events.len() {
        let (_contract, topics, data) = events.get(i).unwrap();
        if topics.is_empty() {
            continue;
        }
        let sym = Symbol::from_val(&env, &topics.get(0).unwrap());
        if sym != Symbol::new(&env, "recovery_executed") {
            continue;
        }
        if let Ok(payload) = AdminTransferred::try_from_val(&env, &data) {
            if payload.new_admin == new_gov {
                found_gov = true;
            }
            if payload.new_admin == new_settle {
                found_settle = true;
            }
        }
    }
    assert!(
        found_gov,
        "governance recovery_executed matches shared event shape"
    );
    assert!(
        found_settle,
        "settlement recovery_executed matches shared event shape"
    );
}

// ---------------------------------------------------------------------------
// Admin transfer across both contracts
// ---------------------------------------------------------------------------

#[test]
fn admin_transfer_is_independent_and_preserves_other_contracts_state() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);
    let rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 30,
        settlement_delay_ledger: 4,
        auto_settle: false,
    };
    settle_client.set_settlement_rule(&settle_admins, &merchant, &rule);

    let new_gov_admin = Address::generate(&env);
    gov_client.transfer_admin(
        &gov_admins,
        &soroban_sdk::vec![&env, new_gov_admin.clone()],
        &1,
    );
    assert_eq!(
        gov_client.get_admin(),
        soroban_sdk::vec![&env, new_gov_admin]
    );

    let new_settle_admin = Address::generate(&env);
    settle_client.transfer_admin(
        &settle_admins,
        &soroban_sdk::vec![&env, new_settle_admin.clone()],
        &1,
    );
    assert_eq!(
        settle_client.get_admin(),
        soroban_sdk::vec![&env, new_settle_admin]
    );

    let stored = settle_client.get_settlement_rule(&merchant).unwrap();
    assert_eq!(stored.platform_fee_bps, 150);
    assert_eq!(stored.network_fee_bps, 30);
    assert_eq!(stored.settlement_delay_ledger, 4);
}

// ---------------------------------------------------------------------------
// Governance anchors / system params do not interfere with settlement storage.
// ---------------------------------------------------------------------------

#[test]
fn governance_anchors_and_system_params_do_not_interfere_with_settlement() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    gov_client.upsert_anchor(&gov_admins, &asset, &anchor);
    gov_client.update_system_param(&gov_admins, &Symbol::new(&env, "max_pay"), &5_000_000);

    assert_eq!(gov_client.get_anchor(&asset), Some(anchor));
    assert_eq!(
        gov_client.get_system_param(&Symbol::new(&env, "max_pay")),
        Some(5_000_000)
    );

    // Bootstrap defaults still apply on the settlement side.
    let split = settle_client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
}

// ---------------------------------------------------------------------------
// Multisig threshold operations work independently across both contracts.
// ---------------------------------------------------------------------------

#[test]
fn multisig_threshold_works_independently_on_both_contracts() {
    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let gov_recovery = Address::generate(&env);
    let settle_recovery = Address::generate(&env);

    let gov_admins = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let gov_id = env.register_contract(None, GovernanceContract);
    let gov_client = GovernanceContractClient::new(&env, &gov_id);
    let deployer = Address::generate(&env);
    gov_client.init(&deployer, &gov_admins, &2, &gov_recovery);

    let settle_admins = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let settle_id = env.register_contract(None, SettlementContract);
    let settle_client = SettlementContractClient::new(&env, &settle_id);
    let deployer = Address::generate(&env);
    settle_client.init(&deployer, &settle_admins, &2, &gov_id, &settle_recovery);

    let one_signer = soroban_sdk::vec![&env, a1.clone()];
    let three_signers = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];

    let result_gov = gov_client.try_update_system_param(&one_signer, &Symbol::new(&env, "k"), &1);
    assert!(
        result_gov.is_err(),
        "governance rejects sub-threshold signers"
    );

    let result_settle = settle_client.try_pause(&one_signer);
    assert!(
        result_settle.is_err(),
        "settlement rejects sub-threshold signers"
    );

    // change_threshold requires threshold + 1 = 3 signers.
    gov_client.change_threshold(&three_signers, &3);
    assert_eq!(gov_client.get_threshold(), 3);

    settle_client.change_threshold(&three_signers, &3);
    assert_eq!(settle_client.get_threshold(), 3);
}

// ---------------------------------------------------------------------------
// Full end-to-end lifecycle: configure governance, register merchant,
// set rules, store payments, verify splits, batch-read records.
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_configure_governance_then_process_payments() {
    let (env, gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();

    // 1. Governance publishes protocol-wide fee config and anchor list.
    gov_client.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        },
    );
    let usdc = Address::generate(&env);
    let usdc_anchor = Address::generate(&env);
    gov_client.upsert_anchor(&gov_admins, &usdc, &usdc_anchor);

    // 2. Settlement admin tightens the global default rule (below ceiling).
    let global_default = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 30,
        settlement_delay_ledger: 50,
        auto_settle: true,
    };
    settle_client.set_default_rule(&settle_admins, &global_default);

    // 3. Register a merchant and assign them a custom rule.
    settle_client.register_merchant(&settle_admins, &merchant);
    let merchant_rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 20,
        settlement_delay_ledger: 20,
        auto_settle: false,
    };
    settle_client.set_settlement_rule(&settle_admins, &merchant, &merchant_rule);

    // 4. Store several payment references.
    let r1 = BytesN::<32>::from_array(&env, &[1u8; 32]);
    let r2 = BytesN::<32>::from_array(&env, &[2u8; 32]);
    let r3 = BytesN::<32>::from_array(&env, &[3u8; 32]);
    let r4 = BytesN::<32>::from_array(&env, &[4u8; 32]);
    let mut refs = Vec::new(&env);
    refs.push_back(r1.clone());
    refs.push_back(r2.clone());
    refs.push_back(r3.clone());
    refs.push_back(r4.clone());
    let amounts: [i128; 4] = [100_000, 250_000, 50_000, 1_234_567];
    settle_client.store_payment_reference(&merchant, &r1, &amounts[0]);
    settle_client.store_payment_reference(&merchant, &r2, &amounts[1]);
    settle_client.store_payment_reference(&merchant, &r3, &amounts[2]);
    settle_client.store_payment_reference(&merchant, &r4, &amounts[3]);

    // 5. Verify each payment locked in the custom rule BPS & correct ceil-rounding splits.
    let check = |idx: u32, r: BytesN<32>, a: i128| {
        let rec = settle_client.get_payment_reference(&merchant, &r).unwrap();
        assert_eq!(
            rec.platform_fee_bps, 150,
            "payment {idx} uses merchant rule BPS"
        );
        assert_eq!(
            rec.network_fee_bps, 20,
            "payment {idx} uses merchant rule BPS"
        );
        assert_eq!(rec.settlement_delay_ledger, 20);
        assert!(!rec.auto_settle);
        let platform = (a * 150 + 9_999) / 10_000;
        let network = (a * 20 + 9_999) / 10_000;
        let merchant_net = a - platform - network;
        assert_eq!(
            rec.platform_fee_amount, platform,
            "payment {idx} platform fee"
        );
        assert_eq!(rec.network_fee_amount, network, "payment {idx} network fee");
        assert_eq!(
            rec.merchant_amount, merchant_net,
            "payment {idx} merchant net"
        );
    };
    check(0, r1, amounts[0]);
    check(1, r2, amounts[1]);
    check(2, r3, amounts[2]);
    check(3, r4, amounts[3]);

    // 6. Governance anchor and fee config remain independently retrievable.
    assert_eq!(gov_client.get_anchor(&usdc), Some(usdc_anchor));
    let stored_fees = gov_client.get_fee_config().unwrap();
    assert_eq!(stored_fees.platform_fee_bps, 250);
    assert_eq!(stored_fees.network_fee_bps, 50);

    // 7. Batch-read returns consistent ordering & length.
    let records = settle_client.get_payments(&merchant, &refs);
    assert_eq!(records.len(), 4);
    for i in 0..amounts.len() as u32 {
        assert_eq!(records.get(i).unwrap().amount, amounts[i as usize]);
    }
}

// ---------------------------------------------------------------------------
// `MIN_PAYMENT_AMOUNT` floor — see the constant's doc comment for the rationale.
// ---------------------------------------------------------------------------

/// Locks the derivation `MIN_PAYMENT_AMOUNT == BPS_DENOMINATOR / 100`, so that
/// changing either value without revisiting the ceil-rounding argument breaks
/// the build rather than silently widening the fee distortion.
#[test]
fn min_payment_amount_is_derived_from_bps_denominator() {
    assert_eq!(MIN_PAYMENT_AMOUNT, 100);
    assert_eq!(MIN_PAYMENT_AMOUNT, (BPS_DENOMINATOR / 100) as i128);
}

#[test]
#[should_panic(expected = "Error(Contract, #313)")]
fn store_payment_reference_rejects_amount_below_min() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[9u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &(MIN_PAYMENT_AMOUNT - 1));
}

#[test]
fn store_payment_reference_accepts_amount_at_min() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[10u8; 32]);
    let split = settle_client.store_payment_reference(&merchant, &reference, &MIN_PAYMENT_AMOUNT);

    assert_eq!(split.gross_amount, MIN_PAYMENT_AMOUNT);
    assert_eq!(
        split.platform_fee_amount + split.network_fee_amount + split.merchant_amount,
        split.gross_amount,
        "fee legs plus merchant amount must reconstruct the gross amount"
    );
    // Bootstrap default rule applies (100 bps platform, 5 network): at the floor
    // each fee leg rounds up to 1 unit.
    assert_eq!(split.platform_fee_amount, 1);
    assert_eq!(split.network_fee_amount, 1);
    assert_eq!(split.merchant_amount, 98);
}

// ---------------------------------------------------------------------------
// Issue 494: Normalized Error Tests
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn public_calculate_fee_split_matches_ceil_invariant(
        amount in MIN_PAYMENT_AMOUNT..=1_000_000_000i128,
        (platform_fee_bps, network_fee_bps) in
            (5u32..=5_000, 5u32..=5_000)
                .prop_filter("fee sum must fit the denominator", |(platform, network)| {
                    *platform + *network <= BPS_DENOMINATOR
                }),
    ) {
        let (env, _gov_client, gov_admins, settle_client, settle_admins, merchant) = setup_both();
        settle_client.register_merchant(&settle_admins, &merchant);
        let rule = SettlementRule {
            platform_fee_bps,
            network_fee_bps,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        settle_client.set_default_rule(&settle_admins, &rule);

        let split = settle_client.calculate_fee_split(&merchant, &amount);
        let denom = BPS_DENOMINATOR as i128;
        let platform = (amount * platform_fee_bps as i128 + denom - 1) / denom;
        let network = (amount * network_fee_bps as i128 + denom - 1) / denom;

        prop_assert_eq!(split.gross_amount, amount);
        prop_assert_eq!(split.platform_fee_amount, platform);
        prop_assert_eq!(split.network_fee_amount, network);
        prop_assert_eq!(split.merchant_amount, (amount - platform - network).max(0));
        let _ = (env, gov_admins);
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #313)")]
fn calculate_fee_split_rejects_amount_zero() {
    let (_env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);
    settle_client.calculate_fee_split(&merchant, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #313)")]
fn calculate_fee_split_rejects_amount_negative() {
    let (_env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);
    settle_client.calculate_fee_split(&merchant, &-10);
}

#[test]
#[should_panic(expected = "Error(Contract, #313)")]
fn calculate_fee_split_rejects_amount_below_min() {
    let (_env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);
    settle_client.calculate_fee_split(&merchant, &(MIN_PAYMENT_AMOUNT - 1));
}

// ---------------------------------------------------------------------------
// Issue 493: Payment references are scoped per merchant
// ---------------------------------------------------------------------------

/// The same 32-byte reference must be usable by two different merchants:
/// uniqueness is scoped to `(merchant, reference)`, so one merchant can no
/// longer squat on a reference to block another merchant (cross-merchant DoS).
/// Each record must also carry its own merchant attribution.
#[test]
fn cross_merchant_reference_reuse_is_allowed() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    let merchant2 = Address::generate(&env);

    settle_client.register_merchant(&settle_admins, &merchant);
    settle_client.register_merchant(&settle_admins, &merchant2);

    let reference = BytesN::<32>::from_array(&env, &[7u8; 32]);

    // Merchant A stores the reference first.
    settle_client.store_payment_reference(&merchant, &reference, &1_000);
    // Merchant B is free to use the very same reference — no squatting.
    settle_client.store_payment_reference(&merchant2, &reference, &2_000);

    // Reads are scoped to the merchant namespace and records carry ownership.
    let rec_a = settle_client
        .get_payment_reference(&merchant, &reference)
        .unwrap();
    let rec_b = settle_client
        .get_payment_reference(&merchant2, &reference)
        .unwrap();

    assert_eq!(
        rec_a.merchant, merchant,
        "record A must attribute merchant A"
    );
    assert_eq!(
        rec_b.merchant, merchant2,
        "record B must attribute merchant B"
    );
    assert_eq!(rec_a.amount, 1_000);
    assert_eq!(rec_b.amount, 2_000);

    // Batch reads are scoped identically: merchant A only sees its own record.
    let refs = soroban_sdk::vec![&env, reference.clone()];
    let batch_a = settle_client.get_payments(&merchant, &refs);
    assert_eq!(batch_a.len(), 1);
    assert_eq!(batch_a.get(0).unwrap().merchant, merchant);
    assert_eq!(batch_a.get(0).unwrap().amount, 1_000);
}

/// Within a single merchant, the reference stays unique: storing the same
/// reference twice for the same merchant must still be rejected.
#[test]
#[should_panic(expected = "Error(Contract, #303)")]
fn same_merchant_duplicate_reference_is_rejected() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[8u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &1_000);
    // Same merchant + same reference -> duplicate.
    settle_client.store_payment_reference(&merchant, &reference, &2_000);
}

// ---------------------------------------------------------------------------
// Issue 492: Payment-record reads are gated behind merchant auth
// ---------------------------------------------------------------------------

/// A caller that is not the merchant must not be able to read the merchant's
/// payment record. Auth mocking is disabled for the read so the merchant's
/// `require_auth()` ownership check is actually enforced rather than mocked
/// away.
#[test]
fn get_payment_reference_rejects_unauthenticated_caller() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[21u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &1_000);

    // Turn off auth mocking: no authorization entries exist, so the merchant
    // ownership check must reject the read.
    env.set_auths(&[]);
    let result = settle_client.try_get_payment_reference(&merchant, &reference);
    assert!(
        result.is_err(),
        "unauthenticated payment read must be rejected"
    );

    // The batch read is gated identically.
    let refs = soroban_sdk::vec![&env, reference];
    let batch_result = settle_client.try_get_payments(&merchant, &refs);
    assert!(
        batch_result.is_err(),
        "unauthenticated batch read must be rejected"
    );
}

/// The merchant who owns the records can always read them back.
#[test]
fn get_payment_reference_owner_read_works() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[22u8; 32]);
    let split = settle_client.store_payment_reference(&merchant, &reference, &10_000);

    // The merchant's own read succeeds and returns the stored economics.
    let record = settle_client
        .get_payment_reference(&merchant, &reference)
        .expect("owner read must succeed");
    assert_eq!(record.merchant, merchant);
    assert_eq!(record.amount, 10_000);
    assert_eq!(record.merchant_amount, split.merchant_amount);

    // Batch read for the owner works too.
    let refs = soroban_sdk::vec![&env, reference];
    let records = settle_client.get_payments(&merchant, &refs);
    assert_eq!(records.len(), 1);
    assert_eq!(records.get(0).unwrap().amount, 10_000);
}

// ---------------------------------------------------------------------------
// Issue 490: Unregistering a merchant orphans its payment records
// ---------------------------------------------------------------------------

/// A merchant's payment records must stop being readable once the merchant is
/// unregistered — no more post-unregister queries against records that are
/// only waiting out their TTL.
#[test]
fn payments_of_unregistered_merchant_are_orphaned() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[31u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &1_000);

    // While registered, the merchant can read its own record.
    assert!(
        settle_client
            .get_payment_reference(&merchant, &reference)
            .is_some(),
        "registered merchant must be able to read its own payment"
    );

    settle_client.unregister_merchant(&settle_admins, &merchant);
    assert!(!settle_client.is_merchant_registered(&merchant));

    // Post-unregister reads are rejected with PaymentOrphaned (#315).
    let orphaned = soroban_sdk::Error::from_contract_error(SettlementError::PaymentOrphaned as u32);
    let single = settle_client.try_get_payment_reference(&merchant, &reference);
    assert!(
        matches!(single, Err(Ok(ref err)) if *err == orphaned),
        "post-unregister single read must be rejected as orphaned"
    );

    let refs = soroban_sdk::vec![&env, reference];
    let batch = settle_client.try_get_payments(&merchant, &refs);
    assert!(
        matches!(batch, Err(Ok(ref err)) if *err == orphaned),
        "post-unregister batch read must be rejected as orphaned"
    );
}

/// The orphaning must survive re-registration: a re-registered merchant must
/// not be able to resurrect the payment history of its earlier registration.
#[test]
fn reregistered_merchant_cannot_resurrect_orphaned_payments() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[32u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &1_000);

    settle_client.unregister_merchant(&settle_admins, &merchant);
    settle_client.register_merchant(&settle_admins, &merchant);
    assert!(settle_client.is_merchant_registered(&merchant));

    // The tombstone outlives the registration cycle.
    let result = settle_client.try_get_payment_reference(&merchant, &reference);
    assert!(
        matches!(
            result,
            Err(Ok(ref err))
                if *err
                    == soroban_sdk::Error::from_contract_error(
                        SettlementError::PaymentOrphaned as u32
                    )
        ),
        "re-registration must not resurrect orphaned payments"
    );
}

/// The timelocked unregister path (Operation::UnregisterMerchant executed
/// through the admin timelock) must orphan payments exactly like the direct
/// unregister_merchant entry point.
#[test]
fn timelocked_unregister_also_orphans_payments() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    let admin = settle_admins.get(0).unwrap();
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[33u8; 32]);
    settle_client.store_payment_reference(&merchant, &reference, &1_000);

    let operation = Operation::UnregisterMerchant(merchant.clone());
    settle_client.schedule(
        &soroban_sdk::vec![&env, admin],
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    settle_client.execute(&operation);

    assert!(!settle_client.is_merchant_registered(&merchant));
    let result = settle_client.try_get_payment_reference(&merchant, &reference);
    assert!(
        matches!(
            result,
            Err(Ok(ref err))
                if *err
                    == soroban_sdk::Error::from_contract_error(
                        SettlementError::PaymentOrphaned as u32
                    )
        ),
        "timelocked unregister must orphan payments too"
    );
}

// ---------------------------------------------------------------------------
// Issue 495: Reentrancy Guard
// ---------------------------------------------------------------------------

// A mock governance contract that attempts to reenter `store_payment_reference`
// during the `get_fee_config` call.
use soroban_sdk::{contract, contractimpl, IntoVal};

#[contract]
pub struct ReentrantGovernanceMock;

#[contractimpl]
impl ReentrantGovernanceMock {
    pub fn get_fee_config(env: Env) -> Option<GovFeeConfig> {
        // Attempt reentrancy if attack is armed
        if let Some(target_settle) = env
            .storage()
            .instance()
            .get::<_, Address>(&Symbol::new(&env, "target_settle"))
        {
            let settle_client = SettlementContractClient::new(&env, &target_settle);
            let merchant: Address = env
                .storage()
                .instance()
                .get(&Symbol::new(&env, "target_merchant"))
                .unwrap();
            let reference: BytesN<32> = env
                .storage()
                .instance()
                .get(&Symbol::new(&env, "target_ref"))
                .unwrap();

            // This should fail with DuplicatePaymentReference because the dummy record locks it
            let _ = settle_client.try_store_payment_reference(&merchant, &reference, &1000);
        }
        None
    }

    pub fn setup_attack(
        env: Env,
        target_settle: Address,
        target_merchant: Address,
        target_ref: BytesN<32>,
    ) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "target_settle"), &target_settle);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "target_merchant"), &target_merchant);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "target_ref"), &target_ref);
    }
}

#[test]
fn store_payment_reference_prevents_reentrancy() {
    let env = Env::default();
    env.mock_all_auths();

    let settle_admin = Address::generate(&env);
    let settle_recovery = Address::generate(&env);
    let settle_admins = soroban_sdk::vec![&env, settle_admin.clone()];

    let mock_gov_id = env.register_contract(None, ReentrantGovernanceMock);
    let settle_id = env.register_contract(None, SettlementContract);

    let settle_client = SettlementContractClient::new(&env, &settle_id);
    let deployer = Address::generate(&env);
    settle_client.init(&deployer, &settle_admins, &1, &mock_gov_id, &settle_recovery);

    let merchant = Address::generate(&env);
    settle_client.register_merchant(&settle_admins, &merchant);

    let reference = BytesN::<32>::from_array(&env, &[99u8; 32]);

    // Configure the malicious mock to reenter with the same reference
    env.invoke_contract::<()>(
        &mock_gov_id,
        &Symbol::new(&env, "setup_attack"),
        soroban_sdk::vec![
            &env,
            settle_id.into_val(&env),
            merchant.into_val(&env),
            reference.into_val(&env)
        ],
    );

    // This call triggers read_rule_or_default -> read_governance_fee_rule -> get_fee_config on our mock
    settle_client.store_payment_reference(&merchant, &reference, &1000);

    // Verify only one payment_stored event was emitted
    let events = env.events().all();
    let mut store_count = 0;
    for i in 0..events.len() {
        let (_contract, topics, _data) = events.get(i).unwrap();
        if !topics.is_empty() {
            let sym = Symbol::from_val(&env, &topics.get(0).unwrap());
            if sym == Symbol::new(&env, "payment_stored") {
                store_count += 1;
            }
        }
    }
    assert_eq!(
        store_count, 1,
        "payment_stored should be emitted exactly once"
    );
}

// ---------------------------------------------------------------------------
// Issue 496: Batch Lookup Caps
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #314)")]
fn get_payments_rejects_batch_too_large() {
    let (env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let mut refs = Vec::new(&env);
    // Create MAX_PAYMENTS_BATCH + 1 elements
    for i in 0..101u8 {
        refs.push_back(BytesN::<32>::from_array(&env, &[i; 32]));
    }

    settle_client.get_payments(&merchant, &refs);
}

#[test]
fn get_payments_accepts_max_batch_size() {
    let (env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let mut refs = Vec::new(&env);
    for i in 0..100u8 {
        refs.push_back(BytesN::<32>::from_array(&env, &[i; 32]));
    }

    let payments = settle_client.get_payments(&merchant, &refs);
    assert_eq!(payments.len(), 0); // No payments stored, but succeeds
}

// ---------------------------------------------------------------------------
// Issue 497: Off-chain Settlement Readiness
// ---------------------------------------------------------------------------

#[test]
fn off_chain_settlement_readiness_logic() {
    let (env, _gov, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    // Rule with 10 ledger delay
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    settle_client.set_default_rule(&settle_admins, &rule);

    let reference = BytesN::<32>::from_array(&env, &[1u8; 32]);
    env.ledger().with_mut(|l| l.sequence_number = 1000);
    settle_client.store_payment_reference(&merchant, &reference, &10_000);

    let record = settle_client
        .get_payment_reference(&merchant, &reference)
        .unwrap();
    assert_eq!(record.ledger, 1000);
    assert_eq!(record.settlement_delay_ledger, 10);

    // Demonstrate off-chain readiness check
    let is_ready = |current_ledger: u32, r: &PaymentRecord| -> bool {
        current_ledger >= r.ledger + r.settlement_delay_ledger
    };

    assert!(!is_ready(1009, &record), "Not ready before delay");
    assert!(is_ready(1010, &record), "Ready at exact delay ledger");
    assert!(is_ready(1011, &record), "Ready after delay");
}

#[test]
fn set_settlement_rule_emits_fallback_and_updated_events() {
    let (env, _gov_client, _gov_admins, settle_client, settle_admins, merchant) = setup_both();
    settle_client.register_merchant(&settle_admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 120,
        network_fee_bps: 20,
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    settle_client.set_settlement_rule(&settle_admins, &merchant, &rule);

    let events = env.events().all();
    let mut fallback_found = false;
    let mut update_found = false;
    let mut last_event_sym = Symbol::new(&env, "");

    for i in 0..events.len() {
        let (_contract, topics, _data) = events.get(i).unwrap();
        if topics.is_empty() {
            continue;
        }
        let sym = Symbol::from_val(&env, &topics.get(0).unwrap());
        if sym == Symbol::new(&env, bettapay_common::events::BOOTSTRAP_FALLBACK_EVENT) {
            fallback_found = true;
            last_event_sym = sym;
        } else if sym == Symbol::new(&env, bettapay_common::events::SETTLEMENT_RULE_UPDATED_EVENT) {
            update_found = true;
            assert!(
                fallback_found,
                "bootstrap_fallback must precede settlement_rule_updated"
            );
            assert_eq!(
                last_event_sym,
                Symbol::new(&env, bettapay_common::events::BOOTSTRAP_FALLBACK_EVENT),
                "events must be sequential"
            );
            last_event_sym = sym;
        }
    }

    assert!(fallback_found, "bootstrap_fallback event missing");
    assert!(update_found, "settlement_rule_updated event missing");
}
