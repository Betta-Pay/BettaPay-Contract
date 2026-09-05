use soroban_sdk::{panic_with_error, Address, Env, Symbol, TryFromVal, Val, Vec};

use bettapay_common::{
    events::{self, PendingRecovery},
    storage::{self, CommonDataKey},
};

use crate::errors::SettlementError;
use crate::types::{DataKey, GovFeeConfig, SettlementRule};
use crate::{
    BOOTSTRAP_DEFAULT_RULE, MAX_SETTLEMENT_DELAY_LEDGER, MERCHANT_TTL_BUMP, MERCHANT_TTL_THRESHOLD,
    READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
    CURRENT_SCHEMA_VERSION,
};

pub(crate) fn read_admins(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_admin(env: &Env) -> Address {
    storage::primary_admin(&read_admins(env)).unwrap()
}

/// Returns the primary admin address, or the zero-address sentinel when the
/// admin entry is missing or has no primary. Used only by `execute_recovery`,
/// which must be able to repair a corrupt admin set (issue #514 / #687).
pub(crate) fn read_optional_primary_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get::<_, Vec<Address>>(&DataKey::Admin)
        .and_then(|admins| storage::primary_admin(&admins))
        .unwrap_or_else(|| {
            Address::from_string(&soroban_sdk::String::from_str(
                env,
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            ))
        })
}

/// Validates and writes the complete admin configuration in its canonical
/// storage shape. Every admin-changing path must use this helper so the
/// `Admin` key is always encoded as `Vec<Address>` alongside its threshold.
pub(crate) fn write_admins(env: &Env, admins: &Vec<Address>, threshold: u32) {
    validate_admins_and_threshold(env, admins, threshold);
    env.storage().instance().set(&DataKey::Admin, admins);
    env.storage()
        .instance()
        .set(&CommonDataKey::Threshold, &threshold);
}

pub(crate) fn read_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&CommonDataKey::Threshold)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

/// Returns the instance-storage schema version, defaulting to the current
/// version when the marker is absent. Per governance_contract's convention,
/// an entry written before the marker existed is treated as version 1
/// (issue #507, issue #704).
pub(crate) fn read_schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SchemaVersion)
        .unwrap_or(CURRENT_SCHEMA_VERSION)
}

pub(crate) fn validate_admins_and_threshold(env: &Env, admins: &Vec<Address>, threshold: u32) {
    if threshold == 0 || threshold > admins.len() {
        panic_with_error!(env, SettlementError::InvalidThreshold);
    }
    if admins.is_empty() {
        panic_with_error!(env, SettlementError::InvalidAdmin);
    }
    for i in 0..admins.len() {
        let admin = admins.get(i).unwrap();
        validate_nonzero_address(
            env,
            &admin,
            SettlementError::ZeroAddress,
        );
        for j in (i + 1)..admins.len() {
            if admin == admins.get(j).unwrap() {
                panic_with_error!(env, SettlementError::InvalidAdmin);
            }
        }
    }
}

pub(crate) fn verify_admin_auth(env: &Env, signers: &Vec<Address>, required_count: u32) {
    let admins = read_admins(env);
    if signers.len() < required_count {
        panic_with_error!(env, SettlementError::Unauthorized);
    }
    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();
        let mut is_admin = false;
        for j in 0..admins.len() {
            if signer == admins.get(j).unwrap() {
                is_admin = true;
                break;
            }
        }
        if !is_admin {
            panic_with_error!(env, SettlementError::Unauthorized);
        }
        for j in (i + 1)..signers.len() {
            if signer == signers.get(j).unwrap() {
                panic_with_error!(env, SettlementError::Unauthorized);
            }
        }
        signer.require_auth();
    }
}

pub(crate) fn read_governance(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_recovery_address(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&CommonDataKey::RecoveryAddress)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_pending_recovery(env: &Env) -> PendingRecovery {
    // Decode by hand so a pending recovery written before `initiated_by`
    // existed (pre-issue #560) is refused with `RecoveryNotPending` instead
    // of surfacing a host-level conversion panic. Refusing is deliberate:
    // an old-format record must never be treated as a valid pending
    // recovery (default-deny, never default-allow).
    let val = env
        .storage()
        .instance()
        .get::<_, Val>(&CommonDataKey::PendingRecovery)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::RecoveryNotPending));
    PendingRecovery::try_from_val(env, &val)
        .unwrap_or_else(|_| panic_with_error!(env, SettlementError::RecoveryNotPending))
}

/// Validates that the provided governance address is a non-zero, non-empty address.
///
/// Note (Issue #124): This function intentionally avoids making a cross-contract call
/// to `governance` during `init` or `update_governance`. Making a cross-contract call
/// during initialization creates a reentrancy / DoS vector where a self-recursive or
/// broken governance contract can call back into the uninitialized settlement contract
/// (causing `NotInitialized` panics) or trap. Governance fee config validity is
/// verified at first use via `try_invoke_contract` in [`read_governance_fee_rule`]
/// and [`validate_fee_against_governance`].
pub(crate) fn validate_governance(env: &Env, governance: &Address) {
    validate_nonzero_address(
        env,
        governance,
        SettlementError::InvalidGovernance,
    );
}

pub(crate) fn validate_nonzero_address(
    env: &Env,
    address: &Address,
    zero_error: SettlementError,
) {
    if storage::is_zero_address(env, address) {
        panic_with_error!(env, zero_error);
    }
}

/// Panics with [`SettlementError::PaymentOrphaned`] when the merchant's
/// payment records are no longer readable.
///
/// Policy (issue #490): unregistering a merchant orphans its payment
/// records. `unregister_merchant` writes an `ArchivedMerchant` tombstone that
/// survives re-registration, and a merchant that was never registered has no
/// readable history either. A payment read therefore requires both a live
/// merchant marker and no tombstone.
pub(crate) fn assert_payments_readable(env: &Env, merchant: &Address) {
    let registered = is_merchant_registered_internal(env, merchant.clone());
    let archived = env
        .storage()
        .persistent()
        .has(&DataKey::ArchivedMerchant(merchant.clone()));
    if !registered || archived {
        panic_with_error!(env, SettlementError::PaymentOrphaned);
    }
}

/// Returns whether a merchant has been registered.
///
/// TTL-neutral: does not touch the merchant marker's TTL. Use this from
/// public/unauthenticated read paths (`is_merchant_registered`,
/// `calculate_fee_split`) — those are callable by anyone for any merchant
/// address, so if they bumped the TTL a third party could keep an arbitrary
/// merchant's marker alive indefinitely, subverting natural eviction.
/// Merchant- or admin-authenticated paths that need to keep an active
/// merchant's marker warm should use
/// [`is_merchant_registered_and_bump_ttl`] instead.
pub(crate) fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    env.storage().persistent().has(&key)
}

/// Returns whether a merchant has been registered, keeping the marker entry
/// warm in storage if so.
///
/// Only call this from a path that already required merchant or admin
/// authentication for this action (e.g. `store_payment_reference`,
/// `set_settlement_rule`) — never from a public/unauthenticated read, or a
/// third party could use it as a liveness oracle to keep an arbitrary
/// merchant's marker alive indefinitely. See
/// [`is_merchant_registered_internal`] for the TTL-neutral read-only check.
pub(crate) fn is_merchant_registered_and_bump_ttl(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    let exists = env.storage().persistent().has(&key);
    if exists {
        // Keep the merchant marker warm so active merchants do not expire early.
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
    }
    exists
}

/// Resolves the effective settlement rule for a merchant by preferring the merchant-specific override,
/// then falling back to the global default, and finally using the bootstrap fallback.
pub(crate) fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
    // Merchant-specific rule wins over any shared configuration.
    let merchant_key = DataKey::Rule(merchant);
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&merchant_key)
    {
        env.storage()
            .persistent()
            .extend_ttl(&merchant_key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
        return rule;
    }
    // Fall back to the admin-controlled global default when present.
    let default_key = DataKey::DefaultRule;
    if let Some(rule) = env
        .storage()
        .instance()
        .get::<_, SettlementRule>(&default_key)
    {
        return rule;
    }
    // Protocol fee source: governance's GovFeeConfig, when available.
    if let Some(rule) = read_governance_fee_rule(env) {
        return rule;
    }
    // Final fallback keeps the contract usable before any config is stored.
    // No event emitted here — the hot path runs this on every payment and
    // event spam would burn unnecessary compute (issue #691).
    BOOTSTRAP_DEFAULT_RULE
}

/// Reads the effective fallback rule without a merchant-specific override,
/// mirroring the fallback chain in [`read_rule_or_default`] (default →
/// governance → bootstrap) but **without** emitting a `bootstrap_fallback`
/// event. Used by event-emitting paths (`clear_settlement_rule`,
/// `unregister_merchant`) where the returned rule is included in a different
/// event payload and a separate bootstrap event would be misleading (issue #689).
pub(crate) fn read_fallback_rule(env: &Env) -> SettlementRule {
    let default_key = DataKey::DefaultRule;
    if let Some(rule) = env
        .storage()
        .instance()
        .get::<_, SettlementRule>(&default_key)
    {
        return rule;
    }
    if let Some(rule) = read_governance_fee_rule(env) {
        return rule;
    }
    BOOTSTRAP_DEFAULT_RULE
}

/// Attempts to read fee BPS from the configured governance contract.
///
/// Returns `None` when governance has no fee configuration yet (the governance
/// contract returned `Ok(Ok(None))`), so callers continue down the fallback
/// chain to bootstrap.  Any other failure — contract trap, host error, or
/// unexpected error value — is surfaced as the typed
/// [`SettlementError::GovernanceCallFailed`] instead of silently collapsing to
/// `None`.
pub(crate) fn read_governance_fee_rule(env: &Env) -> Option<SettlementRule> {
    let governance: Address = env.storage().instance().get(&DataKey::Governance)?;
    let args: Vec<Val> = Vec::new(env);
    match env.try_invoke_contract::<Option<GovFeeConfig>, SettlementError>(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        args,
    ) {
        // Governance returned a populated fee config — convert to a rule.
        Ok(Ok(Some(config))) => {
            let rule = SettlementRule {
                platform_fee_bps: config.platform_fee_bps,
                network_fee_bps: config.network_fee_bps,
                settlement_delay_ledger: 0,
                auto_settle: false,
            };
            if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
                panic_with_error!(env, SettlementError::InvalidSettlementDelay);
            }
            Some(rule)
        }
        // Governance has no fee config set yet — fall through to bootstrap.
        Ok(Ok(None)) => None,
        // Governance call failed (contract error or host error).
        _ => panic_with_error!(env, SettlementError::GovernanceCallFailed),
    }
}

/// Reads the minimum payment amount from the governance contract's system
/// parameters, falling back to [`crate::MIN_PAYMENT_AMOUNT`] (100) when the
/// parameter is unset or governance is unreachable (issue #690).
pub(crate) fn read_min_payment_amount(env: &Env) -> i128 {
    let governance: Option<Address> = env.storage().instance().get(&DataKey::Governance);
    let Some(governance) = governance else {
        return crate::MIN_PAYMENT_AMOUNT;
    };
    let mut args = Vec::<Val>::new(env);
    args.push_back(Symbol::new(env, "min_payment").into());
    match env.try_invoke_contract::<Option<i128>, SettlementError>(
        &governance,
        &Symbol::new(env, "get_system_param"),
        args,
    ) {
        Ok(Ok(Some(min))) => min,
        _ => crate::MIN_PAYMENT_AMOUNT,
    }
}

/// Ensures the contract is not paused before mutating state or performing privileged actions.
pub(crate) fn assert_not_paused(env: &Env) {
    if storage::is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

/// Reads the governance GovFeeConfig via cross-contract call and validates that
/// the settlement rule fees do not exceed governance's configured ceilings.
///
/// When governance has no fee config set (`Ok(Ok(None))`), local hardcoded
/// constants still apply as baseline — this function only enforces ceilings
/// that governance has explicitly configured.
///
/// Any call failure (contract trap or host error) is surfaced as the typed
/// [`SettlementError::GovernanceCallFailed`] rather than an untyped host panic.
pub(crate) fn validate_fee_against_governance(env: &Env, rule: &SettlementRule) {
    let governance: Address = read_governance(env);
    let result = env.try_invoke_contract::<Option<GovFeeConfig>, SettlementError>(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        Vec::new(env),
    );

    let fee_config = match result {
        // Governance returned a populated fee config — check fee ceilings.
        Ok(Ok(Some(cfg))) => cfg,
        // Governance has no fee config set — no ceiling to enforce.
        Ok(Ok(None)) => return,
        // Governance call failed (contract error or host error).
        _ => panic_with_error!(env, SettlementError::GovernanceCallFailed),
    };

    if rule.platform_fee_bps > fee_config.platform_fee_bps {
        panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
    }
    if rule.network_fee_bps > fee_config.network_fee_bps {
        panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
    }
}
