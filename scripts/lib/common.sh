#!/usr/bin/env bash
# BettaPay — Shared helper library for shell scripts
# Source this file from other scripts in the `scripts/` directory.

# ---- ANSI color codes ---------------------------------------------------
BOLD='\033[1m'
BLUE='\033[34m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
NC='\033[0m' # No Color

# ---- Logging helpers ----------------------------------------------------
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

# ---- Assertion helpers --------------------------------------------------
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

# Invoke a contract's multisig-aware init entry point. Keeping the shared flags
# here prevents deployment scripts from drifting from the contract interface.
invoke_contract_init() {
  local contract_id="$1"
  local source_account="$2"
  local admins="$3"
  local threshold="$4"
  shift 4

  soroban contract invoke \
    --id "$contract_id" \
    --source-account "$source_account" \
    --rpc-url "$SOROBAN_RPC_URL" \
    --network-passphrase "$SOROBAN_NETWORK_PASSPHRASE" \
    -- \
    init --admins "$admins" --threshold "$threshold" "$@"
}
