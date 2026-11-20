#!/usr/bin/env bash
# Verify the README API drift check accepts the current docs and rejects stale tokens.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CHECK_SCRIPT="${ROOT_DIR}/scripts/check_readme_api_docs.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

bash "$CHECK_SCRIPT"

sed 's/`AmountTooSmall`/`InvalidAmount`/' "${ROOT_DIR}/README.md" >"$TEST_ROOT/README.md"
if README_DOC="$TEST_ROOT/README.md" bash "$CHECK_SCRIPT" >/dev/null 2>&1; then
  echo "README API docs test failed: stale error name was accepted" >&2
  exit 1
fi

sed 's/| `upsert_anchor` | `signers: Vec<Address>`, `asset: Address`, `anchor: Address` | `()` | Stored Admin multisig, not paused | `NotInitialized`, `Paused`, `Unauthorized`, `InvalidAdmin`/| `upsert_anchor` | `signers: Vec<Address>`, `asset: Address`, `anchor: Address` | `()` | Stored Admin multisig, not paused | `NotInitialized`, `Paused`, `Unauthorized`, `ZeroAddress`/' \
  "${ROOT_DIR}/README.md" >"$TEST_ROOT/README.md"
if README_DOC="$TEST_ROOT/README.md" bash "$CHECK_SCRIPT" >/dev/null 2>&1; then
  echo "README API docs test failed: invalid governance error was accepted" >&2
  exit 1
fi

echo "README API docs tests passed"
