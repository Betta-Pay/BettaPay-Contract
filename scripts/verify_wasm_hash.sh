#!/usr/bin/env bash
# BettaPay — Deployed Wasm Hash Verifier
#
# Recomputes the sha256 of the local optimized Wasm and compares it to the
# Wasm hash actually deployed for a contract ID, confirming that what runs
# on-chain is exactly the artifact built from this checkout. Run it after
# every deploy or upgrade to detect source/chain drift before it reaches the
# upgrade probe.
#
# Usage:
#   bash scripts/verify_wasm_hash.sh --contract-id C... [--wasm FILE]
#
# Options:
#   -c, --contract-id C...       On-chain contract ID to verify (required).
#   -w, --wasm FILE              Local optimized Wasm file
#                                (optional; resolved from .soroban/ IDs when
#                                omitted — defaults to
#                                target/optimized/<contract>_opt.wasm).
#   -r, --rpc-url URL            Soroban RPC endpoint
#                                (default: $SOROBAN_RPC_URL).
#   -p, --network-passphrase ... Network passphrase
#                                (default: $SOROBAN_NETWORK_PASSPHRASE).
#   -h, --help                   Show this help.
#
# Environment variables: SOROBAN_RPC_URL, SOROBAN_NETWORK_PASSPHRASE may be
# used to override the RPC defaults.
#
# Exit codes: 0 = hashes match, 1 = hashes differ, 2 = usage/lookup error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$ROOT_DIR"

CONTRACT_ID=""
WASM_PATH=""
SOROBAN_RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
SOROBAN_NETWORK_PASSPHRASE="${SOROBAN_NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}"

usage() {
  sed -n '3,27p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -c|--contract-id)
      CONTRACT_ID="$2"; shift 2 ;;
    -w|--wasm)
      WASM_PATH="$2"; shift 2 ;;
    -r|--rpc-url)
      SOROBAN_RPC_URL="$2"; shift 2 ;;
    -p|--network-passphrase)
      SOROBAN_NETWORK_PASSPHRASE="$2"; shift 2 ;;
    -h|--help)
      usage ;;
    *)
      log_error "Unknown option: $1"
      usage
      exit 2 ;;
  esac
done

[ -n "$CONTRACT_ID" ] || { log_error "--contract-id is required."; usage; }
assert_contract_id "$CONTRACT_ID" "contract ID"

# ---- Resolve the local optimized Wasm ---------------------------------------

# scripts/simulate.sh persists the IDs it deployed under .soroban/; use them
# to map a contract ID onto its artifact when --wasm is not given.
match_saved_id() {
  local id_file="$1"
  if [ -f "$ROOT_DIR/.soroban/$id_file" ] \
     && [ "$(tr -d '\n' <"$ROOT_DIR/.soroban/$id_file")" = "$CONTRACT_ID" ]; then
    return 0
  fi
  return 1
}

if [ -z "$WASM_PATH" ]; then
  if match_saved_id "bettapay_settlement_id.txt"; then
    WASM_PATH="$ROOT_DIR/target/optimized/settlement_contract_opt.wasm"
  elif match_saved_id "bettapay_governance_id.txt"; then
    WASM_PATH="$ROOT_DIR/target/optimized/governance_contract_opt.wasm"
  else
    log_error "Could not map contract $CONTRACT_ID to a local Wasm."
    log_error "Pass the artifact explicitly: --wasm target/optimized/<contract>_opt.wasm"
    exit 2
  fi
fi

assert_file_exists "$WASM_PATH"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

LOCAL_HASH="$(sha256_of "$WASM_PATH")"
log_info "Local Wasm: $WASM_PATH"
log_info "Local sha256: $LOCAL_HASH"

# ---- Query the on-chain Wasm hash through the soroban CLI --------------------

assert_command soroban
log_info "Inspecting on-chain contract $CONTRACT_ID..."

INSPECT_OUTPUT="$(soroban contract inspect \
  --id "$CONTRACT_ID" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" 2>&1)" || {
  log_error "soroban contract inspect failed:"
  log_error "$INSPECT_OUTPUT"
  exit 2
}

# The inspect output (JSON or text) renders hashes as 64 hex characters,
# sometimes dash-separated. Strip separators and isolate the artifact hash.
# `|| true` keeps pipefail from aborting when no candidate matches, because
# that case is handled below with a clear message.
ON_CHAIN_HASH="$(echo "$INSPECT_OUTPUT" \
  | grep -oE '[0-9a-f-]{63,}' \
  | tr -d '-' \
  | grep -E '^[0-9a-f]{64}$' \
  | head -n 1 || true)"

if [ -z "$ON_CHAIN_HASH" ]; then
  log_error "Could not extract a Wasm hash from the inspect output:"
  log_error "$INSPECT_OUTPUT"
  exit 2
fi

log_info "On-chain wasm hash: $ON_CHAIN_HASH"

if [ "$LOCAL_HASH" != "$ON_CHAIN_HASH" ]; then
  log_error "Wasm hash MISMATCH: on-chain contract $CONTRACT_ID runs $ON_CHAIN_HASH but the local artifact is $LOCAL_HASH."
  log_error "Re-run the deploy/upgrade with a rebuilt 'make optimize' artifact, or point --wasm at the artifact actually deployed."
  exit 1
fi

log_success "Verified: on-chain Wasm of $CONTRACT_ID matches $WASM_PATH ($LOCAL_HASH)."
