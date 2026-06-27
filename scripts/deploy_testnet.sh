#!/usr/bin/env bash
# BettaPay — Stellar Testnet Deployment Script
# Run from inside BettaPay-Contract/
set -euo pipefail

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

: "${SOROBAN_RPC_URL:=https://soroban-testnet.stellar.org}"
: "${SOROBAN_NETWORK_PASSPHRASE:=Test SDF Network ; September 2015}"
: "${BETTAPAY_IDENTITY:=bettapay-admin}"

log_info "Initializing deployment with RPC URL: $SOROBAN_RPC_URL"
log_info "Target identity: $BETTAPAY_IDENTITY"

# Check and generate keys
if ! soroban keys address "$BETTAPAY_IDENTITY" >/dev/null 2>&1; then
  log_info "Identity '$BETTAPAY_IDENTITY' not found. Generating new keys and funding..."
  soroban keys generate "$BETTAPAY_IDENTITY" --fund >/dev/null
  log_success "Identity keys generated successfully."
else
  log_info "Using existing identity '$BETTAPAY_IDENTITY'."
fi

ADMIN_ADDRESS="$(soroban keys address "$BETTAPAY_IDENTITY")"
assert_stellar_address "$ADMIN_ADDRESS" "Admin Address"
log_info "Admin address: $ADMIN_ADDRESS"

# Fund account via Friendbot
log_info "Checking friendbot funding status..."
curl --silent --fail --show-error "https://friendbot.stellar.org/?addr=${ADMIN_ADDRESS}" >/dev/null || log_warn "Friendbot funding request skipped or failed (account may already be funded)."

# Build contracts
log_info "Building settlement and governance contracts..."
cargo build --target wasm32-unknown-unknown --release \
  -p settlement_contract \
  -p governance_contract
log_success "Build completed successfully."

SETTLEMENT_WASM="${ROOT_DIR}/target/optimized/settlement_contract_opt.wasm"
GOVERNANCE_WASM="${ROOT_DIR}/target/optimized/governance_contract_opt.wasm"

assert_file_exists "$SETTLEMENT_WASM"
assert_file_exists "$GOVERNANCE_WASM"

# Deploy settlement contract
log_info "Deploying Settlement contract..."
SETTLEMENT_ID="$(
  soroban contract deploy \
    --wasm "$SETTLEMENT_WASM" \
    --source-account "$BETTAPAY_IDENTITY" \
    --rpc-url "$SOROBAN_RPC_URL" \
    --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE"
)"
assert_contract_id "$SETTLEMENT_ID" "Settlement Contract ID"
log_success "Settlement contract deployed: $SETTLEMENT_ID"

# Deploy governance contract
log_info "Deploying Governance contract..."
GOVERNANCE_ID="$(
  soroban contract deploy \
    --wasm "$GOVERNANCE_WASM" \
    --source-account "$BETTAPAY_IDENTITY" \
    --rpc-url "$SOROBAN_RPC_URL" \
    --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE"
)"
assert_contract_id "$GOVERNANCE_ID" "Governance Contract ID"
log_success "Governance contract deployed: $GOVERNANCE_ID"

# Initialize settlement contract
log_info "Initializing Settlement contract with admin..."
soroban contract invoke \
  --id "$SETTLEMENT_ID" \
  --source-account "$BETTAPAY_IDENTITY" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  -- \
  init --admin "$ADMIN_ADDRESS"
log_success "Settlement contract initialized."

# Initialize governance contract
log_info "Initializing Governance contract with admin..."
soroban contract invoke \
  --id "$GOVERNANCE_ID" \
  --source-account "$BETTAPAY_IDENTITY" \
  --rpc-url "$SOROBAN_RPC_URL" \
  --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
  -- \
  init --admin "$ADMIN_ADDRESS"
log_success "Governance contract initialized."

# Print summary
echo -e "\n========================================================================"
echo -e "  ${GREEN}${BOLD}BettaPay Testnet Deployment Complete${NC}"
echo -e "========================================================================"
echo -e "  Identity:             ${BOLD}$BETTAPAY_IDENTITY${NC}"
echo -e "  Admin address:        ${BOLD}$ADMIN_ADDRESS${NC}"
echo -e "  Settlement contract:  ${GREEN}${BOLD}$SETTLEMENT_ID${NC}"
echo -e "  Governance contract:  ${GREEN}${BOLD}$GOVERNANCE_ID${NC}"
echo -e "========================================================================"
echo -e "\n${YELLOW}${BOLD}Next Steps:${NC}"
echo -e "  Update SETTLEMENT_CONTRACT_ID and GOVERNANCE_CONTRACT_ID"
echo -e "  in BettaPay-Frontend and BettaPay-Backend .env files."
echo -e "========================================================================\n"
