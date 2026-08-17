//! Cross-contract end-to-end integration test exercising the full BettaPay
//! payment lifecycle with both the governance and settlement contracts
//! deployed together on the same test environment.
//!
//! Lifecycle covered:
//!   1. Deploy GovernanceContract and SettlementContract together.
//!   2. Initialise both contracts (governance first, then settlement pointing to it).
//!   3. Governance: set fee ceilings via `set_fee_config` and a system param via
//!      `update_system_param`.
//!   4. Settlement: register a merchant.
//!   5. Settlement: set a custom settlement rule on the merchant — this calls
//!      back into governance via `validate_fee_against_governance`, exercising
//!      the governance↔settlement fee-ceiling cross-contract call.
//!   6. Settlement: set a global default rule — also validated against governance.
//!   7. Fee resolution: store a payment reference.  The settlement contract
//!      walks the rule-resolution chain (merchant rule → default rule →
//!      governance FeeConfig → bootstrap) and calculates the fee split.
//!   8. Verify: the stored `PaymentRecord` reflects the correct snapshot of
//!      the rule and the computed amounts match the BPS configuration.
//!   9. Recovery (settlement): initiate → advance ledger time → execute.
//!  10. Recovery (governance): initiate → advance ledger time → execute.
//!
//! This test is designed to be wired into CI so any drift between the two
//! contracts or any deployment-script regression gets caught early.

use super::*;
use bettapay_common::constants::RECOVERY_DELAY_SECONDS;
use governance_contract::GovernanceContractClient;
use soroban_sdk::testutils::{Address as _, BytesN as _, Ledger as _};
use soroban_sdk::{vec, Address, BytesN, Env, Symbol, Vec};

use crate::SettlementContractClient;
use crate::types::SettlementRule;

/// Builds a fresh cross-contract test environment with both contracts
/// registered in the same `Env` but **not yet initialised**.
///
/// Returns `(env, governance_client, settlement_client, admin, recovery_address, merchant)`.
fn setup_both_uninitialized(
) -> (
    Env,
    GovernanceContractClient<'static>,
    SettlementContractClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov_id = env.register_contract(None, governance_contract::GovernanceContract);
    let gov_client = GovernanceContractClient::new(&env, &gov_id);

    let set_id = env.register_contract(None, settlement_contract::SettlementContract);
    let set_client = SettlementContractClient::new(&env, &set_id);

    (env, gov_client, set_client, admin, recovery_address, merchant)
}

/// Initialises both contracts to a usable post-deployment baseline:
///   - Governance: single-admin multisig (threshold 1).
///   - Settlement: same admin, points at the governance contract.
///
/// Returns the same tuple as [`setup_both_uninitialized`] after `init` calls
/// have been performed, with the admin set / threshold used for each contract.
fn setup_both_initialized() -> (
    Env,
    GovernanceContractClient<'static>,
    SettlementContractClient<'static>,
    Vec<Address>,
    Address,
    Address,
) {
    let (env, gov_client, set_client, admin, recovery_address, merchant) =
        setup_both_uninitialized();

    let admins = vec![&env, admin.clone()];
    let threshold = 1u32;

    // Governance must be initialised first so that its `get_fee_config` entry
    // point exists (and returns `None` — "no explicit config yet") by the
    // time settlement's `init` runs `validate_governance`.
    gov_client.init(&admins, &threshold, &recovery_address);
    set_client.init(&admins, &threshold, &gov_client.address, &recovery_address);

    (env, gov_client, set_client, admins, recovery_address, merchant)
}

/// The canonical end-to-end lifecycle test covering register-merchant →
/// store-payment → fee-resolution → recovery for BOTH contracts.
#[test]
fn full_payment_lifecycle_with_governance_and_settlement() {
    // ------------------------------------------------------------------
    // Phase 0 — bootstrap both contracts from scratch (mimics deploy.sh)
    // ------------------------------------------------------------------
    let (env, gov_client, set_client, admins, recovery_address, merchant) =
        setup_both_initialized();

    assert!(
        gov_client.is_initialized(),
        "governance contract reports initialized after init"
    );
    assert!(
        set_client.is_initialized(),
        "settlement contract reports initialized after init"
    );
    assert_eq!(
        set_client.get_governance(),
        gov_client.address,
        "settlement contract stores governance address"
    );

    // ------------------------------------------------------------------
    // Phase 1 — governance configures protocol-wide ceilings
    // ------------------------------------------------------------------
    // Platform fee ceiling: 500 bps (5 %), network fee ceiling: 200 bps (2 %).
    let gov_fee_cfg = governance_contract::FeeConfig {
        platform_fee_bps: 500,
        network_fee_bps: 200,
    };
    gov_client.set_fee_config(&admins, &gov_fee_cfg);

    let read_back = gov_client
        .get_fee_config()
        .expect("governance fee config should be readable");
    assert_eq!(read_back.platform_fee_bps, 500);
    assert_eq!(read_back.network_fee_bps, 200);

    // Propagate a numeric system parameter (e.g. a settlement delay cap).
    let max_delay_key = Symbol::new(&env, "max_settle_d");
    gov_client.update_system_param(&admins, &max_delay_key, &14_400i128);
    assert_eq!(
        gov_client.get_system_param(&max_delay_key),
        Some(14_400),
        "system param propagates from governance"
    );

    // Register an anchor in governance to exercise the anchor registry path.
    let asset = Address::generate(&env);
    let anchor = Address::generate(&env);
    gov_client.upsert_anchor(&admins, &asset, &anchor);
    assert_eq!(
        gov_client.get_anchor(&asset),
        Some(anchor.clone()),
        "anchor registry round-trips through governance"
    );

    // ------------------------------------------------------------------
    // Phase 2 — settlement: register merchant and set rules
    // ------------------------------------------------------------------
    set_client.register_merchant(&admins, &merchant);
    assert!(
        set_client.is_merchant_registered(merchant.clone()),
        "merchant is registered after register_merchant call"
    );

    // A merchant-specific rule that is below the governance-set ceilings.
    //   platform: 200 bps (≤ 500), network: 50 bps (≤ 200).
    let merchant_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 100,
        auto_settle: true,
    };
    // This call cross-contract-invokes governance::get_fee_config through
    // `validate_fee_against_governance` inside `set_settlement_rule`.
    set_client.set_settlement_rule(&admins, &merchant, &merchant_rule);

    let read_rule = set_client
        .get_settlement_rule(merchant.clone())
        .expect("merchant settlement rule should be set");
    assert_eq!(read_rule.platform_fee_bps, 200);
    assert_eq!(read_rule.network_fee_bps, 50);
    assert_eq!(read_rule.settlement_delay_ledger, 100);
    assert!(read_rule.auto_settle);

    // Also exercise the global-default path.
    let default_rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 50,
        auto_settle: false,
    };
    set_client.set_default_rule(&admins, &default_rule);
    let read_default = set_client
        .get_default_rule()
        .expect("default settlement rule should be set");
    assert_eq!(read_default.platform_fee_bps, 300);
    assert_eq!(read_default.network_fee_bps, 100);

    // ------------------------------------------------------------------
    // Phase 3 — fee resolution: store a payment reference
    // ------------------------------------------------------------------
    let payment_ref = BytesN::<32>::from_array(&env, &[1u8; 32]);
    let gross_amount: i128 = 100_000; // 100k base units

    // `store_payment_reference` internally calls read_rule_or_default which
    // walks merchant → default → governance FeeConfig → bootstrap.  Because a
    // merchant rule exists, the split should reflect the 200/50 bps rule.
    let split = set_client.store_payment_reference(&merchant, &payment_ref, &gross_amount);

    assert_eq!(
        split.gross_amount, gross_amount,
        "split gross_amount matches input"
    );

    // Expected platform: ceil(100_000 * 200 / 10_000) = 2_000
    // Expected network:  ceil(100_000 *  50 / 10_000) =   500
    // Expected merchant: 100_000 - 2_000 - 500 = 97_500
    assert_eq!(split.platform_fee_amount, 2_000, "platform fee matches");
    assert_eq!(split.network_fee_amount, 500, "network fee matches");
    assert_eq!(split.merchant_amount, 97_500, "merchant net matches");

    // Read the stored PaymentRecord and verify the rule snapshot is preserved.
    let record = set_client
        .get_payment_reference(payment_ref.clone())
        .expect("payment record should exist after store_payment_reference");
    assert_eq!(record.amount, gross_amount);
    assert_eq!(record.platform_fee_bps, 200);
    assert_eq!(record.network_fee_bps, 50);
    assert_eq!(record.settlement_delay_ledger, 100);
    assert!(record.auto_settle);
    assert_eq!(record.platform_fee_amount, 2_000);
    assert_eq!(record.network_fee_amount, 500);
    assert_eq!(record.merchant_amount, 97_500);

    // Batch-read path returns the same record.
    let batch_refs = vec![&env, payment_ref.clone()];
    let batch = set_client.get_payments(&batch_refs);
    assert_eq!(batch.len(), 1);
    let batch_record = batch.get(0).unwrap().unwrap();
    assert_eq!(batch_record.amount, gross_amount);
    assert_eq!(batch_record.platform_fee_bps, 200);

    // ------------------------------------------------------------------
    // Phase 4 — governance fee-ceiling enforcement
    // ------------------------------------------------------------------
    // Attempting a settlement rule that exceeds governance's platform-fee
    // ceiling (500 bps) must panic via the cross-contract validation.
    let over_ceiling_rule = SettlementRule {
        platform_fee_bps: 600,        // above governance's 500 ceiling
        network_fee_bps: 50,          // within ceiling
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    let result = std::panic::catch_unwind(|| {
        set_client.set_settlement_rule(&admins, &merchant, &over_ceiling_rule)
    });
    assert!(result.is_err(), "settlement rule above governance ceiling must be rejected by cross-contract validation");

    // ------------------------------------------------------------------
    // Phase 5 — settlement contract recovery lifecycle
    // ------------------------------------------------------------------
    let new_settlement_admin = Address::generate(&env);
    set_client.initiate_recovery(&new_settlement_admin);
    // Advance the ledger timestamp past the recovery delay.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    set_client.execute_recovery();
    assert_eq!(
        set_client.get_admin(),
        vec![&env, new_settlement_admin.clone()],
        "settlement recovery promotes the new admin"
    );
    assert_eq!(
        set_client.get_threshold(),
        1,
        "settlement recovery resets threshold to 1"
    );

    // ------------------------------------------------------------------
    // Phase 6 — governance contract recovery lifecycle
    // ------------------------------------------------------------------
    let new_governance_admin = Address::generate(&env);
    gov_client.initiate_recovery(&new_governance_admin);
    // The timestamp was already bumped in phase 5, but bump again to be safe
    // against a future protocol change that ties the two delays to different
    // constants.
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    gov_client.execute_recovery();
    assert_eq!(
        gov_client.get_admin(),
        vec![&env, new_governance_admin.clone()],
        "governance recovery promotes the new admin"
    );
    assert_eq!(
        gov_client.get_threshold(),
        1,
        "governance recovery resets threshold to 1"
    );

    // Sanity: both contracts remain callable after recovery.
    assert!(gov_client.is_initialized());
    assert!(set_client.is_initialized());
    assert_eq!(
        gov_client.get_anchor(&asset),
        Some(anchor),
        "governance storage survives recovery"
    );
    assert_eq!(
        set_client.get_payment_reference(payment_ref).unwrap().amount,
        gross_amount,
        "settlement storage survives recovery"
    );
}

/// Verifies that the settlement rule-resolution fallback chain actually
/// consults the governance `FeeConfig` when neither a merchant-specific rule
/// nor a global default has been stored (i.e. the `read_governance_fee_rule`
/// cross-contract path in `storage::read_rule_or_default`).
#[test]
fn fee_resolution_falls_back_to_governance_fee_config() {
    let (env, gov_client, set_client, admins, _recovery_address, merchant) =
        setup_both_initialized();

    // Register the merchant but DO NOT set a merchant rule or a default rule.
    set_client.register_merchant(&admins, &merchant);

    // Governance sets a FeeConfig — this should now drive settlement's split.
    gov_client.set_fee_config(
        &admins,
        &governance_contract::FeeConfig {
            platform_fee_bps: 300,
            network_fee_bps: 150,
        },
    );

    let payment_ref = BytesN::<32>::from_array(&env, &[2u8; 32]);
    let gross: i128 = 200_000;

    // The split should come from governance's FeeConfig (300 / 150 bps).
    //   platform: ceil(200_000 * 300 / 10_000) = 6_000
    //   network:  ceil(200_000 * 150 / 10_000) = 3_000
    //   merchant: 200_000 - 6_000 - 3_000 = 191_000
    let split = set_client.store_payment_reference(&merchant, &payment_ref, &gross);
    assert_eq!(split.platform_fee_amount, 6_000);
    assert_eq!(split.network_fee_amount, 3_000);
    assert_eq!(split.merchant_amount, 191_000);

    let record = set_client.get_payment_reference(payment_ref).unwrap();
    assert_eq!(record.platform_fee_bps, 300);
    assert_eq!(record.network_fee_bps, 150);
    // settlement_delay_ledger and auto_settle fall back to the governance
    // wrapper defaults in `read_governance_fee_rule`: 0 / false.
    assert_eq!(record.settlement_delay_ledger, 0);
    assert!(!record.auto_settle);
}

/// Verifies that `update_governance` on the settlement contract correctly
/// re-points it at a freshly-deployed governance contract with a stricter
/// fee ceiling, and that the new ceiling is honoured on the next
/// `set_settlement_rule` call.  This exercises the full update path that a
/// live deployment would use when rotating governance instances.
#[test]
fn settlement_can_rotate_governance_and_enforces_new_ceiling() {
    // Initialise with governance "v1".
    let (env, gov_v1_client, set_client, admins, recovery_address, merchant) =
        setup_both_initialized();

    gov_v1_client.set_fee_config(
        &admins,
        &governance_contract::FeeConfig {
            platform_fee_bps: 5_000,
            network_fee_bps: 5_000,
        },
    );

    set_client.register_merchant(&admins, &merchant);

    // Deploy and initialise governance "v2" with a much tighter ceiling.
    let gov_v2_id = env.register_contract(None, governance_contract::GovernanceContract);
    let gov_v2_client = GovernanceContractClient::new(&env, &gov_v2_id);
    gov_v2_client.init(&admins, &1, &recovery_address);
    gov_v2_client.set_fee_config(
        &admins,
        &governance_contract::FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 50,
        },
    );

    // Re-point settlement at v2.
    set_client.update_governance(&admins, &gov_v2_client.address);
    assert_eq!(set_client.get_governance(), gov_v2_client.address);

    // A rule that would have passed under v1 (200 bps platform) must now fail
    // under v2's 100 bps ceiling.
    let too_high_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 10,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let result = std::panic::catch_unwind(|| {
        set_client.set_settlement_rule(&admins, &merchant, &too_high_rule)
    });
    assert!(
        result.is_err(),
        "rotating governance changes the enforced ceiling"
    );

    // A rule within the v2 ceiling works.
    let ok_rule = SettlementRule {
        platform_fee_bps: 50,
        network_fee_bps: 10,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    set_client.set_settlement_rule(&admins, &merchant, &ok_rule);
}
