#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# shellcheck source=../lib/common.sh
source "${ROOT_DIR}/scripts/lib/common.sh"

fail() {
  echo "tooling smoke test failed: $1" >&2
  exit 1
}

CAPTURED_ARGS=()
soroban() {
  CAPTURED_ARGS=("$@")
}

SOROBAN_RPC_URL="https://rpc.example.invalid"
SOROBAN_NETWORK_PASSPHRASE="Test Network"
ADMIN_ADDRESS="GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"

invoke_contract_init \
  "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM" \
  "test-identity" \
  "[\"${ADMIN_ADDRESS}\"]" \
  1 \
  --recovery-address "$ADMIN_ADDRESS"

CAPTURED=" ${CAPTURED_ARGS[*]} "
[[ "$CAPTURED" == *" init --admins [\"${ADMIN_ADDRESS}\"] --threshold 1 "* ]] ||
  fail "init invocation does not pass --admins and --threshold"
[[ "$CAPTURED" != *" --admin "* ]] || fail "legacy --admin flag is still present"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT
mkdir -p "$TEST_ROOT/target/optimized" "$TEST_ROOT/target/wasm32-unknown-unknown/release"
printf 'optimized' >"$TEST_ROOT/target/optimized/contract_opt.wasm"
dd if=/dev/zero of="$TEST_ROOT/target/wasm32-unknown-unknown/release/contract.wasm" \
  bs=131073 count=1 status=none

# The oversized release artifact must not affect the deployed-artifact gate.
ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_wasm_size.sh" >/dev/null

# Conversely, an oversized optimized artifact must fail the gate.
dd if=/dev/zero of="$TEST_ROOT/target/optimized/contract_opt.wasm" \
  bs=131073 count=1 status=none
if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_wasm_size.sh" >/dev/null 2>&1; then
  fail "size gate accepted an oversized optimized artifact"
fi

echo "tooling smoke tests passed"
