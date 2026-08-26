#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

fail() {
  echo "check_codeowners smoke test failed: $1" >&2
  exit 1
}

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/.github" "$TEST_ROOT/adr" "$TEST_ROOT/scripts"

# A well-formed CODEOWNERS whose patterns all resolve must pass.
cat >"$TEST_ROOT/.github/CODEOWNERS" <<'EOF'
# a comment, and a blank line above should both be ignored

/adr/       @some-user
/scripts/   @some-user @org/some-team
EOF

if ! ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_codeowners.sh" >/dev/null; then
  fail "valid CODEOWNERS file was rejected"
fi

# A pattern with no owner must fail.
cat >"$TEST_ROOT/.github/CODEOWNERS" <<'EOF'
/adr/
EOF

if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "a pattern with no owner was accepted"
fi

# A malformed owner (missing @) must fail.
cat >"$TEST_ROOT/.github/CODEOWNERS" <<'EOF'
/adr/   some-user
EOF

if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "an owner missing the leading @ was accepted"
fi

# A pattern that doesn't resolve to a real path must fail.
cat >"$TEST_ROOT/.github/CODEOWNERS" <<'EOF'
/does-not-exist/   @some-user
EOF

if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "a pattern with no matching path was accepted"
fi

# A missing CODEOWNERS file entirely must fail, not silently pass.
rm "$TEST_ROOT/.github/CODEOWNERS"

if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "a missing CODEOWNERS file was accepted"
fi

echo "check_codeowners smoke tests passed"
