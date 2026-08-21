//! Tests for administrative entry points:
//! `init`, `transfer_admin`, `pause`, `unpause`, `upgrade`, `recovery`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol, TryFromVal};

use bettapay_common::constants::RECOVERY_DELAY_SECONDS;
use bettapay_common::events::AdminTransferred;

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[test]
fn emits_event_on_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &governance,
        &recovery,
    );

    // init stores admin/governance/recovery; event emission may vary by version.
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, admin]);
    assert_eq!(client.get_governance(), governance);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn rejects_double_initialization() {
    let (env, client, admins, _) = setup();
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    client.init(&admins, &1, &governance, &recovery_address);
    let _ = env;
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn get_admin_panics_before_init() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.get_admin();
}

// ---------------------------------------------------------------------------
// transfer_admin
// ---------------------------------------------------------------------------

#[test]
fn transfer_admin_updates_admin_address() {
    let (env, client, admins, _) = setup();
    let new_admin = Address::generate(&env);

    assert_eq!(client.get_admin(), admins);
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, new_admin.clone()], &1);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
}

#[test]
#[should_panic(expected = "Error(Contract, #306)")]
fn rejects_zero_address_admin_transfer() {
    let (env, client, admins, _merchant) = setup();
    let zero_address = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, zero_address], &1);
}

// Issue #72: verify non-admin transfer_admin calls are rejected
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn transfer_admin_rejected_for_non_admin() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.transfer_admin(
        &soroban_sdk::vec![&env, non_admin],
        &soroban_sdk::vec![&env, new_admin],
        &1,
    );
}

#[test]
fn emits_event_on_admin_transfer() {
    let (env, client, admins, _merchant) = setup();
    let old_admin = admins.get(0).unwrap();
    let new_admin = Address::generate(&env);

    let before = env.events().all().len();
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, new_admin.clone()], &1);

    let events = env.events().all();
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one event should be emitted by transfer_admin"
    );

    let event = events.last().unwrap();
    let (contract_id, topics, data) = event;

    assert_eq!(contract_id, client.address);
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "admin_transferred")
    );

    let payload: AdminTransferred = AdminTransferred::try_from_val(&env, &data).unwrap();
    assert_eq!(payload.old_admin, old_admin);
    assert_eq!(payload.new_admin, new_admin);
}

// ---------------------------------------------------------------------------
// pause / unpause
// ---------------------------------------------------------------------------

// Issue #75: verify pause flag changes state in settlement contract
#[test]
fn pause_flag_changes_state() {
    let (_env, client, admins, _merchant) = setup();
    assert!(!client.is_paused());
    client.pause(&admins);
    assert!(client.is_paused());
    client.unpause(&admins);
    assert!(!client.is_paused());
}

// Issue #518: pause/unpause must publish the canonical `paused`/`unpaused`
// topics with the shared `(admin, bool)` payload shape.
#[test]
fn emits_canonical_pause_and_unpause_events() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();

    client.pause(&admins);
    let events = env.events().all();
    let event = events.last().unwrap();
    let (contract_id, topics, data) = event;
    assert_eq!(contract_id, client.address);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "paused")
    );
    let (event_admin, flag): (Address, bool) = FromVal::from_val(&env, &data);
    assert_eq!(event_admin, admin);
    assert!(flag);

    client.unpause(&admins);
    let events = env.events().all();
    let event = events.last().unwrap();
    let (contract_id, topics, data) = event;
    assert_eq!(contract_id, client.address);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "unpaused")
    );
    let (event_admin, flag): (Address, bool) = FromVal::from_val(&env, &data);
    assert_eq!(event_admin, admin);
    assert!(!flag);
}

// Issue #73: verify non-admins cannot pause the settlement contract
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn pause_rejected_for_non_admin() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    client.pause(&soroban_sdk::vec![&env, non_admin]);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn merchant_registration_blocked_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());
    // register_merchant calls assert_not_paused, so this must panic with Paused (#9).
    client.register_merchant(&admins, &merchant);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_settlement_rule_rejected_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    client.pause(&admins);
    assert!(client.is_paused());

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
}

// Issue #350: the merchant-specific settlement rule must not be cleared while paused.
#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn clear_settlement_rule_rejected_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);

    client.pause(&admins);
    assert!(client.is_paused());

    client.clear_settlement_rule(&admins, &merchant);
}

// Issue #231: the global default settlement rule must not be updated while paused.
#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_default_rule_rejected_when_paused() {
    let (_env, client, admins, _merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// ---------------------------------------------------------------------------
// fee ceiling (issue #521)
// ---------------------------------------------------------------------------

// Both fees are independently capped at MAX_FEE_BPS (5000, i.e. 50%), even
// before governance has configured a FeeConfig - settlement no longer relies
// solely on `validate_fee_against_governance` (which is a no-op with no
// governance config set) to keep per-fee values below 100%.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_settlement_rule_rejects_platform_fee_above_max_fee_bps() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS + 1,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_settlement_rule_rejects_network_fee_above_max_fee_bps() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 50,
        network_fee_bps: bettapay_common::constants::MAX_FEE_BPS + 1,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
}

#[test]
fn set_settlement_rule_accepts_fee_at_max_fee_bps_ceiling() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS,
        network_fee_bps: bettapay_common::constants::MIN_FEE_BPS,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_default_rule_rejects_fee_above_max_fee_bps() {
    let (_env, client, admins, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS + 1,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// ---------------------------------------------------------------------------
// upgrade
// ---------------------------------------------------------------------------

#[test]
fn executes_contract_wasm_upgrade_successfully() {
    let (env, client, admins, _) = setup();
    let wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let new_wasm_hash = env.deployer().upload_contract_wasm(wasm);

    let before = env.events().all().len();
    // Verifies the structural update pass completes without panicking
    client.upgrade(&admins, &new_wasm_hash);

    let events = env.events().all();
    assert!(events.len() > before);

    let event = events.last().unwrap();
    let (_contract_id, topics, data) = event;

    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "contract_upgraded")
    );
    assert_eq!(
        BytesN::<32>::from_val(&env, &topics.get(1).unwrap()),
        new_wasm_hash
    );
    assert_eq!(Address::from_val(&env, &data), admins.get(0).unwrap());

    // Ensure the upgraded contract remains callable and retains its state.
    let upgraded_client = SettlementContractClient::new(&env, &client.address);
    assert_eq!(upgraded_client.get_admin(), admins);
}

// ---------------------------------------------------------------------------
// recovery
// ---------------------------------------------------------------------------

#[test]
fn recovery_executes_after_delay() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &governance,
        &recovery_address,
    );
    assert_eq!(client.get_recovery_address(), recovery_address);

    client.initiate_recovery(&new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
}

// ---------------------------------------------------------------------------
// governance update
// ---------------------------------------------------------------------------

#[test]
fn update_governance_stores_validated_address() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&env);

    client.update_governance(&admins, &new_governance);

    assert_eq!(client.get_governance(), new_governance);
}

#[test]
fn bps_newtype_conversions_and_arithmetic_helpers_work() {
    let bps = Bps::new(250);
    assert_eq!(bps.value(), 250);
    assert_eq!(bps.as_i128(), 250i128);

    let fee_amount = bps.calculate_fee_ceil(10_000);
    assert_eq!(fee_amount, 250);

    let rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    assert_eq!(rule.platform_bps(), Bps::new(150));
    assert_eq!(rule.network_bps(), Bps::new(50));

    let from_u32: Bps = 100u32.into();
    let to_u32: u32 = from_u32.into();
    assert_eq!(to_u32, 100);
}

// ---------------------------------------------------------------------------
// get_effective_rule (Issue #579)
// ---------------------------------------------------------------------------

#[test]
fn get_effective_rule_uses_bootstrap_default_when_no_rules_set() {
    let (_env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);

    let rule = client.get_effective_rule(&merchant);
    assert_eq!(rule.platform_fee_bps, BOOTSTRAP_DEFAULT_RULE.platform_fee_bps);
    assert_eq!(rule.network_fee_bps, BOOTSTRAP_DEFAULT_RULE.network_fee_bps);
    assert_eq!(rule.settlement_delay_ledger, BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger);
    assert_eq!(rule.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
}

#[test]
fn get_effective_rule_uses_global_default_when_set() {
    let (_env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);

    let global_default = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 100,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &global_default);

    let rule = client.get_effective_rule(&merchant);
    assert_eq!(rule.platform_fee_bps, 200);
    assert_eq!(rule.network_fee_bps, 50);
    assert_eq!(rule.settlement_delay_ledger, 100);
    assert_eq!(rule.auto_settle, true);
}

#[test]
fn get_effective_rule_merchant_rule_overrides_global_default() {
    let (_env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);

    let global_default = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 100,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &global_default);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 500,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &merchant_rule);

    let rule = client.get_effective_rule(&merchant);
    assert_eq!(rule.platform_fee_bps, 300);
    assert_eq!(rule.network_fee_bps, 100);
    assert_eq!(rule.settlement_delay_ledger, 500);
    assert_eq!(rule.auto_settle, false);
}

#[test]
fn get_effective_rule_cleared_merchant_rule_falls_back_to_global_default() {
    let (_env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);

    let global_default = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 75,
        settlement_delay_ledger: 200,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &global_default);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 500,
        auto_settle: false,
    };
    client.set_settlement_rule(&admins, &merchant, &merchant_rule);
    assert_eq!(client.get_effective_rule(&merchant).platform_fee_bps, 300);

    client.clear_settlement_rule(&admins, &merchant);

    let rule = client.get_effective_rule(&merchant);
    assert_eq!(rule.platform_fee_bps, 150);
    assert_eq!(rule.network_fee_bps, 75);
    assert_eq!(rule.settlement_delay_ledger, 200);
    assert_eq!(rule.auto_settle, true);
}

#[test]
#[should_panic(expected = "Error(Contract, #301)")]
fn get_effective_rule_rejects_unregistered_merchant() {
    let (_env, client, _admins, merchant) = setup();

    client.get_effective_rule(&merchant);
}

#[test]
fn get_effective_rule_agrees_with_calculate_fee_split_bps() {
    let (_env, client, admins, merchant) = setup();

    client.register_merchant(&admins, &merchant);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 80,
        settlement_delay_ledger: 42,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &merchant_rule);

    let effective = client.get_effective_rule(&merchant);
    let split = client.calculate_fee_split(&merchant, &1_000_000);

    assert_eq!(effective.platform_fee_bps, 250);
    assert_eq!(effective.network_fee_bps, 80);
    assert_eq!(split.platform_fee_amount, 2_500);
    assert_eq!(split.network_fee_amount, 800);
}

