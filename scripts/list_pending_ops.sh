#!/usr/bin/env bash
# BettaPay — Pending Scheduled Operations Lister
#
# Lists timelocked operations waiting to execute on a settlement contract so
# operators can monitor the pending queue without decoding events by hand:
# contract state is reconstructed from the `op_scheduled` / `op_executed` /
# `op_cancelled` events (no on-chain enumeration API is added; see issue
# #948). Each pending operation is shown with its hash and its execute_at
# time in readable form.
#
# Usage:
#   bash scripts/list_pending_ops.sh --contract-id C... [options]
#
# Options:
#   -c, --contract-id C...     Settlement contract to inspect (required).
#   -s, --start-ledger N       First ledger scanned for op_* events
#                              (default: RPC's latest ledger minus 150000,
#                              which comfortably covers the seven-day
#                              minimum timelock at ~5s per ledger).
#   -e, --end-ledger N         Last ledger scanned (default: RPC's latest).
#   -f, --format table|json    Output format (default: table).
#   -r, --rpc-url URL          Soroban RPC endpoint
#                              (default: $SOROBAN_RPC_URL).
#   -l, --limit N              Events fetched per RPC page (default: 200).
#   -F, --from-file FILE       Instead of querying the RPC, reconstruct the
#                              pending set from a saved raw getEvents
#                              response (offline replay).
#   -n, --now EPOCH            Evaluate 'due in' as of this unix timestamp
#                              (default: current time).
#   -h, --help                 Show this help.
#
# Environment variables: SOROBAN_RPC_URL overrides the RPC endpoint default.
# No keys are required — the script is read-only and submits no transactions.
#
# Exit codes: 0 = listing produced (a queue may legitimately be empty);
# any non-zero value indicates a usage or RPC error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

CONTRACT_ID=""
START_LEDGER=""
END_LEDGER=""
FORMAT="table"
SOROBAN_RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
PAGE_LIMIT="200"
FROM_FILE=""
NOW_EPOCH=""

usage() {
  sed -n '3,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -c|--contract-id)
      CONTRACT_ID="$2"; shift 2 ;;
    -s|--start-ledger)
      START_LEDGER="$2"; shift 2 ;;
    -e|--end-ledger)
      END_LEDGER="$2"; shift 2 ;;
    -f|--format)
      FORMAT="$2"; shift 2 ;;
    -r|--rpc-url)
      SOROBAN_RPC_URL="$2"; shift 2 ;;
    -l|--limit)
      PAGE_LIMIT="$2"; shift 2 ;;
    -F|--from-file)
      FROM_FILE="$2"; shift 2 ;;
    -n|--now)
      NOW_EPOCH="$2"; shift 2 ;;
    -h|--help)
      usage ;;
    *)
      log_error "Unknown option: $1"
      usage
      exit 1 ;;
  esac
done

[ -n "$CONTRACT_ID" ] || { log_error "--contract-id is required."; usage; }
assert_contract_id "$CONTRACT_ID" "contract ID"

case "$FORMAT" in
  table|json) ;;
  *) log_error "Invalid --format '$FORMAT' (expected 'table' or 'json')."; exit 1 ;;
esac

for value in "$START_LEDGER" "$END_LEDGER" "$NOW_EPOCH"; do
  if [ -n "$value" ] && [[ ! "$value" =~ ^[0-9]+$ ]]; then
    log_error "Ledger bounds and --now must be non-negative integers (got '$value')."
    exit 1
  fi
done

assert_command "python3"

log_info "Listing pending timelocked operations for $CONTRACT_ID ..."
log_info "RPC endpoint: $SOROBAN_RPC_URL (range: ${START_LEDGER:-latest-150000}..${END_LEDGER:-latest})"

ARGS=(
  pending
  --contract-id "$CONTRACT_ID"
  --format "$FORMAT"
  --page-limit "$PAGE_LIMIT"
)
[ -n "$START_LEDGER" ] && ARGS+=(--start-ledger "$START_LEDGER")
[ -n "$END_LEDGER" ] && ARGS+=(--end-ledger "$END_LEDGER")
[ -n "$NOW_EPOCH" ] && ARGS+=(--now "$NOW_EPOCH")
if [ -n "$FROM_FILE" ]; then
  assert_file_exists "$FROM_FILE"
  ARGS+=(--from-file "$FROM_FILE")
else
  ARGS+=(--rpc-url "$SOROBAN_RPC_URL")
fi

python3 "$SCRIPT_DIR/lib/soroban_events.py" "${ARGS[@]}"
