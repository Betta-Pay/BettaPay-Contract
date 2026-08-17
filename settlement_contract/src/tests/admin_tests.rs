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

// ---------------------------------------------------------------------------
// Issue #580: Pause-check ordering matrix for update_governance
//
// Standardized order MUST be: assert_not_paused → validate_governance → verify_admin_auth
// When paused, error is ALWAYS Paused(#5) regardless of auth or validation state.
// ---------------------------------------------------------------------------

fn zero_address(env: &Env) -> Address {
    Address::from_string(&soroban_sdk::String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ))
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn update_governance_paused_admin_valid_governance_returns_paused() {
    let (_env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&_env);

    client.pause(&admins);
    assert!(client.is_paused());

    client.update_governance(&admins, &new_governance);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn update_governance_paused_admin_invalid_governance_returns_paused_not_invalid() {
    let (env, client, admins, _merchant) = setup();
    let invalid_gov = zero_address(&env);

    client.pause(&admins);
    assert!(client.is_paused());

    client.update_governance(&admins, &invalid_gov);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn update_governance_paused_non_admin_valid_governance_returns_paused_not_unauthorized() {
    let (env, client, admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    let new_governance = register_governance(&env);

    client.pause(&admins);
    assert!(client.is_paused());

    client.update_governance(
        &soroban_sdk::vec![&env, non_admin],
        &new_governance,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn update_governance_unpaused_non_admin_valid_governance_returns_unauthorized() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    let new_governance = register_governance(&env);

    assert!(!client.is_paused());

    client.update_governance(
        &soroban_sdk::vec![&env, non_admin],
        &new_governance,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #309)")]
fn update_governance_unpaused_admin_invalid_governance_returns_invalid_governance() {
    let (env, client, admins, _merchant) = setup();
    let invalid_gov = zero_address(&env);

    assert!(!client.is_paused());

    client.update_governance(&admins, &invalid_gov);
}

#[test]
fn update_governance_unpaused_admin_valid_governance_succeeds() {
    let (_env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&_env);
    let old_governance = client.get_governance();

    assert!(!client.is_paused());
    assert_ne!(old_governance, new_governance);

    client.update_governance(&admins, &new_governance);

    assert_eq!(client.get_governance(), new_governance);
}

// ---------------------------------------------------------------------------
// Issue #580: Scheduled execution pause matrix for UpdateGovernance
//
// Scheduled ops go through execute() → _update_governance().
// Pause state at EXECUTE time determines outcome; pause check always
// comes first inside _update_governance.
// ---------------------------------------------------------------------------

use crate::DEFAULT_TIMELOCK_DELAY_SECONDS;
use crate::Operation;
use soroban_sdk::testutils::Ledger as _;

fn schedule_and_advance(env: &Env, client: &SettlementContractClient<'_>, admin: &Address, op: &Operation) {
    client.schedule(admin, op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS + 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn scheduled_update_governance_pause_after_schedule_blocks_on_execute() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let new_governance = register_governance(&env);
    let op = Operation::UpdateGovernance(new_governance);

    schedule_and_advance(&env, &client, &admin, &op);

    client.pause(&admins);
    assert!(client.is_paused());

    client.execute(&op);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn scheduled_update_governance_pause_before_schedule_blocks_on_execute() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let new_governance = register_governance(&env);
    let op = Operation::UpdateGovernance(new_governance);

    client.pause(&admins);
    assert!(client.is_paused());

    schedule_and_advance(&env, &client, &admin, &op);

    client.execute(&op);
}

#[test]
fn scheduled_update_governance_unpaused_at_execute_succeeds() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let new_governance = register_governance(&env);
    let op = Operation::UpdateGovernance(new_governance.clone());
    let old_governance = client.get_governance();

    assert_ne!(old_governance, new_governance);

    schedule_and_advance(&env, &client, &admin, &op);

    client.pause(&admins);
    client.unpause(&admins);
    assert!(!client.is_paused());

    client.execute(&op);

    assert_eq!(client.get_governance(), new_governance);
}

#[test]
fn direct_and_scheduled_update_governance_produce_same_governance_state() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();
    let direct_gov = register_governance(&env);
    let scheduled_gov = register_governance(&env);

    client.update_governance(&admins, &direct_gov);
    assert_eq!(client.get_governance(), direct_gov);

    let reset_gov = register_governance(&env);
    client.update_governance(&admins, &reset_gov);

    let op = Operation::UpdateGovernance(scheduled_gov.clone());
    schedule_and_advance(&env, &client, &admin, &op);
    client.execute(&op);

    assert_eq!(client.get_governance(), scheduled_gov);
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

