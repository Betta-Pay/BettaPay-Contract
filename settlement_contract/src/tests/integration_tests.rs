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
    client.init(&admins, &1, &recovery_address);
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
    gov_client.init(&gov_admins, &1, &gov_recovery);

    let settle_admin = Address::generate(&env);
    let settle_recovery = Address::generate(&env);
    let settle_admins = soroban_sdk::vec![&env, settle_admin.clone()];
    let merchant = Address::generate(&env);
    let settle_id = env.register_contract(None, SettlementContract);
    let settle_client = SettlementContractClient::new(&env, &settle_id);
    settle_client.init(&settle_admins, &1, &gov_id, &settle_recovery);

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
    // Bootstrap default is 100 bps platform, 0 network — fee = 100, merchant = 9900.
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 9_900);
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
        .get_payment_reference(&payment_ref)
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
    old_gov.init(&gov_admins, &1, &gov_recovery);
    old_gov.set_fee_config(
        &gov_admins,
        &GovFeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 20,
        },
    );

    let new_gov_id = env.register_contract(None, GovernanceContract);
    let new_gov = GovernanceContractClient::new(&env, &new_gov_id);
    new_gov.init(&gov_admins, &1, &gov_recovery);
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
    settle_client.init(&settle_admins, &1, &old_gov_id, &settle_recovery);
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
    gov_client.init(&gov_admins, &2, &gov_recovery);

    let settle_admins = soroban_sdk::vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let settle_id = env.register_contract(None, SettlementContract);
    let settle_client = SettlementContractClient::new(&env, &settle_id);
    settle_client.init(&settle_admins, &2, &gov_id, &settle_recovery);

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
        let rec = settle_client.get_payment_reference(&r).unwrap();
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
    let records = settle_client.get_payments(&refs);
    assert_eq!(records.len(), 4);
    for i in 0..4u32 {
        assert_eq!(records.get(i).unwrap().unwrap().amount, amounts[i as usize]);
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
    // Bootstrap default rule applies (100 bps platform, 0 network): at the floor
    // the platform fee is exactly 1 unit, with no ceil-rounding overshoot.
    assert_eq!(split.platform_fee_amount, 1);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 99);
}
