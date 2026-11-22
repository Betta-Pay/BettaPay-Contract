//! Canonical error-code registry shared by every BettaPay contract.
//!
//! Soroban's `#[contracterror]` macro requires each contract to define its
//! own error enum (it becomes part of that contract's WASM interface), so
//! this module cannot export a single shared enum. Instead it fixes the
//! *numeric* meaning of every code: each contract's error enum must use
//! these constants as explicit discriminants rather than picking its own
//! numbers. This is what makes a raw error code returned from either
//! contract unambiguous to an off-chain client without it needing to know
//! which contract produced it.
//!
//! Ranges:
//! - `1..=99`   — codes shared by two or more contracts (this module).
//! - `200..=299` — reserved for `governance_contract`-only codes.
//! - `300..=399` — reserved for `settlement_contract`-only codes.
//!
//! Adding a new contract-specific error: pick the next free number in that
//! contract's range. Adding a new *shared* concept: add a constant here in
//! the `1..=99` range and update every contract that needs it. Either way,
//! [`assert_no_code_collisions`] (exercised by both contracts' test suites)
//! will fail the build if two different concepts end up sharing a number.

/// The contract has already been initialized.
pub const ALREADY_INITIALIZED: u32 = 1;
/// The contract has not been initialized yet.
pub const NOT_INITIALIZED: u32 = 2;
/// The caller is not authorized to perform this action.
pub const UNAUTHORIZED: u32 = 3;
/// The provided fee basis points are invalid or exceed the maximum limit.
pub const INVALID_FEE_BPS: u32 = 4;
/// The contract is currently paused and the operation is not allowed.
pub const PAUSED: u32 = 5;
/// The provided admin address is invalid (e.g., zero address or same as current admin).
pub const INVALID_ADMIN: u32 = 6;
/// The provided recovery address is invalid.
pub const INVALID_RECOVERY_ADDRESS: u32 = 7;
/// No admin recovery is currently pending.
pub const RECOVERY_NOT_PENDING: u32 = 8;
/// The admin-recovery timelock has not yet elapsed.
pub const RECOVERY_DELAY_ACTIVE: u32 = 9;
/// The scheduled operation is not yet ready for execution.
pub const EXECUTION_NOT_READY: u32 = 10;
/// The operation has not been scheduled.
pub const OPERATION_NOT_SCHEDULED: u32 = 11;
/// The operation has already been scheduled.
pub const OPERATION_ALREADY_SCHEDULED: u32 = 12;
/// The deployed WASM does not implement the required interface.
pub const INVALID_WASM_INTERFACE: u32 = 13;
/// The provided multisig threshold is invalid.
pub const INVALID_THRESHOLD: u32 = 14;
/// A recovery operation is already pending; a second one cannot be initiated.
pub const RECOVERY_ALREADY_PENDING: u32 = 15;
/// `unpause` was called while the contract was already unpaused.
pub const ALREADY_UNPAUSED: u32 = 16;
/// `pause` was called while the contract was already paused.
pub const ALREADY_PAUSED: u32 = 17;
/// The new admin set and threshold are identical to the current ones.
pub const SAME_ADMIN: u32 = 18;

/// Lowest code reserved for `governance_contract`-only errors.
pub const GOVERNANCE_RANGE_START: u32 = 200;
/// Lowest code reserved for `settlement_contract`-only errors.
pub const SETTLEMENT_RANGE_START: u32 = 300;

/// All codes defined in the shared `1..=99` range, paired with their name,
/// for use by cross-contract conformity tests.
pub const SHARED_CODES: &[(&str, u32)] = &[
    ("AlreadyInitialized", ALREADY_INITIALIZED),
    ("NotInitialized", NOT_INITIALIZED),
    ("Unauthorized", UNAUTHORIZED),
    ("InvalidFeeBps", INVALID_FEE_BPS),
    ("Paused", PAUSED),
    ("InvalidAdmin", INVALID_ADMIN),
    ("InvalidRecoveryAddress", INVALID_RECOVERY_ADDRESS),
    ("RecoveryNotPending", RECOVERY_NOT_PENDING),
    ("RecoveryDelayActive", RECOVERY_DELAY_ACTIVE),
    ("ExecutionNotReady", EXECUTION_NOT_READY),
    ("OperationNotScheduled", OPERATION_NOT_SCHEDULED),
    ("OperationAlreadyScheduled", OPERATION_ALREADY_SCHEDULED),
    ("InvalidWasmInterface", INVALID_WASM_INTERFACE),
    ("InvalidThreshold", INVALID_THRESHOLD),
    ("RecoveryAlreadyPending", RECOVERY_ALREADY_PENDING),
    ("AlreadyUnpaused", ALREADY_UNPAUSED),
    ("AlreadyPaused", ALREADY_PAUSED),
    ("SameAdmin", SAME_ADMIN),
];

/// Asserts that a contract's full `(name, code)` table is internally
/// consistent with the shared registry: every code it reuses from
/// [`SHARED_CODES`] must be attached to the matching name, every code
/// outside the shared `1..=99` range must fall inside `range_start..`,
/// and no code is duplicated within the table.
///
/// Intended to be called from each contract's own test suite with its full
/// error table, so a future edit that assigns a contract-specific error the
/// same number as a shared code (or vice versa) fails `cargo test --workspace`
/// instead of silently reintroducing issue #517.
pub fn assert_no_code_collisions(contract_codes: &[(&str, u32)], range_start: u32) {
    for &(name, code) in contract_codes {
        if let Some(&(shared_name, _)) = SHARED_CODES.iter().find(|&&(_, c)| c == code) {
            assert_eq!(
                name, shared_name,
                "error code {code} is reserved for shared concept `{shared_name}` \
                 but is also used here for `{name}`",
            );
        } else {
            assert!(
                code >= range_start,
                "error code {code} (`{name}`) is not a shared code and falls \
                 outside this contract's reserved range ({range_start}+)",
            );
        }
    }

    // Check for duplicate codes (collisions)
    for (i, &(name_a, code_a)) in contract_codes.iter().enumerate() {
        for &(name_b, code_b) in &contract_codes[i + 1..] {
            assert_ne!(
                code_a, code_b,
                "duplicate code {code_a} shared between `{name_a}` and `{name_b}`"
            );
        }
    }
}

/// Enumerates contract-specific codes in `contract_codes` (codes >= `range_start`)
/// and returns any gaps (missing integers) between the minimum and maximum
/// code in that range. Used by tests to audit and flag code space gaps.
pub fn find_code_gaps(contract_codes: &[(&str, u32)], range_start: u32) -> alloc::vec::Vec<u32> {
    use alloc::vec::Vec;

    let mut specific_codes: Vec<u32> = contract_codes
        .iter()
        .map(|&(_, c)| c)
        .filter(|&c| c >= range_start)
        .collect();
    specific_codes.sort_unstable();
    specific_codes.dedup();

    let mut gaps = Vec::new();
    if let (Some(&min), Some(&max)) = (specific_codes.first(), specific_codes.last()) {
        for expected in min..=max {
            if !specific_codes.contains(&expected) {
                gaps.push(expected);
            }
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;

    fn registry_entries(markdown: &str, heading: &str) -> Vec<(String, u32)> {
        let section_marker = alloc::format!("## {heading}");
        let start = markdown
            .find(&section_marker)
            .unwrap_or_else(|| panic!("ERROR_CODE_REGISTRY.md is missing `{heading}`"));
        let tail = &markdown[start + section_marker.len()..];
        let section = tail.split("\n## ").next().unwrap();
        let mut entries = Vec::new();

        for line in section.lines() {
            let columns: Vec<&str> = line
                .split('|')
                .map(str::trim)
                .filter(|column| !column.is_empty())
                .collect();
            if columns.len() < 2 || columns[0].starts_with("Code") {
                continue;
            }
            if columns[0].contains('-') || columns[1] == "—" {
                continue;
            }
            let code = columns[0]
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("invalid registry code: {}", columns[0]));
            entries.push((String::from(columns[1].trim_matches('`')), code));
        }
        entries
    }

    fn enum_entries(source: &str, enum_name: &str) -> Vec<(String, u32)> {
        let marker = alloc::format!("pub enum {enum_name} {{");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("source is missing `pub enum {enum_name}`"));
        let body = &source[start + marker.len()..];
        let mut entries = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.starts_with('}') {
                break;
            }
            let Some((name, code)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            let code = code
                .trim()
                .trim_end_matches(',')
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("invalid discriminant for `{name}`"));
            entries.push((String::from(name), code));
        }
        entries
    }

    #[test]
    fn error_code_registry_matches_error_sources() {
        let markdown = include_str!("../../docs/ERROR_CODE_REGISTRY.md");
        let settlement_source =
            include_str!("../../settlement_contract/src/errors.rs");
        let governance_source =
            include_str!("../../governance_contract/src/lib.rs");

        let mut documented_shared =
            registry_entries(markdown, "Shared Error Codes (1-99)");
        documented_shared.sort_by_key(|(_, code)| *code);
        let mut actual_shared = SHARED_CODES
            .iter()
            .map(|&(name, code)| (String::from(name), code))
            .collect::<Vec<_>>();
        actual_shared.sort_by_key(|(_, code)| *code);
        assert_eq!(documented_shared, actual_shared);

        let mut documented_governance =
            registry_entries(markdown, "Governance Contract Errors (200-299)");
        documented_governance.sort_by_key(|(_, code)| *code);
        let mut actual_governance =
            enum_entries(governance_source, "GovernanceError");
        actual_governance.retain(|(_, code)| *code >= 200);
        actual_governance.sort_by_key(|(_, code)| *code);
        assert_eq!(documented_governance, actual_governance);

        let mut documented_settlement =
            registry_entries(markdown, "Settlement Contract Errors (300-399)");
        documented_settlement.sort_by_key(|(_, code)| *code);
        let mut actual_settlement =
            enum_entries(settlement_source, "SettlementError");
        actual_settlement.retain(|(_, code)| *code >= 300);
        actual_settlement.sort_by_key(|(_, code)| *code);
        assert_eq!(documented_settlement, actual_settlement);
    }

    #[test]
    fn shared_codes_are_unique_and_within_range() {
        for (i, &(name_a, code_a)) in SHARED_CODES.iter().enumerate() {
            assert!(
                (1..=99).contains(&code_a),
                "shared error code `{name_a}` = {code_a} is outside 1..=99 range"
            );
            for &(name_b, code_b) in &SHARED_CODES[i + 1..] {
                assert_ne!(
                    code_a, code_b,
                    "duplicate code {code_a} shared between `{name_a}` and `{name_b}`"
                );
            }
        }
    }

    #[test]
    fn assert_no_code_collisions_accepts_valid_tables() {
        let valid_gov: &[(&str, u32)] = &[
            ("AlreadyInitialized", 1),
            ("SameAdmin", 18),
            ("AnchorMissing", 200),
        ];
        assert_no_code_collisions(valid_gov, GOVERNANCE_RANGE_START);

        let valid_settle: &[(&str, u32)] = &[
            ("AlreadyInitialized", 1),
            ("SameAdmin", 18),
            ("MerchantExists", 300),
        ];
        assert_no_code_collisions(valid_settle, SETTLEMENT_RANGE_START);
    }

    #[test]
    #[should_panic(expected = "reserved for shared concept `SameAdmin` but is also used here for `WrongName`")]
    fn assert_no_code_collisions_detects_mismatched_name() {
        let invalid: &[(&str, u32)] = &[("WrongName", 18)];
        assert_no_code_collisions(invalid, GOVERNANCE_RANGE_START);
    }

    #[test]
    #[should_panic(expected = "outside this contract's reserved range")]
    fn assert_no_code_collisions_detects_out_of_range_code() {
        let invalid: &[(&str, u32)] = &[("CustomCode", 150)];
        assert_no_code_collisions(invalid, GOVERNANCE_RANGE_START);
    }

    #[test]
    #[should_panic(expected = "duplicate code 200 shared between `AnchorMissing` and `DuplicateAnchor`")]
    fn assert_no_code_collisions_detects_duplicate_contract_codes() {
        let invalid: &[(&str, u32)] = &[
            ("AnchorMissing", 200),
            ("DuplicateAnchor", 200),
        ];
        assert_no_code_collisions(invalid, GOVERNANCE_RANGE_START);
    }

    #[test]
    fn find_code_gaps_identifies_missing_numbers() {
        let table: &[(&str, u32)] = &[
            ("CodeA", 300),
            ("CodeB", 302),
            ("CodeC", 303),
            ("CodeD", 306),
        ];
        let gaps = find_code_gaps(table, SETTLEMENT_RANGE_START);
        assert_eq!(gaps, alloc::vec![301, 304, 305]);

        let contiguous: &[(&str, u32)] = &[
            ("CodeA", 300),
            ("CodeB", 301),
            ("CodeC", 302),
        ];
        let gaps = find_code_gaps(contiguous, SETTLEMENT_RANGE_START);
        assert!(gaps.is_empty());
    }

    #[test]
    fn find_code_gaps_returns_empty_for_empty_input() {
        assert!(find_code_gaps(&[], SETTLEMENT_RANGE_START).is_empty());
    }

    #[test]
    fn find_code_gaps_returns_empty_for_shared_codes_only() {
        let table: &[(&str, u32)] = &[("SharedA", 1), ("SharedB", 15), ("SharedC", 99)];
        assert!(find_code_gaps(table, SETTLEMENT_RANGE_START).is_empty());
    }

    #[test]
    fn find_code_gaps_ignores_codes_below_range_start() {
        let table: &[(&str, u32)] = &[
            ("Shared", 15),
            ("BelowRange", 299),
            ("FirstContractCode", 300),
            ("NextContractCode", 302),
        ];
        assert_eq!(
            find_code_gaps(table, SETTLEMENT_RANGE_START),
            alloc::vec![301]
        );
    }

    #[test]
    fn find_code_gaps_sorts_and_deduplicates_input() {
        let table: &[(&str, u32)] = &[
            ("CodeD", 304),
            ("CodeA", 300),
            ("CodeC", 302),
            ("DuplicateA", 300),
            ("DuplicateD", 304),
        ];
        assert_eq!(
            find_code_gaps(table, SETTLEMENT_RANGE_START),
            alloc::vec![301, 303]
        );
    }
}
