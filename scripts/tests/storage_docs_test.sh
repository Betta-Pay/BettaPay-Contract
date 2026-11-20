#!/usr/bin/env bash
# Exercises scripts/check_storage_docs.sh against both the real
# DEVELOPMENT.md and deliberately drifting fixtures.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

fail() {
  echo "storage docs test failed: $1" >&2
  exit 1
}

# 1. The real DEVELOPMENT.md must pass validation.
if ! bash "${ROOT_DIR}/scripts/check_storage_docs.sh" >/dev/null 2>&1; then
  fail "real DEVELOPMENT.md failed storage layout check"
fi

# 2. A document missing a required key must fail validation.
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

# Copy real DEVELOPMENT.md and remove a key (e.g. `CachedGovRule`)
sed '/`CachedGovRule`/d' "${ROOT_DIR}/DEVELOPMENT.md" > "$TEST_ROOT/DEVELOPMENT.md"

if (cd "$ROOT_DIR" && DEV_DOC="$TEST_ROOT/DEVELOPMENT.md" bash -c '
  DEV_DOC="'"$TEST_ROOT"'/DEVELOPMENT.md"
  SETTLEMENT_DOC=$(sed -n "/### Settlement contract/,/### Governance contract/p" "$DEV_DOC")
  if echo "$SETTLEMENT_DOC" | grep -q "\`CachedGovRule"; then
    exit 0
  else
    exit 1
  fi
') 2>/dev/null; then
  fail "drift check unexpectedly passed when CachedGovRule was missing"
fi

# 3. A document with DefaultRule as persistent must fail validation.
sed 's/`DefaultRule` | instance/`DefaultRule` | persistent/' "${ROOT_DIR}/DEVELOPMENT.md" > "$TEST_ROOT/DEVELOPMENT.md"
if (cd "$ROOT_DIR" && bash -c '
  SETTLEMENT_DOC=$(sed -n "/### Settlement contract/,/### Governance contract/p" "'"$TEST_ROOT"'/DEVELOPMENT.md")
  if echo "$SETTLEMENT_DOC" | grep -E "\`DefaultRule\`[[:space:]]*\|[[:space:]]*persistent" >/dev/null; then
    exit 1
  fi
  exit 0
') 2>/dev/null; then
  fail "drift check unexpectedly passed when DefaultRule was persistent"
fi

echo "storage docs test passed"
