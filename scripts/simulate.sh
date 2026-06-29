#!/usr/bin/env bash
# BettaPay — Stellar Local Simulation Bootstrap Script
# Run from inside BettaPay-Contract/
set -euo pipefail

# ---- Configurable defaults ------------------------------------------------
: "${SOROBAN_RPC_URL:=https://soroban-testnet.stellar.org}"
: "${SOROBAN_NETWORK_PASSPHRASE:=Test SDF Network ; September 2015}"
: "${SOROBAN_SOURCE:=bettapay-sim}"
: "${FRIENDBOT_URL:=https://friendbot.stellar.org}"

# ---- Argument parsing -----------------------------------------------------
usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run a local simulation bootstrap for BettaPay contracts on Stellar.

Options:
  -r, --rpc-url URL           Soroban RPC endpoint (default: \$SOROBAN_RPC_URL)
  -p, --network-passphrase    Network passphrase (default: \$SOROBAN_NETWORK_PASSPHRASE)
  -s, --source IDENTITY       Source identity name (default: \$SOROBAN_SOURCE)
  -f, --friendbot-url URL     Friendbot funding URL (default: \$FRIENDBOT_URL)
  -c, --config FILE           Load configuration from a file
  -h, --help                  Show this help message and exit

Environment variables SOROBAN_RPC_URL, SOROBAN_NETWORK_PASSPHRASE,
SOROBAN_SOURCE, and FRIENDBOT_URL can also be used to override defaults.
EOF
  exit 0
}

load_config() {
  local config_file="$1"
  if [ ! -f "$config_file" ]; then
    log_error "Config file '$config_file' not found."
    exit 1
  fi
  log_info "Loading configuration from '$config_file'"
  # shellcheck source=/dev/null
  source "$config_file"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -r|--rpc-url)
      SOROBAN_RPC_URL="$2"; shift 2 ;;
    -p|--network-passphrase)
      SOROBAN_NETWORK_PASSPHRASE="$2"; shift 2 ;;
    -s|--source)
      SOROBAN_SOURCE="$2"; shift 2 ;;
    -f|--friendbot-url)
      FRIENDBOT_URL="$2"; shift 2 ;;
    -c|--config)
      load_config "$2"; shift 2 ;;
    -h|--help)
      usage ;;
    *)
      log_error "Unknown option: $1"
      usage
      exit 1 ;;
  esac
done

# ANSI color codes
BOLD='\033[1m'
BLUE='\033[34m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
NC='\033[0m' # No Color

# Helper logging functions
log_info() {
  echo -e "${BLUE}${BOLD}[INFO]${NC} $1"
}

log_success() {
  echo -e "${GREEN}${BOLD}[SUCCESS]${NC} $1"
}

log_warn() {
  echo -e "${YELLOW}${BOLD}[WARNING]${NC} $1"
}

log_error() {
  echo -e "${RED}${BOLD}[ERROR]${NC} $1" >&2
}

# Helper assertion functions
assert_command() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    log_error "Required command '$cmd' is not installed or not in PATH."
    exit 1
  fi
}

assert_file_exists() {
  local file="$1"
  if [ ! -f "$file" ]; then
    log_error "Required file '$file' not found."
    exit 1
  fi
}

assert_non_empty() {
  local val="$1"
  local name="$2"
  if [ -z "$val" ]; then
    log_error "Assertion failed: '$name' is empty."
    exit 1
  fi
}

assert_stellar_address() {
  local addr="$1"
  local name="$2"
  assert_non_empty "$addr" "$name"
  if [[ ! "$addr" =~ ^G[A-Z2-7]{55}$ ]]; then
    log_error "Assertion failed: '$name' ('$addr') is not a valid Stellar address."
    exit 1
  fi
}

assert_contract_id() {
  local id="$1"
  local name="$2"
  assert_non_empty "$id" "$name"
  if [[ ! "$id" =~ ^C[A-Z2-7]{55}$ ]]; then
    log_error "Assertion failed: '$name' ('$id') is not a valid Soroban contract address."
    exit 1
  fi
}

# Ensure Soroban CLI is available
assert_command soroban

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

log_info "Initializing simulation with RPC URL: $SOROBAN_RPC_URL"
log_info "Source identity: $SOROBAN_SOURCE"

# Check and generate keys
if ! soroban keys address "$SOROBAN_SOURCE" >/dev/null 2>&1; then
  log_info "Identity '$SOROBAN_SOURCE' not found. Generating new keys and funding..."
  soroban keys generate "$SOROBAN_SOURCE" --fund >/dev/null
  log_success "Identity keys generated successfully."
else
  log_info "Using existing identity '$SOROBAN_SOURCE'."
fi

SOROBAN_SOURCE_ADDRESS="$(soroban keys address "$SOROBAN_SOURCE")"
assert_stellar_address "$SOROBAN_SOURCE_ADDRESS" "Source Address"
log_info "Source address: $SOROBAN_SOURCE_ADDRESS"

# Fund account via Friendbot
log_info "Checking friendbot funding status..."
curl --silent --fail --show-error "${FRIENDBOT_URL}?addr=${SOROBAN_SOURCE_ADDRESS}" >/dev/null || log_warn "Friendbot funding request skipped or failed (account may already be funded)."

# Build contracts
log_info "Building settlement and governance contracts..."
cargo build --target wasm32-unknown-unknown --release \
  -p settlement_contract \
  -p governance_contract
log_success "Build completed successfully."

SETTLEMENT_WASM="$ROOT_DIR/target/wasm32-unknown-unknown/release/settlement_contract.wasm"
GOVERNANCE_WASM="$ROOT_DIR/target/wasm32-unknown-unknown/release/governance_contract.wasm"

assert_file_exists "$SETTLEMENT_WASM"
assert_file_exists "$GOVERNANCE_WASM"

mkdir -p "$ROOT_DIR/.soroban"

# Deploy settlement contract
log_info "Deploying Settlement contract..."
soroban contract deploy \
  --wasm "$SETTLEMENT_WASM" \
  --source-account "$SOROBAN_SOURCE" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  >"$ROOT_DIR/.soroban/bettapay_settlement_id.txt"

SETTLEMENT_ID="$(tr -d '\n' <"$ROOT_DIR/.soroban/bettapay_settlement_id.txt")"
assert_contract_id "$SETTLEMENT_ID" "Settlement Contract ID"
log_success "Settlement contract deployed: $SETTLEMENT_ID"

# Deploy governance contract
log_info "Deploying Governance contract..."
soroban contract deploy \
  --wasm "$GOVERNANCE_WASM" \
  --source-account "$SOROBAN_SOURCE" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  >"$ROOT_DIR/.soroban/bettapay_governance_id.txt"

GOVERNANCE_ID="$(tr -d '\n' <"$ROOT_DIR/.soroban/bettapay_governance_id.txt")"
assert_contract_id "$GOVERNANCE_ID" "Governance Contract ID"
log_success "Governance contract deployed: $GOVERNANCE_ID"

# Initialize settlement contract
log_info "Initializing Settlement contract with admin..."
soroban contract invoke \
  --id "$SETTLEMENT_ID" \
  --source-account "$SOROBAN_SOURCE" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  -- \
  init --admin "$SOROBAN_SOURCE_ADDRESS"
log_success "Settlement contract initialized."

# Initialize governance contract
log_info "Initializing Governance contract with admin..."
soroban contract invoke \
  --id "$GOVERNANCE_ID" \
  --source-account "$SOROBAN_SOURCE" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  -- \
  init --admin "$SOROBAN_SOURCE_ADDRESS"
log_success "Governance contract initialized."

# Print summary
echo -e "\n========================================================================"
echo -e "  ${GREEN}${BOLD}Simulation Bootstrap Complete${NC}"
echo -e "========================================================================"
echo -e "  Source Identity:      ${BOLD}$SOROBAN_SOURCE${NC}"
echo -e "  Source address:       ${BOLD}$SOROBAN_SOURCE_ADDRESS${NC}"
echo -e "  Settlement contract:  ${GREEN}${BOLD}$SETTLEMENT_ID${NC}"
echo -e "  Governance contract:  ${GREEN}${BOLD}$GOVERNANCE_ID${NC}"
echo -e "========================================================================\n"
