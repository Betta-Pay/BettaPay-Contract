#!/usr/bin/env bash
# BettaPay — Documentation value drift check
# Keeps the documented contract limits aligned with their source constants.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
LIMITS_DOC="$ROOT_DIR/docs/contract-limits.md"
WASM_DOC="$ROOT_DIR/docs/wasm-size.md"

read_rust_constant() {
  local file="$1"
  local name="$2"
  sed -n "s/^pub const ${name}:[^=]*= *\([^;]*\);.*/\1/p" "$file" \
    | head -n 1 \
    | tr -d ' _'
}

check_constant() {
  local file="$1"
  local name="$2"
  local value
  value="$(read_rust_constant "$file" "$name")"
  if [ -z "$value" ]; then
    echo "Missing source constant: $name" >&2
    exit 1
  fi
  if ! grep -Fq "| \`${name}\` | \`${value}\`" "$LIMITS_DOC"; then
    echo "Documentation drift: $name should be documented as $value" >&2
    exit 1
  fi
}

check_constant "$ROOT_DIR/settlement_contract/src/policy.rs" MIN_PAYMENT_AMOUNT
check_constant "$ROOT_DIR/settlement_contract/src/policy.rs" MAX_SETTLEMENT_DELAY_LEDGER
check_constant "$ROOT_DIR/settlement_contract/src/policy.rs" MAX_PAYMENTS_BATCH
check_constant "$ROOT_DIR/settlement_contract/src/policy.rs" MAX_PAYMENTS_PER_MERCHANT
check_constant "$ROOT_DIR/bettapay_common/src/constants.rs" MIN_FEE_BPS
check_constant "$ROOT_DIR/bettapay_common/src/constants.rs" MAX_FEE_BPS
check_constant "$ROOT_DIR/bettapay_common/src/constants.rs" BPS_DENOMINATOR

warn_limit="$(sed -n 's/^WARN_LIMIT=\([0-9]*\)$/\1/p' "$ROOT_DIR/scripts/check_wasm_size.sh")"
error_limit="$(sed -n 's/^ERROR_LIMIT=\([0-9]*\).*/\1/p' "$ROOT_DIR/scripts/check_wasm_size.sh")"
grep -Fq "| Warning | \`${warn_limit}\`" "$WASM_DOC" || {
  echo "Documentation drift: warning Wasm limit should be $warn_limit" >&2
  exit 1
}
grep -Fq "| Error | \`${error_limit}\`" "$WASM_DOC" || {
  echo "Documentation drift: error Wasm limit should be $error_limit" >&2
  exit 1
}

BOOTSTRAP_DOC="$ROOT_DIR/docs/bootstrap-fallback.md"
if [ -f "$BOOTSTRAP_DOC" ]; then
  grep -Fq '| `platform_fee_bps` | `100` |' "$BOOTSTRAP_DOC" || {
    echo "Documentation drift: platform_fee_bps in bootstrap doc should be 100" >&2
    exit 1
  }
  grep -Fq '| `network_fee_bps` | `5` |' "$BOOTSTRAP_DOC" || {
    echo "Documentation drift: network_fee_bps in bootstrap doc should be 5" >&2
    exit 1
  }
  grep -Fq '| `settlement_delay_ledger` | `0` |' "$BOOTSTRAP_DOC" || {
    echo "Documentation drift: settlement_delay_ledger in bootstrap doc should be 0" >&2
    exit 1
  }
  grep -Fq '| `auto_settle` | `false` |' "$BOOTSTRAP_DOC" || {
    echo "Documentation drift: auto_settle in bootstrap doc should be false" >&2
    exit 1
  }
fi

echo "Documented contract limits and bootstrap defaults match source values."
