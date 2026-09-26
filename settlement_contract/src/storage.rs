use soroban_sdk::{panic_with_error, Address, Env, IntoVal, Map, Symbol, TryFromVal, Val, Vec};

use bettapay_common::{
    events::PendingRecovery,
    storage::{self, CommonDataKey},
};

use crate::errors::SettlementError;
use crate::types::{DataKey, GovFeeConfig, SettlementRule};
use crate::{
    BOOTSTRAP_DEFAULT_RULE, CURRENT_SCHEMA_VERSION, MAX_SETTLEMENT_DELAY_LEDGER, MERCHANT_TTL_BUMP,
    MERCHANT_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD, RULE_TTL_BUMP,
    RULE_TTL_THRESHOLD,
};

/// Governance system-parameter key for the fee circuit-breaker (issue #743).
///
/// Settlement reads this before consulting governance's fee config. A value of
/// exactly `1` means "bypass": skip the `get_fee_config` call and fall through
/// to the bootstrap default, so a broken fee config cannot block payments.
/// Anything else — unset, `0`, a trap, or a malformed value — leaves today's
/// behaviour untouched (default 0).
const BYPASS_GOV_FEES_PARAM: &str = "bypass_gov_fees";

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
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
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
        validate_nonzero_address(env, &admin, SettlementError::ZeroAddress);
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
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
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
    validate_nonzero_address(env, governance, SettlementError::InvalidGovernance);
}

pub(crate) fn validate_nonzero_address(env: &Env, address: &Address, zero_error: SettlementError) {
    if storage::is_zero_address(env, address) {
        panic_with_error!(env, zero_error);
    }
}

/// Returns whether a merchant has been registered **without** touching TTL.
///
/// This is the TTL-neutral read used by public query entry points so that a
/// read-only check never mutates storage.
pub(crate) fn is_merchant_registered_read(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    env.storage().persistent().has(&key)
}

/// Returns whether a merchant has been registered and keeps the marker entry warm in storage.
/// Panics with [`SettlementError::PaymentOrphaned`] when the merchant's
/// payment records are no longer readable.
///
/// Policy (issue #490): unregistering a merchant orphans its payment
/// records. `unregister_merchant` writes an `ArchivedMerchant` tombstone while
/// the merchant is unregistered, and a merchant that was never registered has
/// no readable history either. A payment read therefore requires both a live
/// merchant marker and no tombstone. Re-registration clears the tombstone
/// (issue #685), so a re-registered merchant's records become readable again.
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
///
/// This is a **pure resolution** function — it does NOT emit events.
/// Callers that need to signal an unconfigured deployment (e.g. mutating
/// entry points such as `store_payment_reference` or `set_settlement_rule`)
/// must check whether the returned rule equals [`BOOTSTRAP_DEFAULT_RULE`]
/// and emit `bootstrap_fallback` explicitly. Read-only paths (e.g.
/// `calculate_fee_split`) must not emit events.
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
        // fallback: default rule
        return rule;
    }
    if let Some(rule) = read_governance_fee_rule(env) {
        // fallback: governance fee config
        return rule;
    }
    // fallback: bootstrap default (no default rule or governance fee config set)
    BOOTSTRAP_DEFAULT_RULE
}

/// Reads governance's `bypass_gov_fees` circuit-breaker flag (issue #743).
///
/// Returns `true` only when governance answers with exactly `1`. This is
/// deliberately fail-safe: an unset parameter, a `0`, a governance address that
/// does not expose `get_system_param`, or any trap/typed error all return
/// `false`, preserving the pre-existing behaviour where settlement consults the
/// governance fee config.
fn governance_bypass_enabled(env: &Env, governance: &Address) -> bool {
    let mut args = Vec::<Val>::new(env);
    args.push_back(Symbol::new(env, BYPASS_GOV_FEES_PARAM).into_val(env));
    match env.try_invoke_contract::<Option<i128>, SettlementError>(
        governance,
        &Symbol::new(env, "get_system_param"),
        args,
    ) {
        Ok(Ok(Some(value))) => value == 1,
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => false,
    }
}

/// Attempts to read fee BPS from the configured governance contract.
///
/// Returns `None` in either of these cases, so callers continue down the
/// fallback chain to [`BOOTSTRAP_DEFAULT_RULE`]:
/// - Governance has no fee configuration yet (`Ok(Ok(None))`).
/// - The `get_fee_config` call fails — a contract trap, a typed error, or a
///   host error (issue #741). The payment path degrades gracefully: a broken
///   governance contract must not brick fallback payments.
///
/// The bypass circuit-breaker (issue #743) is checked first: when governance's
/// `bypass_gov_fees` system parameter is `1`, this returns `None` without
/// calling `get_fee_config` at all.
///
/// A malformed config (a map that is not exactly the two required `u32`
/// fields) still panics with [`SettlementError::GovernanceCallFailed`] rather
/// than falling back — silently skipping a configured fee ceiling is worse
/// than failing loudly (issue #483). Admin writes keep hard-failing on any
/// governance call failure via [`validate_fee_against_governance`] (issue #742).
///
/// # Settlement timing fields (issue #484)
///
/// Governance provides protocol-level fee ceilings only. The resulting
/// `SettlementRule` **always** has `settlement_delay_ledger: 0` (immediate
/// settlement) and `auto_settle: false` (no automatic settlement). These
/// values are intentionally fixed by design:
///
/// - Settlement timing is a per-merchant or admin-configured operational
///   concern, not a protocol-wide governance policy.
/// - The bootstrap default uses the same values (`0` / `false`), so
///   merchants without any rule see consistent behavior.
/// - If governance-controlled settlement timing is needed in the future,
///   extend `GovFeeConfig` and this function in a coordinated upgrade.
///
/// See also: [`GovFeeConfig`][crate::GovFeeConfig].
pub(crate) fn read_governance_fee_rule(env: &Env) -> Option<SettlementRule> {
    let governance: Address = env.storage().instance().get(&DataKey::Governance)?;

    // Issue #743: circuit-breaker before the fee-config cross-contract call.
    if governance_bypass_enabled(env, &governance) {
        return None;
    }

    // Issue #741: the read/payment path degrades to bootstrap on any failure.
    let raw_val = match env.try_invoke_contract::<Val, SettlementError>(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        Vec::new(env),
    ) {
        Ok(Ok(val)) => val,
        Ok(Err(_)) | Err(_) => return None,
    };

    let config = try_read_governance_fee_config(env, raw_val)?;

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

/// Reads the minimum payment amount from the governance contract's system
/// parameters, falling back to [`crate::MIN_PAYMENT_AMOUNT`] (100) when the
/// parameter is unset or governance is unreachable (issue #690).
pub(crate) fn read_min_payment_amount(env: &Env) -> i128 {
    let governance: Option<Address> = env.storage().instance().get(&DataKey::Governance);
    let Some(governance) = governance else {
        return crate::MIN_PAYMENT_AMOUNT;
    };
    let mut args = Vec::<Val>::new(env);
    args.push_back(Symbol::new(env, "min_payment").into_val(env));
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

/// Invokes the governance contract's `get_fee_config` entry point and returns
/// the raw [`Val`] it produces.
///
/// Both [`read_governance_fee_rule`] and [`validate_fee_against_governance`]
/// need this call; extracting it here removes the duplication and keeps the
/// error-handling policy (`GovernanceCallFailed` on any non-`Ok(Ok(_))`
/// result) in one place.
fn invoke_governance_get_fee_config(env: &Env, governance: &Address) -> Val {
    match env.try_invoke_contract::<Val, SettlementError>(
        governance,
        &Symbol::new(env, "get_fee_config"),
        Vec::new(env),
    ) {
        Ok(Ok(val)) => val,
        _ => panic_with_error!(env, SettlementError::GovernanceCallFailed),
    }
}

/// Classifies the raw `get_fee_config` value without panicking, so the shape
/// rules can be exercised directly (see the proptest at the bottom of this
/// file). [`try_read_governance_fee_config`] is the panicking wrapper used by
/// the contract.
///
/// - `Ok(Some(config))` — a map with exactly the two required `u32` fields.
/// - `Ok(None)` — `Void` or any non-map value (governance has no config yet).
/// - `Err(GovernanceCallFailed)` — a malformed map: not exactly 2 entries, a
///   missing required key, or a field that is not `u32` (issue #483).
fn classify_governance_fee_config(
    env: &Env,
    raw_val: Val,
) -> Result<Option<GovFeeConfig>, SettlementError> {
    // A Void return means governance has no fee config set yet.
    // Map::try_from_val safely returns Err for non-map values.
    let map: Map<Symbol, Val> = match Map::try_from_val(env, &raw_val) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };

    // Issue #483: assert exactly 2 fields before reading.
    // A governance returning a single-field config (e.g. only platform_fee_bps)
    // must be rejected rather than silently skipping the network-fee ceiling.
    if map.len() != 2 {
        return Err(SettlementError::GovernanceCallFailed);
    }

    let platform_fee_bps = match map.get(Symbol::new(env, "platform_fee_bps")) {
        Some(val) => {
            u32::try_from_val(env, &val).map_err(|_| SettlementError::GovernanceCallFailed)?
        }
        None => return Err(SettlementError::GovernanceCallFailed),
    };

    let network_fee_bps = match map.get(Symbol::new(env, "network_fee_bps")) {
        Some(val) => {
            u32::try_from_val(env, &val).map_err(|_| SettlementError::GovernanceCallFailed)?
        }
        None => return Err(SettlementError::GovernanceCallFailed),
    };

    Ok(Some(GovFeeConfig {
        platform_fee_bps,
        network_fee_bps,
    }))
}

/// Validates the raw return value from governance's `get_fee_config` as a
/// properly-shaped `GovFeeConfig` (a Soroban `#[contracttype]` struct encoded
/// as a map keyed by field name), panicking on a malformed one.
///
/// Returns `Some` when the raw value is a map with both required fields
/// (`platform_fee_bps` and `network_fee_bps`), and `None` when it is `Void`
/// (governance has no config set yet).
///
/// Panics with [`SettlementError::GovernanceCallFailed`] when the governance
/// contract returned a malformed config (issue #483): a map with fewer or more
/// than 2 entries, a map missing either required key, or a field that is not
/// `u32`.
///
/// # Why the shape is validated by hand
///
/// `try_invoke_contract::<Option<GovFeeConfig>, SettlementError>` deserialises
/// the return value into `Option<GovFeeConfig>` in the **calling** contract's
/// guest code. If the governance contract returned a struct with a different
/// shape (e.g. 1 field instead of 2), the host-side `map_unpack_to_slice`
/// panics and the panic is **not** caught by `try_invoke_contract`'s
/// `Result`-based error handling — it propagates as an opaque host trap
/// ("escalating error to panic") rather than surfacing as the typed
/// `GovernanceCallFailed` error.
///
/// This avoids that path by:
/// 1. Calling `try_invoke_contract::<Val, SettlementError>` to get the raw
///    `Val` return value without triggering typed deserialisation.
/// 2. Converting the raw `Val` to `Map<Symbol, Val>` (safe: returns `Err` for
///    non-map values like `Void`).
/// 3. Validating the map structure (entry count, required keys, field types)
///    before constructing `GovFeeConfig`.
fn try_read_governance_fee_config(env: &Env, raw_val: Val) -> Option<GovFeeConfig> {
    match classify_governance_fee_config(env, raw_val) {
        Ok(config) => config,
        Err(error) => panic_with_error!(env, error),
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
///
/// # Hard-fail is intentional here (issue #742)
///
/// This is the **write** path (`set_settlement_rule` / `set_default_rule` and
/// their scheduled variants). Unlike the read path, which degrades to the
/// bootstrap default when governance is unreachable (issue #741), an admin
/// write must never silently accept a fee configuration that governance might
/// reject once it recovers. So a trap keeps raising `GovernanceCallFailed`
/// instead of returning early. The asymmetry is deliberate: reads fail open,
/// writes fail loud.
pub(crate) fn validate_fee_against_governance(env: &Env, rule: &SettlementRule) {
    let governance: Address = read_governance(env);
    let raw_val = invoke_governance_get_fee_config(env, &governance);

    let fee_config = match try_read_governance_fee_config(env, raw_val) {
        Some(cfg) => cfg,
        // Governance has no fee config set — no ceiling to enforce.
        None => return,
    };

    if rule.platform_fee_bps > fee_config.platform_fee_bps {
        panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
    }
    if rule.network_fee_bps > fee_config.network_fee_bps {
        panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
    }
}

#[cfg(test)]
mod governance_fee_config_shape_tests {
    //! Issue #740: property coverage for the shape of governance's fee config.
    //!
    //! Fixed-shape tests cannot cover the space of malformed maps (0, 1, 3 or 4
    //! entries, wrong keys, wrong field types). This fuzzes that space and
    //! asserts one rule: a map is accepted only when it has exactly the two
    //! required keys with `u32` values. Anything else is rejected with
    //! `GovernanceCallFailed`; a non-map value still means "no config".

    use super::classify_governance_fee_config;
    use crate::errors::SettlementError;
    use proptest::prelude::*;
    use soroban_sdk::{Env, IntoVal, Map, Symbol, Val};

    const FIELD_KEYS: [&str; 4] = [
        "platform_fee_bps",
        "network_fee_bps",
        "extra_one",
        "extra_two",
    ];

    fn raw_map(env: &Env, fields: &[(usize, u32)]) -> Val {
        let mut map: Map<Symbol, Val> = Map::new(env);
        for (index, value) in fields {
            let _ = map.set(Symbol::new(env, FIELD_KEYS[*index]), (*value).into_val(env));
        }
        map.into_val(env)
    }

    proptest! {
        /// Any subset of the four keys (0..=4 present), arbitrary `u32`
        /// values: only the exact two-required-key map is accepted.
        #[test]
        fn only_two_required_u32_fields_are_accepted(
            layout in (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
            platform in any::<u32>(),
            network in any::<u32>(),
        ) {
            let env = Env::default();
            let present = [layout.0, layout.1, layout.2, layout.3];
            let count = present.iter().filter(|is_present| **is_present).count();

            // Collect the present fields into a fixed buffer; no heap needed.
            let mut fields = [(0usize, 0u32); 4];
            let mut cursor = 0usize;
            for (index, is_present) in present.iter().enumerate() {
                if *is_present {
                    let value = match index {
                        0 => platform,
                        1 => network,
                        _ => 0,
                    };
                    fields[cursor] = (index, value);
                    cursor += 1;
                }
            }

            let result =
                classify_governance_fee_config(&env, raw_map(&env, &fields[..cursor]));
            let well_formed = count == 2 && present[0] && present[1];

            match result {
                Ok(Some(config)) => {
                    prop_assert!(well_formed, "layout {:?} must not be accepted", present);
                    prop_assert_eq!(config.platform_fee_bps, platform);
                    prop_assert_eq!(config.network_fee_bps, network);
                }
                Ok(None) => prop_assert!(false, "a map value is never treated as no-config"),
                Err(error) => {
                    prop_assert!(
                        !well_formed,
                        "well-formed layout {:?} must be accepted",
                        present
                    );
                    prop_assert!(matches!(error, SettlementError::GovernanceCallFailed));
                }
            }
        }
    }

    #[test]
    fn non_map_value_means_no_config() {
        let env = Env::default();
        assert!(matches!(
            classify_governance_fee_config(&env, 5u32.into_val(&env)),
            Ok(None)
        ));
    }

    #[test]
    fn non_u32_field_value_is_rejected() {
        let env = Env::default();
        let mut map: Map<Symbol, Val> = Map::new(&env);
        // Numerically in range, but the wrong Soroban type (I128, not U32).
        let _ = map.set(
            Symbol::new(&env, "platform_fee_bps"),
            250i128.into_val(&env),
        );
        let _ = map.set(Symbol::new(&env, "network_fee_bps"), 50u32.into_val(&env));
        assert!(matches!(
            classify_governance_fee_config(&env, map.into_val(&env)),
            Err(SettlementError::GovernanceCallFailed)
        ));
    }

    #[test]
    fn value_wider_than_u32_is_rejected() {
        let env = Env::default();
        let mut map: Map<Symbol, Val> = Map::new(&env);
        let _ = map.set(
            Symbol::new(&env, "platform_fee_bps"),
            (u32::MAX as u64 + 1).into_val(&env),
        );
        let _ = map.set(Symbol::new(&env, "network_fee_bps"), 50u32.into_val(&env));
        assert!(matches!(
            classify_governance_fee_config(&env, map.into_val(&env)),
            Err(SettlementError::GovernanceCallFailed)
        ));
    }
}
