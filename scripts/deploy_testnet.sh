#!/usr/bin/env bash
# BettaPay — Stellar Testnet Deployment Script
# Run from inside BettaPay-Contract/
set -euo pipefail

# Load shared helpers
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

# Ensure Soroban CLI is available
assert_command soroban

ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
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

: "${RECOVERY_ADDRESS:=$ADMIN_ADDRESS}"
assert_stellar_address "$RECOVERY_ADDRESS" "Recovery Address"
log_info "Recovery address: $RECOVERY_ADDRESS"

# Fund account via Friendbot
log_info "Checking friendbot funding status..."
curl --silent --fail --show-error "https://friendbot.stellar.org/?addr=${ADMIN_ADDRESS}" >/dev/null || log_warn "Friendbot funding request skipped or failed (account may already be funded)."

# Build contracts
log_info "Building and optimizing settlement and governance contracts..."
make optimize
log_success "Optimized build completed successfully."
log_info "Building settlement and governance contracts..."
make optimize
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
log_info "Initializing Settlement contract with admin set..."
ADMINS="[\"${ADMIN_ADDRESS}\"]"
invoke_contract_init "$SETTLEMENT_ID" "$BETTAPAY_IDENTITY" "$ADMINS" 1 \
  --governance "$GOVERNANCE_ID" --recovery-address "$RECOVERY_ADDRESS"
log_success "Settlement contract initialized."

# Initialize governance contract
log_info "Initializing Governance contract with admin set..."
invoke_contract_init "$GOVERNANCE_ID" "$BETTAPAY_IDENTITY" "$ADMINS" 1 \
  --recovery-address "$RECOVERY_ADDRESS"
log_success "Governance contract initialized."

# Print summary
echo -e "\n========================================================================"
echo -e "  ${GREEN}${BOLD}BettaPay Testnet Deployment Complete${NC}"
echo -e "========================================================================"
echo -e "  Identity:             ${BOLD}$BETTAPAY_IDENTITY${NC}"
echo -e "  Admin address:        ${BOLD}$ADMIN_ADDRESS${NC}"
echo -e "  Recovery address:     ${BOLD}$RECOVERY_ADDRESS${NC}"
echo -e "  Settlement contract:  ${GREEN}${BOLD}$SETTLEMENT_ID${NC}"
echo -e "  Governance contract:  ${GREEN}${BOLD}$GOVERNANCE_ID${NC}"
echo -e "========================================================================"
echo -e "\n${YELLOW}${BOLD}Next Steps:${NC}"
echo -e "  After deployment, update the contract IDs in:"
echo -e "  - BettaPay-Frontend (https://github.com/org/BettaPay-Frontend) .env"
echo -e "  - BettaPay-Backend (https://github.com/org/BettaPay-Backend) .env"
echo -e "========================================================================\n"
