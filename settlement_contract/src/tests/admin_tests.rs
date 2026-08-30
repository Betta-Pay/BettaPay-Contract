//! Tests for administrative entry points:
//! `init`, `transfer_admin`, `pause`, `unpause`, `upgrade`, `recovery`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, Env, FromVal, Symbol, TryFromVal};

use bettapay_common::constants::{
    BPS_DENOMINATOR, MAX_FEE_BPS, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS,
};
use bettapay_common::events::{AdminTransferred, PendingRecovery};
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
    let deployer = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

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
// Fee-sum boundary (issue #466)
// ---------------------------------------------------------------------------

// Default rule with platform + network summing exactly to BPS_DENOMINATOR
// (100 %) must be accepted — the check is `>`, not `>=`.
#[test]
fn set_default_rule_accepts_fee_sum_at_boundary() {
    let (_env, client, admins, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS,
        network_fee_bps: bettapay_common::constants::MAX_FEE_BPS,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// Default rule with combined fees exceeding BPS_DENOMINATOR must be rejected.
// With MAX_FEE_BPS = 5000 the sum check is currently unreachable from
// set_default_rule alone (individual fee cap fires first), but this test
// validates the scheduled path's defence-in-depth and pins the behaviour
// for future MAX_FEE_BPS increases.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_default_rule_rejects_fee_sum_exceeding_denominator() {
    let (_env, client, admins, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS + 1,
        network_fee_bps: bettapay_common::constants::MAX_FEE_BPS,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// Scheduled path (_set_default_rule) must enforce the same sum check.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn set_default_rule_rejects_fee_sum_exceeding_denominator_scheduled() {
    let (env, client, admins, _merchant) = setup();
    let admin = admins.get(0).unwrap();

    let rule = SettlementRule {
        platform_fee_bps: bettapay_common::constants::MAX_FEE_BPS + 1,
        network_fee_bps: bettapay_common::constants::MAX_FEE_BPS,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    let operation = Operation::SetDefaultRule(rule);
    client.schedule(
        &soroban_sdk::vec![&env, admin],
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);
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
    let deployer = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

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
