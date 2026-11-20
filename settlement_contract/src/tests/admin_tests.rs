//! Tests for administrative entry points:
//! `init`, `transfer_admin`, `pause`, `unpause`, `upgrade`, `recovery`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, BytesN, Env, FromVal, Symbol, TryFromVal, Vec};

use bettapay_common::constants::RECOVERY_DELAY_SECONDS;
use bettapay_common::events::{AdminTransferred, PendingRecovery, INITIALIZED_EVENT};
use bettapay_common::storage::CommonDataKey;

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

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, admin.clone()]);
    assert_eq!(client.get_governance(), governance);
    assert!(!client.is_paused());

    // init publishes exactly one `initialized` event with the primary admin
    // as its data, matching governance_contract::init.
    let events = env.events().all();
    assert_eq!(events.len(), 1, "exactly one event emitted on init");
    let (_contract_id, topics, data) = events.get(0).unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, INITIALIZED_EVENT)
    );
    assert_eq!(Address::from_val(&env, &data), admin);
}

#[test]
fn init_seeds_paused_key_to_false() {
    let (env, client, _admins, _merchant) = setup();
    assert!(!client.is_paused());
    env.as_contract(&client.address, || {
        assert!(env.storage().instance().has(&CommonDataKey::Paused));
        assert_eq!(
            env.storage()
                .instance()
                .get::<_, bool>(&CommonDataKey::Paused),
            Some(false)
        );
    });
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
// change_threshold
// ---------------------------------------------------------------------------

fn contract_error(error: SettlementError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(error as u32)
}

/// Registers a 2-of-2 settlement contract (both admins must sign admin calls).
fn setup_two_of_two() -> (Env, SettlementContractClient<'static>, Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();
    let admin1 = Address::generate(&env);
    let admin2 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin1, admin2];
    let governance = register_governance(&env);
    let recovery = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.init(&admins, &2, &governance, &recovery);
    (env, client, admins)
}

#[test]
fn change_threshold_allows_2_of_2_to_lower_to_1() {
    let (env, client, admins) = setup_two_of_two();

    client.change_threshold(&admins, &1);
    assert_eq!(client.get_threshold(), 1);
    assert_eq!(
        Symbol::from_val(&env, &env.events().all().last().unwrap().1.get(0).unwrap()),
        Symbol::new(&env, bettapay_common::events::THRESHOLD_CHANGED_EVENT)
    );
}

#[test]
fn change_threshold_keeps_1_of_2_requirement_at_two_signers() {
    let (env, client, admins) = setup_two_of_two();
    client.change_threshold(&admins, &1);

    // The cap only relaxes the N-of-N case; with two admins and threshold 1
    // the old "+1" rule still requires both signatures.
    let single_signer = soroban_sdk::vec![&env, admins.get(0).unwrap()];
    assert_eq!(
        client.try_change_threshold(&single_signer, &2),
        Err(Ok(contract_error(SettlementError::Unauthorized)))
    );
}

#[test]
fn change_threshold_rejects_sub_threshold_signers() {
    let (env, client, admins) = setup_two_of_two();
    let single_signer = soroban_sdk::vec![&env, admins.get(0).unwrap()];
    assert_eq!(
        client.try_change_threshold(&single_signer, &1),
        Err(Ok(contract_error(SettlementError::Unauthorized)))
    );
}

#[test]
fn change_threshold_authenticates_before_validating_new_threshold() {
    let (env, client, admins) = setup_two_of_two();
    let unauthorized = soroban_sdk::vec![&env, Address::generate(&env)];

    // An unauthenticated caller must always see Unauthorized, never a
    // validation error revealing the admin count or current threshold.
    assert_eq!(
        client.try_change_threshold(&unauthorized, &0),
        Err(Ok(contract_error(SettlementError::Unauthorized)))
    );
    assert_eq!(
        client.try_change_threshold(&unauthorized, &2),
        Err(Ok(contract_error(SettlementError::Unauthorized)))
    );

    // Authorized callers still get the ordinary validation error.
    assert_eq!(
        client.try_change_threshold(&admins, &0),
        Err(Ok(contract_error(SettlementError::InvalidThreshold)))
    );
    assert_eq!(
        client.try_change_threshold(&admins, &3),
        Err(Ok(contract_error(SettlementError::InvalidThreshold)))
    );
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
fn every_admin_writer_preserves_the_vector_shape() {
    // Direct transfer.
    let (env, client, admins, _) = setup();
    let direct_admin = Address::generate(&env);
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, direct_admin.clone()], &1);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, direct_admin]);

    // Recovery transfer.
    let (env, client, _admins, _) = setup();
    let recovery_admin = Address::generate(&env);
    client.initiate_recovery(&recovery_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, recovery_admin]);

    // Timelocked transfer (the path that previously wrote a scalar Address).
    let (env, client, admins, _) = setup();
    let scheduled_admin = Address::generate(&env);
    let operation = Operation::TransferAdmin(scheduled_admin.clone());
    client.schedule(
        &admins.get(0).unwrap(),
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, scheduled_admin]);
}

#[test]
fn failed_recovery_keeps_pending_target() {
    let (env, client, _admins, _) = setup();
    let zero_admin = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    let pending = PendingRecovery {
        new_admin: zero_admin.clone(),
        execute_after: env.ledger().timestamp(),
    };

    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
    });

    assert!(client.try_execute_recovery().is_err());
    let retained: Option<PendingRecovery> = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&CommonDataKey::PendingRecovery)
    });
    assert_eq!(retained.unwrap().new_admin, zero_admin);
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
        Symbol::new(&env, bettapay_common::events::ADMIN_TRANSFERRED_EVENT)
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

// Issue #550: settlement previously emitted non-canonical "pause"/"unpause"/
// "admin" topics while governance used "paused"/"unpaused"/
// "admin_transferred", so an indexer subscribed to the canonical names
// missed every settlement event. This pins settlement's topics to
// `bettapay_common::events`' shared constants so it fails again if either
// contract's topic strings drift apart.
#[test]
fn pause_unpause_and_admin_transfer_use_canonical_topics() {
    let (env, client, admins, _merchant) = setup();

    client.pause(&admins);
    let (_, pause_topics, _) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &pause_topics.get(0).unwrap()),
        Symbol::new(&env, bettapay_common::events::PAUSED_EVENT)
    );

    client.unpause(&admins);
    let (_, unpause_topics, _) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &unpause_topics.get(0).unwrap()),
        Symbol::new(&env, bettapay_common::events::UNPAUSED_EVENT)
    );

    let new_admin = Address::generate(&env);
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, new_admin], &1);
    let (_, transfer_topics, _) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &transfer_topics.get(0).unwrap()),
        Symbol::new(&env, bettapay_common::events::ADMIN_TRANSFERRED_EVENT)
    );
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
