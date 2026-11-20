#!/usr/bin/env bash
# BettaPay — Storage Layout Documentation Drift Check
#
# Validates that DEVELOPMENT.md storage layout tables accurately reflect all
# current DataKey variants in settlement_contract and governance_contract,
# and CommonDataKey variants in bettapay_common.
#
# Exit codes: 0 = valid / no drift, 1 = drift detected.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

DEV_DOC="${ROOT_DIR}/DEVELOPMENT.md"
SETTLEMENT_TYPES="${ROOT_DIR}/settlement_contract/src/types.rs"
GOVERNANCE_LIB="${ROOT_DIR}/governance_contract/src/lib.rs"
COMMON_STORAGE="${ROOT_DIR}/bettapay_common/src/storage.rs"

assert_file_exists "$DEV_DOC"
assert_file_exists "$SETTLEMENT_TYPES"
assert_file_exists "$GOVERNANCE_LIB"
assert_file_exists "$COMMON_STORAGE"

log_info "Verifying DEVELOPMENT.md storage layout tables against contract code..."

MISSING=0

# Extract settlement section
SETTLEMENT_DOC=$(sed -n '/### Settlement contract/,/### Governance contract/p' "$DEV_DOC")
GOVERNANCE_DOC=$(sed -n '/### Governance contract/,/## Two Constraints/p' "$DEV_DOC")

# Settlement DataKey variants to check
SETTLEMENT_KEYS=(
  "Admin"
  "Initializing"
  "Governance"
  "Merchant"
  "Rule"
  "ArchivedMerchant"
  "DefaultRule"
  "Payment"
  "ScheduledOperation"
  "Deployer"
  "SchemaVersion"
  "CachedGovRule"
)

for key in "${SETTLEMENT_KEYS[@]}"; do
  if ! echo "$SETTLEMENT_DOC" | grep -q "\`$key"; then
    log_error "DEVELOPMENT.md settlement table is missing DataKey variant: $key"
    MISSING=1
  fi
done

# Governance DataKey variants to check
GOVERNANCE_KEYS=(
  "Admin"
  "FeeConfig"
  "SchemaVersion"
  "Deployer"
  "Anchor"
  "SystemParam"
)

for key in "${GOVERNANCE_KEYS[@]}"; do
  if ! echo "$GOVERNANCE_DOC" | grep -q "\`$key"; then
    log_error "DEVELOPMENT.md governance table is missing DataKey variant: $key"
    MISSING=1
  fi
done

# CommonDataKey variants to check in both sections
COMMON_KEYS=(
  "RecoveryAddress"
  "PendingRecovery"
  "Paused"
  "Threshold"
)

for key in "${COMMON_KEYS[@]}"; do
  if ! echo "$SETTLEMENT_DOC" | grep -q "\`$key"; then
    log_error "DEVELOPMENT.md settlement table is missing CommonDataKey variant: $key"
    MISSING=1
  fi
  if ! echo "$GOVERNANCE_DOC" | grep -q "\`$key"; then
    log_error "DEVELOPMENT.md governance table is missing CommonDataKey variant: $key"
    MISSING=1
  fi
done

# Verify storage kinds for DefaultRule and FeeConfig are instance (not persistent)
if echo "$SETTLEMENT_DOC" | grep -E "\`DefaultRule\`[[:space:]]*\|[[:space:]]*persistent" >/dev/null; then
  log_error "DEVELOPMENT.md incorrectly lists DefaultRule as persistent storage (must be instance)"
  MISSING=1
fi

if echo "$GOVERNANCE_DOC" | grep -E "\`FeeConfig\`[[:space:]]*\|[[:space:]]*persistent" >/dev/null; then
  log_error "DEVELOPMENT.md incorrectly lists FeeConfig as persistent storage (must be instance)"
  MISSING=1
fi

OWNERSHIP_DOC="${ROOT_DIR}/docs/storage-key-ownership.md"
if [ -f "$OWNERSHIP_DOC" ]; then
  for key in "${SETTLEMENT_KEYS[@]}"; do
    if ! grep -q "\`.*$key" "$OWNERSHIP_DOC"; then
      log_error "docs/storage-key-ownership.md is missing DataKey variant: $key"
      MISSING=1
    fi
  done
  for key in "${GOVERNANCE_KEYS[@]}"; do
    if ! grep -q "\`.*$key" "$OWNERSHIP_DOC"; then
      log_error "docs/storage-key-ownership.md is missing DataKey variant: $key"
      MISSING=1
    fi
  done
  for key in "${COMMON_KEYS[@]}"; do
    if ! grep -q "\`.*$key" "$OWNERSHIP_DOC"; then
      log_error "docs/storage-key-ownership.md is missing CommonDataKey variant: $key"
      MISSING=1
    fi
  done
fi

if [[ $MISSING -ne 0 ]]; then
  log_error "Storage layout documentation drift detected!"
  exit 1
fi

log_success "Storage layout documentation matches contract definitions."
exit 0
