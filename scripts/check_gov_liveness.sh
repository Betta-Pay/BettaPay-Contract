#!/usr/bin/env bash
# BettaPay — Governance Liveness Probe Script
#
# Probes the governance contract's `get_fee_config` entry point to verify
# that the contract is live, responsive, and returning valid configuration.
# Alerts on invocation trap, execution failure, or malformed return.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=lib/common.sh
if [ -f "${SCRIPT_DIR}/lib/common.sh" ]; then
  source "${SCRIPT_DIR}/lib/common.sh"
else
  # Minimal fallback logging if common.sh is unavailable
  log_info() { echo "[INFO] $1"; }
  log_success() { echo "[SUCCESS] $1"; }
  log_warn() { echo "[WARNING] $1"; }
  log_error() { echo "[ERROR] $1" >&2; }
fi

alert() {
  local message="$1"
  log_error "LIVENESS ALERT: ${message}"
  if [ -n "${ALERT_WEBHOOK_URL:-}" ]; then
    if command -v curl >/dev/null 2>&1; then
      curl -s -X POST -H "Content-Type: application/json" \
        -d "{\"alert\":\"governance_liveness_failure\",\"message\":\"${message}\"}" \
        "${ALERT_WEBHOOK_URL}" >/dev/null 2>&1 || true
    fi
  fi
  exit 1
}

# Resolve target governance contract ID
GOV_ID="${1:-${GOV_CONTRACT_ID:-${GOV:-}}}"
RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
NETWORK_PASSPHRASE="${SOROBAN_NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}"

if [ -z "$GOV_ID" ]; then
  log_error "Usage: $0 <GOV_CONTRACT_ID> or set GOV_CONTRACT_ID / GOV environment variable."
  exit 2
fi

if ! command -v soroban >/dev/null 2>&1; then
  alert "soroban CLI is not installed or not in PATH."
fi

log_info "Probing governance liveness on contract: ${GOV_ID}"

INVOKE_ARGS=(
  contract invoke
  --id "${GOV_ID}"
  --rpc-url "${RPC_URL}"
  --network-passphrase "${NETWORK_PASSPHRASE}"
)

if [ -n "${SOROBAN_SOURCE_ACCOUNT:-}" ]; then
  INVOKE_ARGS+=(--source-account "${SOROBAN_SOURCE_ACCOUNT}")
fi

INVOKE_ARGS+=(-- get_fee_config)

# Execute probe invocation
set +e
PROBE_OUTPUT="$(soroban "${INVOKE_ARGS[@]}" 2>&1)"
EXIT_CODE=$?
set -e

if [ $EXIT_CODE -ne 0 ]; then
  alert "Governance contract trapped or invocation failed (exit code ${EXIT_CODE}): ${PROBE_OUTPUT}"
fi

# Verify return data is non-empty and well-formed
TRIMMED_OUTPUT="$(echo "${PROBE_OUTPUT}" | tr -d '[:space:]')"
if [ -z "${TRIMMED_OUTPUT}" ]; then
  alert "Governance contract returned empty response for get_fee_config"
fi

# The contract returns Option<FeeConfig>: either "null" (None) or a map/struct with fee parameters.
# Any panic or unexpected error message indicates a malformed or trapping state.
if [[ "${TRIMMED_OUTPUT}" == *"Error("* ]] || [[ "${TRIMMED_OUTPUT}" == *"HostError"* ]] || [[ "${TRIMMED_OUTPUT}" == *"panic"* ]]; then
  alert "Governance contract returned an error response: ${PROBE_OUTPUT}"
fi

log_success "Governance contract liveness probe succeeded: ${PROBE_OUTPUT}"
exit 0
