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

# The soroban-cli version check must accept the pinned version and name both
# versions on a mismatch.
FAKE_SOROBAN="$TEST_ROOT/fake-soroban"
printf '#!/usr/bin/env bash\necho "soroban 21.5.0 (abc123)"\n' >"$FAKE_SOROBAN"
chmod +x "$FAKE_SOROBAN"
SOROBAN="$FAKE_SOROBAN" SOROBAN_VERSION="21.5.0" \
  bash "$ROOT_DIR/scripts/check_soroban_version.sh" >/dev/null ||
  fail "version check rejected a matching soroban-cli"
if MISMATCH_OUTPUT="$(SOROBAN="$FAKE_SOROBAN" SOROBAN_VERSION="99.0.0" \
  bash "$ROOT_DIR/scripts/check_soroban_version.sh" 2>&1)"; then
  fail "version check accepted a mismatched soroban-cli"
fi
[[ "$MISMATCH_OUTPUT" == *"local 21.5.0"*"pinned 99.0.0"* ]] ||
  fail "version mismatch message does not name both versions"

echo "tooling smoke tests passed"
