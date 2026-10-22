use super::*;
use proptest::prelude::*;
use soroban_sdk::testutils::storage::Persistent;
use soroban_sdk::testutils::{Address as _, Events, Ledger, MockAuth, MockAuthInvoke};
use soroban_sdk::{vec, Bytes, FromVal, IntoVal, String};

fn setup() -> (
    Env,
    GovernanceContractClient<'static>,
    Vec<Address>,
    Address,
) {
    super::setup_with(2, 2)
}

#[allow(dead_code)]
fn upload_test_wasm(env: &Env) -> BytesN<32> {
    let wasm = Bytes::from_slice(env, &[]);
    env.deployer().upload_contract_wasm(wasm)
}

// -----------------------------------------------------------------------
// supports_interface — issue #48
// -----------------------------------------------------------------------
//
// These tests pin the exact version semantics so the function can never
// silently degrade into an always-true stub.

/// Version 1 is the current advertised interface; `supports_interface(1)`
/// must return `true`.
#[test]
fn supports_interface_returns_true_for_current_version() {
    let env = Env::default();
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    assert!(
        client.supports_interface(&SUPPORTED_INTERFACE_VERSION),
        "supports_interface must return true for the current interface version ({})",
        SUPPORTED_INTERFACE_VERSION,
    );
}

/// Version 0 has never been a valid interface version; it must be rejected.
#[test]
fn supports_interface_returns_false_for_version_zero() {
    let env = Env::default();
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&0u32),
        "supports_interface must return false for version 0",
    );
}

/// Version 2 is a hypothetical future version not yet implemented; it
/// must be rejected so callers can distinguish old Wasm from new.
#[test]
fn supports_interface_returns_false_for_unknown_future_version() {
    let env = Env::default();
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&(SUPPORTED_INTERFACE_VERSION + 1)),
        "supports_interface must return false for a future version not yet implemented",
    );
}

/// A large sentinel value must also be rejected (issue #48: must not be
/// an always-true stub).
#[test]
fn supports_interface_returns_false_for_large_sentinel() {
    let env = Env::default();
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    assert!(
        !client.supports_interface(&u32::MAX),
        "supports_interface must return false for a large out-of-range version",
    );
}

#[test]
fn executes_contract_wasm_upgrade_successfully() {
    // After adding the interface check, empty wasm (no exports) is correctly
    // rejected rather than silently accepted.  This test verifies rejection
    // and confirms the contract remains operational afterwards.
    //
    // The positive case (conforming wasm accepted) requires uploading the
    // governance wasm bytes; the `upgrade_rejects_wasm_missing_supports_interface`
    // and `upgrade_rejects_non_admin_before_interface_check` tests cover the
    // negative guard paths.
    let (env, client, admins, _recovery) = setup();
    let bad_hash = upload_test_wasm(&env); // empty wasm — no supports_interface

    let result = client.try_upgrade(&admins, &bad_hash);
    assert!(
        result.is_err(),
        "upgrade with non-conforming wasm must be rejected"
    );

    // Contract is intact after the failed upgrade.
    let live_client = GovernanceContractClient::new(&env, &client.address);
    assert_eq!(live_client.get_admin(), admins);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn governance_rejects_double_initialization() {
    let (env, client, admins, recovery) = setup();
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &recovery);
}

// Issue #471: init used to authenticate only the first `threshold` admins,
// so with `admins.len() > threshold` the extras were stored without ever
// proving key control — a later `change_threshold` could then elevate an
// admin who never consented at init. Every proposed admin must authenticate.
#[test]
fn init_requires_auth_from_every_admin_when_threshold_below_len() {
    let env = Env::default();
    // No mock_all_auths: only the auths explicitly mocked below succeed.

    let admin_a = Address::generate(&env);
    let admin_b = Address::generate(&env); // extra admin beyond threshold
    let admins = vec![&env, admin_a.clone(), admin_b.clone()];
    let recovery = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    // Only the in-threshold admin authenticates; the extra admin does not.
    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: (admins.clone(), 1u32, &recovery).into_val(&env),
        sub_invokes: &[],
    };
    env.mock_auths(&[MockAuth {
        address: &admin_a,
        invoke: &invoke,
    }]);

    let deployer = Address::generate(&env);
    assert!(
        client.try_init(&deployer, &admins, &1, &recovery).is_err(),
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
    let admins = vec![&env, admin_a.clone(), admin_b.clone()];
    let recovery = Address::generate(&env);
    let deployer = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: (deployer.clone(), admins.clone(), 1u32, &recovery).into_val(&env),
        sub_invokes: &[],
    };
    env.mock_auths(&[
        MockAuth {
            address: &deployer,
            invoke: &invoke,
        },
        MockAuth {
            address: &admin_a,
            invoke: &invoke,
        },
        MockAuth {
            address: &admin_b,
            invoke: &invoke,
        },
    ]);

    client.init(&deployer, &admins, &1, &recovery);
    assert_eq!(client.get_admin(), admins);
    assert_eq!(client.get_threshold(), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn governance_rejects_zero_threshold_init() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &vec![&env, admin], &0, &recovery);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn governance_rejects_zero_address_admin_transfer() {
    let (env, client, admins, _recovery) = setup();
    let zero_address = bettapay_common::storage::zero_address(&env);

    client.transfer_admin(&admins, &vec![&env, zero_address], &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")]
fn rejects_same_admin_transfer() {
    let (_env, client, admins, _recovery) = setup();
    let threshold = client.get_threshold();
    client.transfer_admin(&admins, &admins, &threshold);
}

#[test]
fn updates_system_parameters() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "max_settle");
    let before = env.events().all().len();
    client.update_system_param(&admins, &key, &1440);
    assert_eq!(client.get_system_param(&key), Some(1440));
    assert!(env.events().all().len() > before);
}

#[test]
fn system_parameter_key_uniqueness_overwrites_value() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "test_param");

    client.update_system_param(&admins, &key, &100);
    assert_eq!(client.get_system_param(&key), Some(100));

    client.update_system_param(&admins, &key, &200);
    assert_eq!(client.get_system_param(&key), Some(200));

    client.update_system_param(&admins, &key, &300);
    assert_eq!(client.get_system_param(&key), Some(300));
}

#[test]
fn sets_fee_config() {
    let (env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    let before = env.events().all().len();
    client.set_fee_config(&admins, &cfg);
    let got = client.get_fee_config().expect("expected config");
    assert_eq!(got.platform_fee_bps, 120);
    assert_eq!(got.network_fee_bps, 35);
    assert!(env.events().all().len() > before);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_fee_config_blocked_when_paused() {
    let (_env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };

    client.pause(&admins);
    client.set_fee_config(&admins, &cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn set_fee_config_checks_paused_before_auth_and_validation() {
    let (env, client, admins, _recovery) = setup();
    let non_admin = Address::generate(&env);
    let non_admin_signer = vec![&env, non_admin];
    let invalid_cfg = FeeConfig {
        platform_fee_bps: 5_001,
        network_fee_bps: 4,
    };

    client.pause(&admins);
    client.set_fee_config(&non_admin_signer, &invalid_cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn set_fee_config_rejects_signatures_below_threshold_when_not_paused() {
    let (env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    let single_signer = vec![&env, admins.get(0).unwrap()];

    client.set_fee_config(&single_signer, &cfg);
}

#[test]
fn fee_config_event_emitted_with_correct_fields() {
    let (env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };

    client.set_fee_config(&admins, &cfg);

    let events = env.events().all();
    let event = events.last().unwrap();

    let (_contract_id, topics, data) = event;

    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, events::FEE_CONFIG_UPDATED_EVENT)
    );

    let (event_admin, event_cfg): (Address, FeeConfig) = FromVal::from_val(&env, &data);
    assert_eq!(event_admin, admins.get(0).unwrap());
    assert_eq!(event_cfg.platform_fee_bps, 120);
    assert_eq!(event_cfg.network_fee_bps, 35);
}

#[test]
fn set_fee_config_rejects_individual_fee_exceeding_max_without_overwriting_config() {
    let (_env, client, admins, _recovery) = setup();
    let previous = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    client.set_fee_config(&admins, &previous);

    let invalid = FeeConfig {
        platform_fee_bps: MAX_FEE_BPS + 1,
        network_fee_bps: MIN_FEE_BPS,
    };
    let result = client.try_set_fee_config(&admins, &invalid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        GovernanceError::InvalidFeeBps.into()
    );

    let stored = client.get_fee_config().expect("previous config remains");
    assert_eq!(stored.platform_fee_bps, previous.platform_fee_bps);
    assert_eq!(stored.network_fee_bps, previous.network_fee_bps);
}

// -----------------------------------------------------------------------
// Issue #816: FeeConfig sum-boundary test at 10000 vs 10001
// -----------------------------------------------------------------------

/// Verifies the critical sum-boundary invariant: a config whose
/// platform + network fee sum equals exactly `BPS_DENOMINATOR` (10 000)
/// must be accepted and readable, while a sum of 10 001 must be rejected
/// with `InvalidFeeBps` without overwriting the accepted configuration.
#[test]
fn fee_config_sum_boundary_and_overflow_are_enforced() {
    let (_env, client, admins, _recovery) = setup();

    // MAX_FEE_BPS + MAX_FEE_BPS == BPS_DENOMINATOR — must succeed.
    let at_boundary = FeeConfig {
        platform_fee_bps: MAX_FEE_BPS,
        network_fee_bps: MAX_FEE_BPS,
    };
    assert!(
        client.try_set_fee_config(&admins, &at_boundary).is_ok(),
        "FeeConfig {{5000, 5000}} with sum == BPS_DENOMINATOR must be accepted"
    );
    let stored = client
        .get_fee_config()
        .expect("accepted boundary config must be readable");
    assert_eq!(stored.platform_fee_bps, at_boundary.platform_fee_bps);
    assert_eq!(stored.network_fee_bps, at_boundary.network_fee_bps);

    // With MAX_FEE_BPS at half the denominator, an over-sum config necessarily
    // also violates an individual bound. It must still fail with the typed
    // InvalidFeeBps error and preserve the last accepted value.
    let invalid = FeeConfig {
        platform_fee_bps: MAX_FEE_BPS,
        network_fee_bps: MAX_FEE_BPS + 1,
    };
    let result = client.try_set_fee_config(&admins, &invalid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        GovernanceError::InvalidFeeBps.into()
    );

    let stored = client
        .get_fee_config()
        .expect("accepted boundary config remains");
    assert_eq!(stored.platform_fee_bps, at_boundary.platform_fee_bps);
    assert_eq!(stored.network_fee_bps, at_boundary.network_fee_bps);
}

#[test]
fn upserts_and_removes_anchor() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    let before_upsert = env.events().all().len();
    client.upsert_anchor(&admins, &asset, &anchor);
    assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
    assert!(env.events().all().len() > before_upsert);

    let before_remove = env.events().all().len();
    client.remove_anchor(&admins, &asset);
    assert_eq!(client.get_anchor(&asset), None);
    assert!(env.events().all().len() > before_remove);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn rejects_upsert_anchor_with_zero_address_asset() {
    let (env, client, admins, _recovery) = setup();
    let zero_address = bettapay_common::storage::zero_address(&env);
    let anchor = Address::generate(&env);

    client.upsert_anchor(&admins, &zero_address, &anchor);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn rejects_upsert_anchor_with_zero_address_anchor() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let zero_address = bettapay_common::storage::zero_address(&env);

    client.upsert_anchor(&admins, &asset, &zero_address);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn rejects_upsert_anchor_where_asset_equals_anchor() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);

    client.upsert_anchor(&admins, &asset, &asset);
}

#[test]
fn get_anchor_extends_anchor_ttl() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);

    client.upsert_anchor(&admins, &asset, &anchor);
    assert_eq!(client.get_anchor(&asset), Some(anchor.clone()));
    assert_eq!(client.get_anchor(&asset), Some(anchor));
}

#[test]
fn upsert_anchor_uses_same_ttl_policy_as_read() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    let seq = env.ledger().sequence();

    client.upsert_anchor(&admins, &asset, &anchor);

    let live_until_after_write = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Anchor(asset.clone()))
    });

    // The write path bumps with ANCHOR_TTL_BUMP (via extend_ttl with the
    // ANCHOR_TTL threshold/bump pair), so the remaining TTL (live-until
    // minus the current ledger) must clear the anchor bump policy.
    assert!(
        live_until_after_write - seq >= ANCHOR_TTL_BUMP,
        "write path remaining TTL {} < ANCHOR_TTL_BUMP ({ANCHOR_TTL_BUMP})",
        live_until_after_write - seq
    );

    assert_eq!(client.get_anchor(&asset), Some(anchor));

    let live_until_after_read = env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Anchor(asset.clone()))
    });

    // The read path uses the same ANCHOR_TTL_THRESHOLD / ANCHOR_TTL_BUMP,
    // so it never shrinks the policy established by the write.
    assert!(
            live_until_after_read >= live_until_after_write,
            "read path live-until ({live_until_after_read}) should not be below write path ({live_until_after_write})"
        );
}

#[test]
fn anchor_upsert_overwrites_existing_anchor() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor_one = Address::generate(&env);
    let anchor_two = Address::generate(&env);

    client.upsert_anchor(&admins, &asset, &anchor_one);
    assert_eq!(client.get_anchor(&asset), Some(anchor_one));

    client.upsert_anchor(&admins, &asset, &anchor_two);
    assert_eq!(client.get_anchor(&asset), Some(anchor_two));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn rejects_fee_bps_above_max() {
    let (_env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 5_001,
        network_fee_bps: 100,
    };
    client.set_fee_config(&admins, &cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn rejects_fee_bps_below_min() {
    let (_env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 100,
        network_fee_bps: 4,
    };
    client.set_fee_config(&admins, &cfg);
}

#[test]
fn accepts_fee_bps_at_boundaries() {
    let (_env, client, admins, _recovery) = setup();
    client.set_fee_config(
        &admins,
        &FeeConfig {
            platform_fee_bps: 5,
            network_fee_bps: 5,
        },
    );
    client.set_fee_config(
        &admins,
        &FeeConfig {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_000,
        },
    );
}

proptest! {
    #[test]
    fn valid_fee_configs_are_accepted(
        (platform_fee_bps, network_fee_bps) in
            (5u32..=5_000, 5u32..=5_000)
                .prop_filter("fee sum must fit the denominator", |(platform, network)| {
                    *platform + *network <= BPS_DENOMINATOR
                }),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let recovery = Address::generate(&env);
        let admins = vec![&env, admin];
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery);

        let config = FeeConfig {
            platform_fee_bps,
            network_fee_bps,
        };
        client.set_fee_config(&admins, &config);
        let stored = client.get_fee_config().unwrap();
        prop_assert_eq!(stored.platform_fee_bps, platform_fee_bps);
        prop_assert_eq!(stored.network_fee_bps, network_fee_bps);
    }

    #[test]
    fn fee_configs_with_an_out_of_range_leg_are_rejected(
        platform_fee_bps in 0u32..=5_000,
        network_fee_bps in 0u32..=5_000,
        invalid_platform in any::<bool>(),
    ) {
        let invalid_value = if invalid_platform {
            5_001
        } else {
            4
        };
        let config = if invalid_platform {
            FeeConfig {
                platform_fee_bps: invalid_value,
                network_fee_bps,
            }
        } else {
            FeeConfig {
                platform_fee_bps,
                network_fee_bps: invalid_value,
            }
        };
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let recovery = Address::generate(&env);
        let admins = vec![&env, admin];
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
        client.init(&deployer, &admins, &1, &recovery);

        prop_assert!(client.try_set_fee_config(&admins, &config).is_err());
    }

    #[test]
    fn threshold_validation_accepts_exact_admin_count_and_rejects_out_of_range(
        admin_count in 1u32..=5,
        threshold in 0u32..=6,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let mut admins = Vec::new(&env);
        for _ in 0..admin_count {
            admins.push_back(Address::generate(&env));
        }
        let recovery = Address::generate(&env);
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
        let deployer = Address::generate(&env);

        let result = client.try_init(&deployer, &admins, &threshold, &recovery);
        if threshold == 0 || threshold > admin_count {
            prop_assert!(result.is_err());
        } else {
            prop_assert!(result.is_ok());
            prop_assert_eq!(client.get_threshold(), threshold);
        }
    }
}

#[test]
#[should_panic]
fn rejects_removing_unknown_anchor() {
    let (env, client, admins, _recovery) = setup();
    let missing_asset = Address::generate(&env);
    client.remove_anchor(&admins, &missing_asset);
}

#[test]
fn checks_if_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    assert!(!client.is_initialized());
    let deployer = Address::generate(&env);
    client.init(&deployer, &vec![&env, admin.clone()], &1, &recovery_address);
    assert!(client.is_initialized());
}

#[test]
fn emits_event_on_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);

    let admins = vec![&env, admin.clone()];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery_address);
    assert!(client.is_initialized());
    assert_eq!(client.get_admin(), admins);
    assert_eq!(client.get_threshold(), 1);

    let events = env.events().all();
    assert_eq!(events.len(), 1, "exactly one event emitted on init");

    let (_contract_id, topics, data) = events.get(0).unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, events::INITIALIZED_EVENT)
    );
    assert_eq!(Address::from_val(&env, &data), admin);
}

// `Symbol`'s 32-character limit is enforced by the Stellar protocol
// itself (SCSYMBOL_LIMIT) at construction time, not merely by an SDK
// convenience check. `Symbol::new` below panics with
// `Error(Value, InvalidInput)` before `update_system_param` is ever
// invoked, so there is no public API path to construct an in-memory
// `Symbol` over 32 characters. The test documents this protocol-level
// invariant and ensures the contract does not assert a code path that
// cannot be reached through it.
#[test]
#[should_panic(expected = "Error(Value, InvalidInput)")]
fn rejects_oversized_symbol_key() {
    let (env, client, admins, _recovery) = setup();
    let oversized = "this_is_a_very_long_system_parameter_key";
    let key = Symbol::new(&env, oversized);
    client.update_system_param(&admins, &key, &123);
}

#[test]
fn accepts_valid_symbol_key() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "valid_key_32_chars_or_less");
    client.update_system_param(&admins, &key, &123);
    assert_eq!(client.get_system_param(&key), Some(123));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn rejects_operation_when_signatures_below_threshold() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "key");
    // threshold is 2, but only 1 signer provided
    let single_signer = vec![&env, admins.get(0).unwrap()];
    client.update_system_param(&single_signer, &key, &100);
}

#[test]
fn changes_threshold_with_valid_signatures() {
    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let a3 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone(), a3.clone()];
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery);

    assert_eq!(client.get_threshold(), 1);

    let signers = vec![&env, a1.clone()];
    client.change_threshold(&signers, &2);
    assert_eq!(client.get_threshold(), 2);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn change_threshold_fails_with_insufficient_signatures() {
    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &recovery);

    // Current threshold is 2, needs 2 signatures for change_threshold, but only 1 provided.
    let single_signer = vec![&env, a1.clone()];
    client.change_threshold(&single_signer, &1);
}

#[test]
fn gov_two_of_two_can_lower_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &recovery);

    assert_eq!(client.get_threshold(), 2);

    // 2-of-2 can lower threshold to 1 with both signers
    client.change_threshold(&admins, &1);
    assert_eq!(client.get_threshold(), 1);

    // Zero threshold is still rejected
    let zero_res = client.try_change_threshold(&admins, &0);
    assert_eq!(
        zero_res.unwrap_err().unwrap(),
        GovernanceError::InvalidThreshold.into()
    );
}

// Issue #565: setting a threshold above the admin count must surface
// `InvalidThreshold` (#14), not `Unauthorized` (#3) from the auth gate.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn change_threshold_above_admin_count_rejects_with_invalid_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery);

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
    let admins = vec![&env, a1.clone(), a2.clone()];
    let recovery = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &recovery);

    client.change_threshold(&admins, &0);
}

// -----------------------------------------------------------------------
// Recovery timing / race tests
// -----------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn execute_recovery_rejects_before_delay() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &vec![&env, admin], &1, &recovery);

    client.initiate_recovery(&new_admin);
    // Do NOT advance the ledger — the delay is still active.
    client.execute_recovery();
}

#[test]
fn execute_recovery_clears_pending_record() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &vec![&env, admin], &1, &recovery);

    client.initiate_recovery(&new_admin);

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
    let recovery = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let admins = vec![&env, admin];
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery);

    client.initiate_recovery(&new_admin);
    client.cancel_recovery(&admins);

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
    let recovery = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &vec![&env, admin], &1, &recovery);

    client.initiate_recovery(&new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();

    // Second execute after the pending record has been consumed.
    client.execute_recovery();
}

#[test]
fn transfers_admin_successfully() {
    let (env, client, admins, _recovery) = setup();
    let new_a1 = Address::generate(&env);
    let new_admins = vec![&env, new_a1.clone()];
    client.transfer_admin(&admins, &new_admins, &1);
    assert_eq!(client.get_admin(), new_admins);
    assert_eq!(client.get_threshold(), 1);
}

#[test]
fn pause_then_unpause_round_trip_succeeds() {
    let (env, client, admins, _recovery) = setup();

    assert!(!client.is_paused());

    let before_pause = env.events().all().len();
    client.pause(&admins);
    assert!(client.is_paused());
    assert!(env.events().all().len() > before_pause);

    let before_unpause = env.events().all().len();
    client.unpause(&admins);
    assert!(!client.is_paused());
    assert!(env.events().all().len() > before_unpause);
}

/// Pins `pause`/`unpause` to `bettapay_common::events`' canonical topic
/// constants rather than a locally inlined string, so this test fails if
/// either entry point stops routing through the shared emit helper.
#[test]
fn pause_and_unpause_emit_canonical_shared_topics() {
    let (env, client, admins, _recovery) = setup();

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
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn initiate_recovery_rejects_overwrite_while_pending() {
    let env = Env::default();
    env.mock_all_auths();
    let admin1 = Address::generate(&env);
    let admin2 = Address::generate(&env);
    let admins = vec![&env, admin1, admin2];
    let recovery_address = Address::generate(&env);
    let first_target = Address::generate(&env);
    let second_target = Address::generate(&env);

    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &2, &recovery_address);

    client.initiate_recovery(&first_target);

    // Second initiation must be rejected — a recovery is already pending.
    client.initiate_recovery(&second_target);
}

// -----------------------------------------------------------------------
// InvalidWasmInterface: upgrade flow enforces supports_interface(1)
// -----------------------------------------------------------------------

/// Uploading an empty Wasm (which has no `supports_interface` export)
/// must be rejected with `InvalidWasmInterface` (code 13).
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn upgrade_rejects_wasm_missing_supports_interface() {
    let (env, client, admins, _recovery) = setup();
    // Empty wasm has no exports — the probe call will fail, raising the typed error.
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    client.upgrade(&admins, &bad_hash);
}

/// Upgrading with a non-admin caller must still be rejected with
/// `Unauthorized` (code 3), showing auth is checked before interface probing.
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn upgrade_rejects_non_admin_before_interface_check() {
    let (env, client, _admins, _recovery) = setup();
    let non_admin = Address::generate(&env);
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));
    client.upgrade(&soroban_sdk::vec![&env, non_admin], &bad_hash);
}

/// A Wasm hash that was never uploaded (`upload_contract_wasm` was never
/// called for it) cannot be probed: protocol 21 exposes no wasm-presence
/// check to contract code, so the probe deployment traps with a
/// host-level `Storage`/`MissingValue` error ("Wasm does not exist")
/// rather than the typed `InvalidWasmInterface`. Documented here so the
/// boundary of the typed-error guarantee is explicit — see
/// `bettapay_common::upgrade::probe_supports_interface`.
#[test]
#[should_panic(expected = "Error(Storage, MissingValue)")]
fn upgrade_rejects_never_uploaded_wasm_hash() {
    let (env, client, admins, _recovery) = setup();
    let garbage = soroban_sdk::BytesN::from_array(&env, &[0x47u8; 32]);
    client.upgrade(&admins, &garbage);
}

/// Issue #473: `contract_upgraded` is published at the canonical point —
/// after auth and interface validation, immediately before the code
/// swap. A rejected upgrade must therefore emit no event at all,
/// pinning the event's position relative to the validation step.
#[test]
fn upgrade_emits_no_event_on_failed_upgrade() {
    let (env, client, admins, _recovery) = setup();
    let bad_hash = env
        .deployer()
        .upload_contract_wasm(soroban_sdk::Bytes::from_slice(&env, &[]));

    let before = env.events().all().len();
    let result = client.try_upgrade(&admins, &bad_hash);
    assert!(result.is_err(), "non-conforming wasm must be rejected");
    assert_eq!(
        env.events().all().len(),
        before,
        "no contract_upgraded event on failed upgrade"
    );
}

// -----------------------------------------------------------------------
// Issue #507: schema-version marker + migrate skeleton
// -----------------------------------------------------------------------

#[test]
fn init_writes_schema_version_marker_and_migrate_is_idempotent() {
    let (env, client, admins, _recovery) = setup();

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
        Symbol::new(&env, events::MIGRATED_EVENT)
    );
}

/// Issue #984: version-bump simulation. A stored `SchemaVersion` below
/// `CURRENT_SCHEMA_VERSION` (as after an upgrade that bumped the constant) is
/// advanced by `migrate`, and a second call is then a no-op.
#[test]
fn migrate_advances_schema_version_below_current() {
    let (env, client, admins, _recovery) = setup();
    let read_version = || {
        env.as_contract(&client.address, || {
            env.storage()
                .instance()
                .get::<_, u32>(&DataKey::SchemaVersion)
        })
    };

    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &(CURRENT_SCHEMA_VERSION - 1));
    });
    assert_eq!(read_version(), Some(CURRENT_SCHEMA_VERSION - 1));

    client.migrate(&admins);
    assert_eq!(read_version(), Some(CURRENT_SCHEMA_VERSION));

    client.migrate(&admins);
    assert_eq!(read_version(), Some(CURRENT_SCHEMA_VERSION));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn migrate_is_blocked_while_paused() {
    let (_env, client, admins, _recovery) = setup();
    client.pause(&admins);
    client.migrate(&admins);
}

// -----------------------------------------------------------------------
// Issue #514: execute_recovery repairs a corrupt/empty admin set
// -----------------------------------------------------------------------

#[test]
fn execute_recovery_repairs_an_empty_corrupt_admin_set() {
    use soroban_sdk::testutils::Ledger;
    let (env, client, _admins, _recovery_address) = setup();
    let recovered = Address::generate(&env);

    client.initiate_recovery(&recovered);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + RECOVERY_DELAY_SECONDS + 1);

    // Corrupt the admin entry: overwrite with an empty admin set so there
    // is no primary admin to read when building the recovery event.
    env.as_contract(&client.address, || {
        let empty: Vec<Address> = Vec::new(&env);
        env.storage().instance().set(&DataKey::Admin, &empty);
    });

    client.execute_recovery();

    assert_eq!(client.get_admin(), vec![&env, recovered.clone()]);
    assert_eq!(client.get_threshold(), 1);
}

// -----------------------------------------------------------------------
// Issue #515: governance admin reads use the 50k/100k instance policy
// -----------------------------------------------------------------------

#[test]
fn get_admin_uses_50k_100k_instance_ttl_policy() {
    use soroban_sdk::testutils::storage::Instance;
    let (env, client, _admins, _recovery) = setup();

    client.get_admin();

    let ttl = env.as_contract(&client.address, || env.storage().instance().get_ttl());
    assert!(
        ttl >= READ_INSTANCE_TTL_BUMP,
        "expected get_admin to bump instance TTL to at least {READ_INSTANCE_TTL_BUMP}, got {ttl}"
    );
}

// -----------------------------------------------------------------------
// Issue #516: reconciled pause matrix
// -----------------------------------------------------------------------

#[test]
fn pause_blocks_fee_and_anchor_writes() {
    let (env, client, admins, _recovery) = setup();
    client.pause(&admins);
    assert!(client.is_paused());

    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };

    assert!(
        client.try_set_fee_config(&admins, &cfg).is_err(),
        "set_fee_config must be blocked while paused"
    );
    assert!(
        client.try_upsert_anchor(&admins, &asset, &anchor).is_err(),
        "upsert_anchor must be blocked while paused"
    );
    assert!(
        client.try_remove_anchor(&admins, &asset).is_err(),
        "remove_anchor must be blocked while paused"
    );
}

#[test]
fn pause_allows_admin_transfer_threshold_and_recovery() {
    use soroban_sdk::testutils::Ledger;
    let env = Env::default();
    env.mock_all_auths();
    let a1 = Address::generate(&env);
    let a2 = Address::generate(&env);
    let admins = vec![&env, a1.clone(), a2.clone()];
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(&env, &contract_id);
    let deployer = Address::generate(&env);
    client.init(&deployer, &admins, &1, &recovery_address);

    client.pause(&admins);
    assert!(client.is_paused());

    // change_threshold (threshold 1 -> needs threshold = 1 signer)
    client.change_threshold(&admins, &2);
    assert_eq!(client.get_threshold(), 2);

    // transfer_admin (threshold 2 -> needs 2 signers)
    let new_a = Address::generate(&env);
    client.transfer_admin(&admins, &vec![&env, new_a.clone()], &1);
    assert_eq!(client.get_admin(), vec![&env, new_a.clone()]);

    // recovery flow
    let recovered = Address::generate(&env);
    client.initiate_recovery(&recovered);
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + RECOVERY_DELAY_SECONDS + 1);
    client.execute_recovery();
    assert_eq!(client.get_admin(), vec![&env, recovered.clone()]);
    assert_eq!(client.get_threshold(), 1);
}

// -----------------------------------------------------------------------
// Recovery initiator (issue #560)
// -----------------------------------------------------------------------

// The pending recovery must record the address that initiated it, so
// `cancel_recovery` can validate the cancellation against the initiator
// and off-chain consumers can audit who started the recovery.
#[test]
fn pending_recovery_records_initiating_address() {
    let (env, client, _admins, recovery_address) = setup();
    let new_admin = Address::generate(&env);

    client.initiate_recovery(&new_admin);

    let pending: PendingRecovery = env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .get(&CommonDataKey::PendingRecovery)
            .unwrap()
    });
    assert_eq!(pending.initiated_by, recovery_address);
    assert_eq!(pending.new_admin, new_admin);
}

// The address that initiated a recovery may cancel it directly. The
// admin gate is unchanged, so this is an additional canceller, not a
// replacement.
#[test]
fn cancel_recovery_by_initiator_succeeds() {
    let (env, client, _admins, recovery_address) = setup();
    let new_admin = Address::generate(&env);
    client.initiate_recovery(&new_admin);

    client.cancel_recovery(&vec![&env, recovery_address]);

    assert!(
        !env.as_contract(&client.address, || env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)),
        "a cancelled recovery must be removed from storage"
    );
}

// A caller that is neither an admin nor the recorded initiator is
// refused, and the pending recovery survives the refused attempt.
#[test]
fn cancel_recovery_by_non_initiator_non_admin_is_refused() {
    let (env, client, _admins, _recovery_address) = setup();
    let new_admin = Address::generate(&env);
    client.initiate_recovery(&new_admin);

    let stranger = Address::generate(&env);
    let result = client.try_cancel_recovery(&vec![&env, stranger]);
    assert_eq!(
        result,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            GovernanceError::Unauthorized as u32
        )))
    );
    assert!(
        env.as_contract(&client.address, || env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)),
        "a refused cancellation must leave the pending recovery in place"
    );
}

// The pre-existing admin path is preserved: the admin set can still
// cancel a recovery it did not initiate.
#[test]
fn cancel_recovery_by_admin_still_succeeds() {
    let (env, client, admins, _recovery_address) = setup();
    let new_admin = Address::generate(&env);
    client.initiate_recovery(&new_admin);

    client.cancel_recovery(&admins);

    assert!(
        !env.as_contract(&client.address, || env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)),
        "the admin path must still be able to cancel a pending recovery"
    );
}

// -----------------------------------------------------------------------
// Below-minimum fee config rejection
// -----------------------------------------------------------------------

#[test]
fn below_min_fee_config_rejected_without_overwriting_config() {
    let (_env, client, admins, _recovery) = setup();
    let previous = FeeConfig {
        platform_fee_bps: MIN_FEE_BPS,
        network_fee_bps: MIN_FEE_BPS,
    };
    client.set_fee_config(&admins, &previous);

    let invalid = FeeConfig {
        platform_fee_bps: MIN_FEE_BPS - 1,
        network_fee_bps: MIN_FEE_BPS,
    };
    let result = client.try_set_fee_config(&admins, &invalid);
    assert_eq!(
        result.unwrap_err().unwrap(),
        GovernanceError::InvalidFeeBps.into()
    );

    let stored = client.get_fee_config().expect("previous config remains");
    assert_eq!(stored.platform_fee_bps, previous.platform_fee_bps);
    assert_eq!(stored.network_fee_bps, previous.network_fee_bps);
}

#[test]
fn min_fee_config_accepted() {
    let (_env, client, admins, _recovery) = setup();
    let cfg = FeeConfig {
        platform_fee_bps: 5,
        network_fee_bps: 5,
    };
    assert!(
        client.try_set_fee_config(&admins, &cfg).is_ok(),
        "FeeConfig {{5, 5}} at MIN_FEE_BPS must succeed"
    );
}

// -----------------------------------------------------------------------
// Self-anchor rejection (asset == anchor)
// -----------------------------------------------------------------------

#[test]
fn self_anchor_rejected() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    assert!(
        client.try_upsert_anchor(&admins, &asset, &asset).is_err(),
        "upsert_anchor with asset==anchor must fail"
    );
}

#[test]
fn distinct_anchor_accepted() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    assert!(
        client.try_upsert_anchor(&admins, &asset, &anchor).is_ok(),
        "upsert_anchor with distinct asset and anchor must succeed"
    );
}

// -----------------------------------------------------------------------
// Negative system param rejection
// -----------------------------------------------------------------------

#[test]
fn negative_system_param_rejected() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "min_payment");
    assert!(
        client.try_update_system_param(&admins, &key, &-1).is_err(),
        "update_system_param with value -1 must fail with InvalidParamValue"
    );
}

#[test]
fn non_negative_system_param_accepted() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "min_payment");
    assert!(
        client.try_update_system_param(&admins, &key, &0).is_ok(),
        "update_system_param with value 0 must succeed"
    );
    assert_eq!(client.get_system_param(&key), Some(0));
}

// -----------------------------------------------------------------------
// Issue #743: bypass_gov_fees circuit-breaker is a 0/1 flag
// -----------------------------------------------------------------------

#[test]
fn bypass_gov_fees_accepts_zero_and_one() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "bypass_gov_fees");

    client.update_system_param(&admins, &key, &1);
    assert_eq!(client.get_system_param(&key), Some(1));

    client.update_system_param(&admins, &key, &0);
    assert_eq!(client.get_system_param(&key), Some(0));
}

#[test]
fn bypass_gov_fees_rejects_values_above_one() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "bypass_gov_fees");

    assert!(
        client.try_update_system_param(&admins, &key, &2).is_err(),
        "bypass_gov_fees above 1 must fail with InvalidParamValue"
    );
    assert_eq!(
        client.get_system_param(&key),
        None,
        "a rejected flag must not be stored"
    );
}

#[test]
fn other_system_params_are_not_restricted_to_boolean_values() {
    let (env, client, admins, _recovery) = setup();
    let key = Symbol::new(&env, "max_settle");
    client.update_system_param(&admins, &key, &1440);
    assert_eq!(client.get_system_param(&key), Some(1440));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn paused_with_bad_auth_yields_paused_over_unauthorized() {
    let (env, client, admins, _recovery) = setup();
    client.pause(&admins);
    assert!(client.is_paused());

    let non_admin = Address::generate(&env);
    let cfg = FeeConfig {
        platform_fee_bps: 120,
        network_fee_bps: 35,
    };
    // Must fail with Paused (#5) rather than Unauthorized (#3)
    client.set_fee_config(&soroban_sdk::vec![&env, non_admin], &cfg);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn upsert_anchor_rejects_zero_address_asset() {
    let (env, client, admins, _recovery) = setup();
    let zero = bettapay_common::storage::zero_address(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admins, &zero, &anchor);
}

#[test]
fn upsert_anchor_accepts_valid_address() {
    let (env, client, admins, _recovery) = setup();
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    client.upsert_anchor(&admins, &asset, &anchor);
    assert_eq!(client.get_anchor(&asset), Some(anchor));
}

#[test]
fn zero_address_and_empty_string_governance_validation() {
    let (env, client, admins, _recovery) = setup();
    let zero = bettapay_common::storage::zero_address(&env);

    // Transferring admin to zero address must fail with InvalidAdmin (6)
    let res = client.try_transfer_admin(&admins, &soroban_sdk::vec![&env, zero.clone()], &1);
    assert_eq!(
        res.unwrap_err().unwrap(),
        GovernanceError::InvalidAdmin.into()
    );

    // Updating recovery address to zero address must fail with InvalidRecoveryAddress (7)
    let res = client.try_update_recovery_address(&admins, &zero);
    assert_eq!(
        res.unwrap_err().unwrap(),
        GovernanceError::InvalidRecoveryAddress.into()
    );

    // Initiating recovery with zero address target must fail with InvalidAdmin (6)
    let res = client.try_initiate_recovery(&zero);
    assert_eq!(
        res.unwrap_err().unwrap(),
        GovernanceError::InvalidAdmin.into()
    );

    // Empty-string address input fails conversion before invocation
    let empty_str = soroban_sdk::String::from_str(&env, "");
    let empty_res = Address::try_from_string(&empty_str);
    assert!(
        empty_res.is_err(),
        "empty string must fail Address conversion before contract invocation"
    );
}
