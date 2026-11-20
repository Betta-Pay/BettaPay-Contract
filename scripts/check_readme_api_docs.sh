#!/usr/bin/env bash
# Check the README API tables for signatures and errors known to be stale.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
README_DOC="${README_DOC:-${ROOT_DIR}/README.md}"

if [[ ! -f "$README_DOC" ]]; then
  echo "README API docs check failed: missing $README_DOC" >&2
  exit 1
fi

API_TABLES="$(sed -n '/#### Settlement API Table/,/^---$/p; /#### Governance API Table/,/^---$/p' "$README_DOC")"
GOVERNANCE_TABLE="$(sed -n '/#### Governance API Table/,/^---$/p' "$README_DOC")"

if grep -E 'InvalidAddress|InvalidAmount|`init`[[:space:]]*\|[[:space:]]*`admin: Address`|`transfer_admin`[[:space:]]*\|[[:space:]]*`(_caller: Address|new_admin: Address)' <<<"$API_TABLES" >/dev/null; then
  echo "README API docs check failed: stale API signature or error name found" >&2
  exit 1
fi

if grep -Fq '`ZeroAddress`' <<<"$GOVERNANCE_TABLE"; then
  echo "README API docs check failed: GovernanceError has no ZeroAddress variant" >&2
  exit 1
fi

required_rows=(
  '| `init` | `deployer: Address`, `admins: Vec<Address>`, `threshold: u32`, `governance: Address`, `recovery_address: Address`'
  '| `update_governance` | `signers: Vec<Address>`, `new_governance: Address`'
  '| `transfer_admin` | `signers: Vec<Address>`, `new_admins: Vec<Address>`, `new_threshold: u32`'
  '| `change_threshold` | `signers: Vec<Address>`, `new_threshold: u32`'
  '| `pause` | `signers: Vec<Address>`, `reason: u32`'
  '| `schedule` | `signers: Vec<Address>`, `operation: Operation`, `execute_in: u64` | `()` | Stored Admin multisig threshold | `Paused`, `Unauthorized`, `ExecutionNotReady`, `ExecutionDelayTooLong`, `PayloadTooLarge`, `InvalidThreshold`, `InvalidSettlementDelay`'
  '| `get_payments` | `merchant: Address`, `refs: Vec<BytesN<32>>`, `cursor: u32`, `signers: Vec<Address>`'
  '| `init` | `deployer: Address`, `admins: Vec<Address>`, `threshold: u32`, `recovery_address: Address`'
  '| `upsert_anchor` | `signers: Vec<Address>`, `asset: Address`, `anchor: Address` | `()` | Stored Admin multisig, not paused | `NotInitialized`, `Paused`, `Unauthorized`, `InvalidAdmin`'
)

for row in "${required_rows[@]}"; do
  if ! grep -Fq "$row" <<<"$API_TABLES"; then
    echo "README API docs check failed: missing current API row: $row" >&2
    exit 1
  fi
done

echo "README API docs match the current public signatures and stale-token check."
