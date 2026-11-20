#!/usr/bin/env bash
# BettaPay — Interface Version Parity Check
#
# Reads the advertised interface version (the SUPPORTED_INTERFACE_VERSION
# const) from settlement_contract and governance_contract and fails with a
# non-zero exit when the two disagree. Both contracts must advertise the
# same supported interface version; the `upgrade` probe gates Wasm swaps on
# it, so silent drift would let an upgrade force a mismatched Wasm onto the
# network before the probe trips.
#
# Primary mode parses the Rust source (no build artifacts required — this is
# the fallback that keeps the check independent of `make optimize`). When
# both on-chain contract IDs are supplied, the script additionally probes the
# deployed Wasm's `supports_interface` entry point so chain state is verified
# against the parsed constant too.
#
# Usage:
#   bash scripts/check_interface_parity.sh [options]
#
# Options:
#   -s, --settlement-id C...  On-chain settlement contract ID (optional).
#   -g, --governance-id C...  On-chain governance contract ID (optional).
#   -r, --rpc-url URL         Soroban RPC endpoint (default: $SOROBAN_RPC_URL).
#   -a, --source-account ID   Soroban identity used for on-chain probing
#                             (default: $SOROBAN_SOURCE, "bettapay-admin").
#   -h, --help                Show this help.
#
# Environment variables (all optional, only used for on-chain probing):
#   SOROBAN_RPC_URL, SOROBAN_NETWORK_PASSPHRASE, SOROBAN_SOURCE
#
# Exit codes: 0 = versions match, 1 = mismatch/missing/parse error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

SETTLEMENT_SRC="${SETTLEMENT_SRC:-$ROOT_DIR/settlement_contract/src}"
GOVERNANCE_SRC="${GOVERNANCE_SRC:-$ROOT_DIR/governance_contract/src}"

SETTLEMENT_ID=""
GOVERNANCE_ID=""
SOROBAN_RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
SOROBAN_SOURCE="${SOROBAN_SOURCE:-bettapay-admin}"

usage() {
  sed -n '3,31p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -s|--settlement-id)
      SETTLEMENT_ID="$2"; shift 2 ;;
    -g|--governance-id)
      GOVERNANCE_ID="$2"; shift 2 ;;
    -r|--rpc-url)
      SOROBAN_RPC_URL="$2"; shift 2 ;;
    -a|--source-account)
      SOROBAN_SOURCE="$2"; shift 2 ;;
    -h|--help)
      usage ;;
    *)
      log_error "Unknown option: $1"
      usage
      exit 1 ;;
  esac
done

# Extract the version from the source tree of one contract. The constant
# currently lives in governance_contract/src/lib.rs and
# settlement_contract/src/policy.rs (re-exported into lib.rs via
# `pub(crate) use policy::*`), so the whole src dir is scanned but test
# snapshots and non-Rust artifacts are excluded.
parse_supported_interface_version() {
  local src_dir="$1"
  local contract_name="$2"
  local declarations

  if [ ! -d "$src_dir" ]; then
    log_error "Source directory for $contract_name not found: $src_dir"
    exit 1
  fi

  declarations="$(find "$src_dir" -name '*.rs' -type f -print0 \
    | xargs -0 grep -n 'const SUPPORTED_INTERFACE_VERSION: u32 = [0-9]*;' || true)"

  if [ -z "$declarations" ]; then
    log_error "No SUPPORTED_INTERFACE_VERSION declaration found for $contract_name in $src_dir."
    exit 1
  fi

  local count
  count="$(echo "$declarations" | wc -l | tr -d ' ')"
  if [ "$count" -ne 1 ]; then
    log_error "$contract_name declares SUPPORTED_INTERFACE_VERSION $count times; expected exactly one."
    echo "$declarations" >&2
    exit 1
  fi

  local version
  version="$(echo "$declarations" | sed -n 's/.*const SUPPORTED_INTERFACE_VERSION: u32 = \([0-9]*\);.*/\1/p')"
  if [ -z "$version" ]; then
    log_error "Could not parse SUPPORTED_INTERFACE_VERSION for $contract_name."
    exit 1
  fi
  echo "$version"
}

SETTLEMENT_VERSION="$(parse_supported_interface_version "$SETTLEMENT_SRC" "settlement_contract")"
GOVERNANCE_VERSION="$(parse_supported_interface_version "$GOVERNANCE_SRC" "governance_contract")"

log_info "settlement_contract SUPPORTED_INTERFACE_VERSION: $SETTLEMENT_VERSION"
log_info "governance_contract SUPPORTED_INTERFACE_VERSION: $GOVERNANCE_VERSION"

if [ "$SETTLEMENT_VERSION" != "$GOVERNANCE_VERSION" ]; then
  log_error "Interface version mismatch: settlement_contract advertises $SETTLEMENT_VERSION but governance_contract advertises $GOVERNANCE_VERSION."
  log_error "Interface versions may only change together; keep SUPPORTED_INTERFACE_VERSION in sync across both contracts."
  exit 1
fi

if [ -n "$SETTLEMENT_ID" ] && [ -z "$GOVERNANCE_ID" ] \
   || [ -n "$GOVERNANCE_ID" ] && [ -z "$SETTLEMENT_ID" ]; then
  log_error "On-chain probing needs both --settlement-id and --governance-id (or neither, to use the source-level fallback)."
  exit 2
fi

if [ -n "$SETTLEMENT_ID" ] && [ -n "$GOVERNANCE_ID" ]; then
  assert_command soroban
  assert_contract_id "$SETTLEMENT_ID" "settlement contract ID"
  assert_contract_id "$GOVERNANCE_ID" "governance contract ID"
  log_info "Probing deployed Wasm supports_interface (RPC: $SOROBAN_RPC_URL)..."
  for pair in "$SETTLEMENT_ID|$SETTLEMENT_VERSION|settlement" \
              "$GOVERNANCE_ID|$GOVERNANCE_VERSION|governance"; do
    IFS='|' read -r id version name <<<"$pair"
    supports_reply="$(soroban contract invoke \
      --id "$id" \
      --source-account "$SOROBAN_SOURCE" \
      --rpc-url "$SOROBAN_RPC_URL" \
      --network-passphrase "${SOROBAN_NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}" \
      -- supports_interface --version "$version")"
    if [[ "$supports_reply" != *"true"* ]]; then
      log_error "On-chain $name Wasm rejected supports_interface($version): $supports_reply"
      exit 1
    fi
  done
  log_success "Deployed Wasm supports_interface matches the source constant on both contracts."
fi

log_success "Interface versions match: both contracts advertise version $SETTLEMENT_VERSION."
