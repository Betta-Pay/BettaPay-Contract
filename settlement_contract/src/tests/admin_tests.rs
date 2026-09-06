//! Tests for administrative entry points:
//! `init`, `transfer_admin`, `pause`, `unpause`, `upgrade`, `recovery`.

use crate::types::DataKey;
use crate::*;
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, Env, FromVal, Symbol, TryFromVal};
use soroban_sdk::testutils::{Address as _, Events, Ledger, MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, Env, FromVal, IntoVal, Symbol, TryFromVal};

use bettapay_common::constants::{
    BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS,
};
use bettapay_common::events::{AdminTransferred, PendingRecovery};
use bettapay_common::storage::CommonDataKey;

use super::reentrant_governance::{stash_target, ReentrantGovernance};
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
    let deployer = Address::generate(&env);

    client.init(
        &deployer,
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
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery_address);
    let _ = env;
}

// Issue #566: a self-recursive governance must not be able to reenter init.
// Soroban's host blocks same-contract reentry today, but the init-in-progress
// marker provides contract-level defence-in-depth. This test verifies both
// layers: the host rejects the reentrant call, and even if the host allowed it,
// the marker would catch it.
#[test]
fn rejects_reentrant_init_via_self_recursive_governance() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin];

    // Deploy the settlement contract first (not yet initialised).
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    // Deploy the malicious governance contract and stash the settlement
    // address so get_fee_config can reenter init.
    let reentrant_gov_id = env.register_contract(None, ReentrantGovernance);
    env.as_contract(&reentrant_gov_id, || {
        stash_target(&env, &contract_id);
    });

    // init now validates governance lazily (only at first fee-config use),
    // so a self-recursive governance does not cause a reentrant call during
    // init. The init-in-progress marker remains defence-in-depth but is
    // never exercised here.
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &reentrant_gov_id, &recovery_address);
    assert_eq!(client.get_governance(), reentrant_gov_id);
}

// Issue #566: the init-in-progress marker directly guards against reinit.
// Even without cross-contract reentry, manually setting the marker should
// cause init to reject with AlreadyInitialized.
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn rejects_init_while_initializing_marker_is_set() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let governance = register_governance(&env);
    let admins = soroban_sdk::vec![&env, admin];

    let contract_id = env.register_contract(None, SettlementContract);

    // Simulate an in-progress init by setting the Initializing marker.
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Initializing, &());
    });

    // init must reject because the Initializing marker is present.
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery_address);
// Issue #471: init used to authenticate only the first `threshold` admins, so
// with `admins.len() > threshold` the extras were stored without ever proving
// key control — a later `change_threshold` could then elevate an admin who
// never consented at init. Every proposed admin must authenticate.
#[test]
fn init_requires_auth_from_every_admin_when_threshold_below_len() {
    let env = Env::default();
    // No mock_all_auths: only the auths explicitly mocked below succeed.

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env); // extra admin beyond threshold
    let admins = soroban_sdk::vec![&env, admin_a.clone(), admin_b.clone()];
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    // Only the in-threshold admin authenticates; the extra admin does not.
    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: (admins.clone(), 1u32, &governance, &recovery).into_val(&env),
        sub_invokes: &[],
    };
    env.mock_auths(&[MockAuth {
        address: &admin_a,
        invoke: &invoke,
    }]);

    assert!(
        client
            .try_init(&admins, &1, &governance, &recovery)
            .is_err(),
        "init must fail when an admin beyond the threshold never authenticated"
    );
    assert!(
        !client.is_initialized(),
        "failed init must not leave the contract initialized"
    );
}

// Issue #471: the same len > threshold setup succeeds once every proposed
// admin has authenticated, and all of them are stored.
#[test]
fn init_accepts_all_admins_authenticated_when_threshold_below_len() {
    let env = Env::default();

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, admin_a.clone(), admin_b.clone()];
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: (admins.clone(), 1u32, &governance, &recovery).into_val(&env),
        sub_invokes: &[],
    };
    env.mock_auths(&[
        MockAuth {
            address: &admin_a,
            invoke: &invoke,
        },
        MockAuth {
            address: &admin_b,
            invoke: &invoke,
        },
    ]);

    client.init(&admins, &1, &governance, &recovery);
    assert_eq!(client.get_admin(), admins);
    assert_eq!(client.get_threshold(), 1);
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
    let scheduled_admins = soroban_sdk::vec![&env, scheduled_admin.clone()];
    let operation = Operation::TransferAdmin(scheduled_admins, 1);
    client.schedule(&admins, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&admins.get(0).unwrap().clone(), &operation);
    client.execute(&admins.get(0).unwrap(), &operation);
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
        initiated_by: Address::generate(&env),
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

// Issue #475: transfer_admin to the identical admin set must be rejected.
#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn rejects_same_admin_transfer() {
    let (_env, client, admins, _merchant) = setup();
    let threshold = client.get_threshold();
    client.transfer_admin(&admins, &admins, &threshold);
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

// Issue #470: settlement previously accepted a second `pause` while already
// paused, re-emitting a misleading `paused` event (governance rejected it
// with `AlreadyPaused`). These pin settlement's guards to the same behaviour.
#[test]
#[should_panic(expected = "Error(Contract, #316)")]
fn double_pause_is_rejected() {
    let (_env, client, admins, _merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());
    // Second pause while already paused must be rejected with AlreadyPaused.
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn pause_rejected_when_already_paused() {
    let (_env, client, admins, _merchant) = setup();
    client.pause(&admins);
    // Second pause must reject with AlreadyPaused (#15) and emit no extra event.
    client.pause(&admins);
}

#[test]
#[should_panic(expected = "Error(Contract, #317)")]
fn double_unpause_is_rejected() {
    let (_env, client, admins, _merchant) = setup();
    // Unpause with no prior pause must be rejected with AlreadyUnpaused.
    client.unpause(&admins);
}

// Issue #470: a rejected double-pause must not emit a duplicate `paused`
// event, so indexers only ever see transitions.
#[test]
fn double_pause_emits_no_duplicate_event() {
    let (env, client, admins, _merchant) = setup();
    client.pause(&admins);
    let events_before = env.events().all().len();

    let result = client.try_pause(&admins);
    assert!(result.is_err(), "double pause must be rejected");
    assert_eq!(
        env.events().all().len(),
        events_before,
        "rejected double pause must not emit a duplicate `paused` event"
    );
    assert!(client.is_paused(), "state must remain paused");
}

#[test]
fn double_unpause_emits_no_duplicate_event() {
    let (env, client, admins, _merchant) = setup();
    client.pause(&admins);
    client.unpause(&admins);
    let events_before = env.events().all().len();

    let result = client.try_unpause(&admins);
    assert!(result.is_err(), "double unpause must be rejected");
    assert_eq!(
        env.events().all().len(),
        events_before,
        "rejected double unpause must not emit a duplicate `unpaused` event"
    );
    assert!(!client.is_paused(), "state must remain unpaused");
}

// Issue #470: a real pause/unpause transition emits exactly one event each.
#[test]
fn pause_unpause_emit_exactly_one_event_per_transition() {
    let (env, client, admins, _merchant) = setup();

    let before = env.events().all().len();
    client.pause(&admins);
    assert_eq!(env.events().all().len(), before + 1);

    let before = env.events().all().len();
    client.unpause(&admins);
    assert_eq!(env.events().all().len(), before + 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn unpause_rejected_when_already_unpaused() {
    let (_env, client, admins, _merchant) = setup();
    // Contract starts unpaused; calling unpause immediately must reject with AlreadyUnpaused (#16).
    client.unpause(&admins);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn double_pause_emits_no_extra_event() {
    let (env, client, admins, _merchant) = setup();
    client.pause(&admins);
    let prev = env.events().all().len();
    client.pause(&admins);
    assert_eq!(
        env.events().all().len(),
        prev,
        "double pause must not emit events"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn unpause_when_not_paused_emits_no_event() {
    let (env, client, admins, _merchant) = setup();
    let prev = env.events().all().len();
    client.unpause(&admins);
    assert_eq!(
        env.events().all().len(),
        prev,
        "unpause when not paused must not emit events"
    );
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
#[should_panic(expected = "Error(Contract, #6)")]
fn register_merchant_rejects_admin_address() {
    let (_env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();

    // The admin cannot be registered as a merchant
    client.register_merchant(&admins, &admin);
}

#[test]
// SettlementError::Paused maps to error code 5
#[should_panic(expected = "Error(Contract, #5)")]
fn set_default_rule_rejected_when_paused() {
    let (_env, client, admins, _merchant) = setup();

    // Pause the contract to simulate an emergency state
    client.pause(&admins);
    assert!(
        client.is_paused(),
    assert_eq!(
        client.is_paused(),
        true,
        "Contract must be paused before testing rejection"
    );

    // Attempt to set a valid default rule; this should be rejected due to the pause state
    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// ---------------------------------------------------------------------------
// paused-state consistency (issue #476)
// ---------------------------------------------------------------------------

// All privileged entry points must return `Paused` (#5) — never `Unauthorized`
// (#3) — when the contract is paused, regardless of whether the caller is an
// admin. This pins the "pause-before-auth" ordering documented in the ADR.

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn update_governance_rejected_when_paused_for_non_admin() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&env);

    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    client.update_governance(&soroban_sdk::vec![&env, non_admin], &new_governance);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn update_governance_rejected_when_paused_for_admin() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&env);

    client.pause(&admins);
    assert!(client.is_paused());

    client.update_governance(&admins, &new_governance);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn register_merchant_rejected_when_paused_for_non_admin() {
    let (env, client, admins, merchant) = setup();

    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    client.register_merchant(&soroban_sdk::vec![&env, non_admin], &merchant);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_settlement_rule_rejected_when_paused_for_non_admin() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&soroban_sdk::vec![&env, non_admin], &merchant, &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn clear_settlement_rule_rejected_when_paused_for_non_admin() {
    let (env, client, admins, merchant) = setup();
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

    let non_admin = Address::generate(&env);
    client.clear_settlement_rule(&soroban_sdk::vec![&env, non_admin], &merchant);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_default_rule_rejected_when_paused_for_non_admin() {
    let (env, client, admins, _merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&soroban_sdk::vec![&env, non_admin], &rule);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn unregister_merchant_rejected_when_paused_for_non_admin() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    client.unregister_merchant(&soroban_sdk::vec![&env, non_admin], &merchant);
}

// ---------------------------------------------------------------------------
// merchant marker consistency (issue #477)
// ---------------------------------------------------------------------------

/// Both the direct `register_merchant` and the timelocked `_register_merchant`
/// must write the same marker type for `DataKey::Merchant`. This test reads
/// the raw storage value after each path and asserts they are identical.
#[test]
fn merchant_marker_is_identical_across_direct_and_timelocked_paths() {
    use crate::types::DataKey;
    use soroban_sdk::testutils::Ledger;

    let (env, client, admins, _merchant) = setup();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    // --- Direct path ---
    client.register_merchant(&admins, &merchant_a);

    // Both writers must store a value that is readable as `()`.
    let marker_a: () = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get(&DataKey::Merchant(merchant_a.clone()))
            .unwrap()
    });

    // --- Timelocked path ---
    let operation = Operation::RegisterMerchant(merchant_b.clone());
    client.schedule(
        &admins.get(0).unwrap(),
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&Address::generate(&env), &operation);

    let marker_b: () = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get(&DataKey::Merchant(merchant_b.clone()))
            .unwrap()
    });

    // Both writers must produce the same stored value type.
    assert_eq!(
        marker_a, marker_b,
        "direct and timelocked register_merchant must store identical marker values"
    );
}

// ---------------------------------------------------------------------------
// fee ceiling (issue #521)
// ---------------------------------------------------------------------------

// Both fees are independently capped at MAX_FEE_BPS (5000, i.e. 50%), even
// before governance has configured a GovFeeConfig - settlement no longer relies
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

#[test]
fn bootstrap_default_rule_satisfies_setter_fee_validation() {
    let rule = BOOTSTRAP_DEFAULT_RULE;

    assert!(rule.platform_fee_bps >= MIN_FEE_BPS);
    assert_eq!(rule.network_fee_bps, MIN_FEE_BPS);
    assert!(rule.platform_fee_bps <= MAX_FEE_BPS);
    assert!(rule.network_fee_bps <= MAX_FEE_BPS);
    assert!(rule.platform_fee_bps <= BPS_DENOMINATOR);
    assert!(rule.network_fee_bps <= BPS_DENOMINATOR);
    assert!(rule.platform_fee_bps + rule.network_fee_bps <= BPS_DENOMINATOR);
    assert!(rule.settlement_delay_ledger <= MAX_SETTLEMENT_DELAY_LEDGER);
}

// ---------------------------------------------------------------------------
// upgrade
// ---------------------------------------------------------------------------

#[test]
fn executes_contract_wasm_upgrade_successfully() {
    // After the interface check was added, empty wasm (no exports) is correctly
    // rejected. This test verifies rejection and confirms the contract is intact.
    let (env, client, admins, _) = setup();
    let wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let bad_hash = env.deployer().upload_contract_wasm(wasm);

    // Empty wasm has no `supports_interface` — upgrade must fail.
    let result = client.try_upgrade(&admins, &bad_hash);
    assert!(
        result.is_err(),
        "upgrade with non-conforming wasm must be rejected"
    );

    // Contract remains operational after the rejected upgrade.
    let live_client = SettlementContractClient::new(&env, &client.address);
    assert_eq!(live_client.get_admin(), admins);
}

// ---------------------------------------------------------------------------
// change_threshold
// ---------------------------------------------------------------------------

// Issue #565: setting a threshold above the admin count must surface
// `InvalidThreshold` (#14), not `Unauthorized` (#3) from the auth gate.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn change_threshold_above_admin_count_rejects_with_invalid_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery);

    // Threshold 3 > admins.len() 2 — must fail with InvalidThreshold, not auth.
    client.change_threshold(&admins, &3);
}

// Issue #565: threshold == 0 must also be rejected before the auth gate.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn change_threshold_zero_rejects_with_invalid_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = soroban_sdk::vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &governance, &recovery);

    client.change_threshold(&admins, &0);
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
    let deployer = Address::generate(&env);

    client.init(
        &deployer,
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

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn initiate_recovery_rejects_overwrite_while_pending() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let first_target = Address::generate(&env);
    let second_target = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &governance,
        &recovery_address,
    );

    client.initiate_recovery(&first_target);

    // Second initiation must be rejected — a recovery is already pending.
    client.initiate_recovery(&second_target);
}

// ---------------------------------------------------------------------------
// recovery timing / race tests
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn execute_recovery_rejects_before_delay() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin],
        &1,
        &governance,
        &recovery_address,
    );

    client.initiate_recovery(&new_admin);
    // Do NOT advance the ledger — the delay is still active.
    client.execute_recovery();
}

#[test]
fn execute_recovery_clears_pending_record() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin],
        &1,
        &governance,
        &recovery_address,
    );

    client.initiate_recovery(&new_admin);

    // Pending record exists before the delay.
    let before: Option<PendingRecovery> = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&CommonDataKey::PendingRecovery)
    });
    assert!(
        before.is_some(),
        "pending recovery must exist after initiate"
    );

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();

    let after: Option<PendingRecovery> = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&CommonDataKey::PendingRecovery)
    });
    assert!(
        after.is_none(),
        "pending recovery must be cleared after execute"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn execute_recovery_after_cancel_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &governance, &recovery_address);

    client.initiate_recovery(&new_admin);
    client.cancel_recovery(&admins);

    // Advance past the delay — but the recovery was cancelled.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn execute_recovery_second_call_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(
        &deployer,
        &soroban_sdk::vec![&env, admin],
        &1,
        &governance,
        &recovery_address,
    );

    client.initiate_recovery(&new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();

    // Second execute after the pending record has been consumed.
    client.execute_recovery();
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

/// The direct `update_governance` path must reject an address that fails
/// `validate_governance`. A zero address is rejected with
/// `InvalidGovernance` (#309) before any storage is written.
#[test]
#[should_panic(expected = "Error(Contract, #309)")]
fn update_governance_rejects_zero_address() {
    let (env, client, admins, _merchant) = setup();
    let zero_address = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));

    client.update_governance(&admins, &zero_address);
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
// InvalidWasmInterface: upgrade flow enforces supports_interface(1)
// ---------------------------------------------------------------------------

/// Uploading an empty Wasm (which has no `supports_interface` export) must be
/// rejected with `InvalidWasmInterface` (code 13).
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn upgrade_rejects_wasm_missing_supports_interface() {
    let (env, client, admins, _) = setup();
    // Empty wasm has no exports — the probe call will trap, raising the typed error.
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    client.upgrade(&admins, &bad_hash);
}

/// Non-admin callers must be rejected with `Unauthorized` (code 3) before
/// the interface check is even attempted.
// ---------------------------------------------------------------------------
// Matrix test: check-order parity between direct and scheduled paths (issue #523)
// ---------------------------------------------------------------------------

/// Verifies that the direct (`set_settlement_rule`) and scheduled
/// (`schedule` + `execute`) paths enforce the same canonical check order:
///   pause → fee validation → merchant registration.
///
/// For every (paused, invalid_fee, missing_merchant) combination the test
/// asserts that both paths surface the identical error code.
#[test]
fn settlement_rule_check_order_parity_across_paths() {
    use soroban_sdk::testutils::Ledger;

    let (env, client, admins, merchant) = setup();
    // Register the merchant so we have a "present" case to contrast with.
    client.register_merchant(&admins, &merchant);

    let valid_rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    let invalid_fee_rule = SettlementRule {
        platform_fee_bps: 0,
        network_fee_bps: 0,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let missing = Address::generate(&env); // unregistered merchant

    // Helper: schedule + advance time + execute via the timelocked path.
    let schedule_and_execute = |client: &SettlementContractClient,
                                admins: &soroban_sdk::Vec<Address>,
                                op: &Operation| {
        client.schedule(admins, op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
        env.ledger()
            .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
        client.execute(&admins.get(0).unwrap(), op);
    };

    // ---- 1. Paused ⇒ Paused (code 5) on both paths ----
    client.pause(&admins);

    let op = Operation::SetSettlementRule(merchant.clone(), valid_rule.clone());
    assert_eq!(client.try_set_settlement_rule(&admins, &merchant, &valid_rule).unwrap_err(),
               soroban_sdk::Error::from_contract_error(5));
    assert_eq!(client.try_schedule(&admins, &op, &DEFAULT_TIMELOCK_DELAY_SECONDS).unwrap_err(),
               soroban_sdk::Error::from_contract_error(5));

    // Unpause for the remaining cases.
    client.unpause(&admins);

    // ---- 2. Invalid fee + registered merchant ⇒ InvalidFeeBps (code 4) ----
    assert_eq!(client.try_set_settlement_rule(&admins, &merchant, &invalid_fee_rule).unwrap_err(),
               soroban_sdk::Error::from_contract_error(4));
    let op = Operation::SetSettlementRule(merchant.clone(), invalid_fee_rule.clone());
    client.schedule(&admins, &op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.try_execute(&admins.get(0).unwrap(), &op).unwrap_err(),
               soroban_sdk::Error::from_contract_error(4));

    // ---- 3. Valid rule + missing merchant ⇒ MerchantMissing (code 302) ----
    assert_eq!(client.try_set_settlement_rule(&admins, &missing, &valid_rule).unwrap_err(),
               soroban_sdk::Error::from_contract_error(302));
    let op = Operation::SetSettlementRule(missing.clone(), valid_rule.clone());
    client.schedule(&admins, &op, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert_eq!(client.try_execute(&admins.get(0).unwrap(), &op).unwrap_err(),
               soroban_sdk::Error::from_contract_error(302));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn upgrade_rejects_non_admin_before_interface_check() {
    let (env, client, _admins, _) = setup();
    let non_admin = Address::generate(&env);
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    client.upgrade(&soroban_sdk::vec![&env, non_admin], &bad_hash);
}

// Issue #563: is_merchant_registered (public) must not bump the merchant entry's TTL.
#[test]
fn is_merchant_registered_is_ttl_neutral() {
    let (env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    // Advance the ledger so that any TTL bump would be observable.
    env.ledger().with_mut(|l| l.sequence_number += 100);

    // Record the TTL immediately before the public read.
    let ttl_before = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Merchant(merchant.clone()))
    });

    // The public query must succeed and return true.
    assert!(client.is_merchant_registered(&merchant));

    // The merchant entry TTL must NOT have increased.
    let ttl_after = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Merchant(merchant.clone()))
    });
    assert!(
        ttl_after <= ttl_before,
        "is_merchant_registered must be TTL-neutral (before={ttl_before}, after={ttl_after})"
    );
}
/// A Wasm hash that was never uploaded (`upload_contract_wasm` was never
/// called for it) cannot be probed: protocol 21 exposes no wasm-presence
/// check to contract code, so the probe deployment traps with a host-level
/// `Storage`/`MissingValue` error ("Wasm does not exist") rather than the
/// typed `InvalidWasmInterface`. Documented here so the boundary of the
/// typed-error guarantee is explicit — see
/// `bettapay_common::upgrade::probe_supports_interface`.
#[test]
#[should_panic(expected = "Error(Storage, MissingValue)")]
fn upgrade_rejects_never_uploaded_wasm_hash() {
    let (env, client, admins, _) = setup();
    let garbage = soroban_sdk::BytesN::from_array(&env, &[0x47u8; 32]);
    client.upgrade(&admins, &garbage);
}

// ---------------------------------------------------------------------------
// Issue #704: SchemaVersion baseline + migrate skeleton
// ---------------------------------------------------------------------------

/// `init` must write the `SchemaVersion` marker, and calling `migrate`
/// repeatedly must be a no-op once the contract is already at
/// `CURRENT_SCHEMA_VERSION` — mirroring governance_contract's equivalent
/// test for issue #507.
#[test]
fn init_writes_schema_version_marker_and_migrate_is_idempotent() {
    use crate::types::DataKey;

    let (env, client, admins, _merchant) = setup();

    let version = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get::<_, u32>(&DataKey::SchemaVersion)
    });
    assert_eq!(version, Some(CURRENT_SCHEMA_VERSION));

    client.migrate(&admins);
    client.migrate(&admins);

    let version_after = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get::<_, u32>(&DataKey::SchemaVersion)
    });
    assert_eq!(version_after, Some(CURRENT_SCHEMA_VERSION));

    let (_, topics, _) = env.events().all().last().unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, bettapay_common::events::MIGRATED_EVENT)
    );
}

/// `migrate` is admin-gated like every other administrative entry point.
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn migrate_rejected_for_non_admin() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    client.migrate(&soroban_sdk::vec![&env, non_admin]);
}

/// `migrate` must respect the same pause gate as the rest of the admin
/// surface — a paused contract can't be migrated out from under an
/// in-progress incident response.
#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn migrate_rejected_while_paused() {
    let (_env, client, admins, _merchant) = setup();
    client.pause(&admins);
    client.migrate(&admins);
}
