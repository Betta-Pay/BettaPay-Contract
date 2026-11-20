#!/usr/bin/env bash
# BettaPay — Contract Event Exporter
#
# One-shot exporter: pulls the events emitted by a BettaPay contract over a
# caller-supplied ledger range and writes them as JSON or CSV. Useful for
# ledger audits, incident triage, and indexer debugging — replayable and
# diff-able between runs (records are sorted deterministically by ledger and
# paging token).
#
# Usage:
#   bash scripts/export_events.sh --contract-id C... [options]
#
# Options:
#   -c, --contract-id C...     Contract whose events to export (required).
#   -s, --start-ledger N       First ledger of the range
#                              (default: RPC's latest ledger minus 1000).
#   -e, --end-ledger N         Last ledger of the range (default: RPC's
#                              latest ledger).
#   -f, --format json|csv      Output format (default: json).
#   -o, --output FILE          Destination file (default: stdout).
#   -r, --rpc-url URL          Soroban RPC endpoint
#                              (default: $SOROBAN_RPC_URL).
#   -l, --limit N              Events fetched per RPC page (default: 200).
#   -E, --events-src PATH      Path to bettapay_common/src/events.rs, the
#                              source of event-name constants used to decode
#                              topic[0] (default: resolved from repo root).
#   -F, --from-file FILE       Instead of querying the RPC, parse a saved raw
#                              getEvents response (offline replay/edit).
#   -h, --help                 Show this help.
#
# Environment variables: SOROBAN_RPC_URL overrides the RPC endpoint default.
#
# JSON rows: ledger, paging_token, ledger_timestamp, contract_id, event_type,
# topic0, topic0_constant, topic1, data_base64, data_decoded.
# CSV columns: ledger, paging_token, ledger_timestamp, contract_id,
# event_type, event_name, topic0, topic1, data.
#
# Exit codes: 0 = export success (possibly zero events); any non-zero value
# indicates a usage or RPC error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
EVENTS_SRC="$ROOT_DIR/bettapay_common/src/events.rs"

CONTRACT_ID=""
START_LEDGER=""
END_LEDGER=""
FORMAT="json"
OUTPUT=""
SOROBAN_RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
PAGE_LIMIT="200"
FROM_FILE=""

usage() {
  sed -n '3,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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
    -o|--output)
      OUTPUT="$2"; shift 2 ;;
    -r|--rpc-url)
      SOROBAN_RPC_URL="$2"; shift 2 ;;
    -l|--limit)
      PAGE_LIMIT="$2"; shift 2 ;;
    -E|--events-src)
      EVENTS_SRC="$2"; shift 2 ;;
    -F|--from-file)
      FROM_FILE="$2"; shift 2 ;;
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
  json|csv) ;;
  *) log_error "Invalid --format '$FORMAT' (expected 'json' or 'csv')."; exit 1 ;;
esac

for value in "$START_LEDGER" "$END_LEDGER"; do
  if [ -n "$value" ] && [[ ! "$value" =~ ^[0-9]+$ ]]; then
    log_error "Ledger bounds must be non-negative integers (got '$value')."
    exit 1
  fi
done

assert_file_exists "$EVENTS_SRC"
assert_command "python3"

log_info "Exporting events for $CONTRACT_ID as $FORMAT ..."
log_info "RPC endpoint: $SOROBAN_RPC_URL (range: ${START_LEDGER:-latest-1000}..${END_LEDGER:-latest})"

# Build the exporter's argument vector and run the shared helper library.
ARGS=(
  export
  --contract-id "$CONTRACT_ID"
  --format "$FORMAT"
  --events-src "$EVENTS_SRC"
  --page-limit "$PAGE_LIMIT"
)
[ -n "$START_LEDGER" ] && ARGS+=(--start-ledger "$START_LEDGER")
[ -n "$END_LEDGER" ] && ARGS+=(--end-ledger "$END_LEDGER")
if [ -n "$FROM_FILE" ]; then
  assert_file_exists "$FROM_FILE"
  ARGS+=(--from-file "$FROM_FILE")
else
  ARGS+=(--rpc-url "$SOROBAN_RPC_URL")
fi

if [ -n "$OUTPUT" ]; then
  python3 "$SCRIPT_DIR/lib/soroban_events.py" "${ARGS[@]}" >"$OUTPUT"
  log_success "Exported events for $CONTRACT_ID to '$OUTPUT'."
else
  python3 "$SCRIPT_DIR/lib/soroban_events.py" "${ARGS[@]}"
  log_success "Export complete."
fi
