#!/usr/bin/env bash
# Exercises scripts/check_codeowners.sh against both the repo's real
# CODEOWNERS file and deliberately broken fixtures, so regressions in the
# validator itself (or in CODEOWNERS drifting from the real tree) are
# caught in CI.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

fail() {
  echo "codeowners check test failed: $1" >&2
  exit 1
}

# 1. The real, committed CODEOWNERS file must pass as-is.
if ! ROOT_DIR="$ROOT_DIR" bash "${ROOT_DIR}/scripts/check_codeowners.sh" >/tmp/codeowners_ok.log 2>&1; then
  cat /tmp/codeowners_ok.log >&2
  fail "the repository's real CODEOWNERS file did not pass validation"
fi
rm -f /tmp/codeowners_ok.log

# 2. A pattern that matches nothing in the tree must fail the gate.
BAD_ROOT="$(mktemp -d)"
trap 'rm -rf "$BAD_ROOT"' EXIT
mkdir -p "$BAD_ROOT/real_dir"
touch "$BAD_ROOT/real_dir/keep.rs"
cat >"$BAD_ROOT/CODEOWNERS" <<'EOF'
# a pattern with no matching path anywhere in the tree
/this/path/does/not/exist/ @someone
EOF

if ROOT_DIR="$BAD_ROOT" bash "${ROOT_DIR}/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "validator accepted a pattern with no matching path"
fi

# 3. An entry missing an owner must fail the gate.
cat >"$BAD_ROOT/CODEOWNERS" <<'EOF'
/real_dir/
EOF

if ROOT_DIR="$BAD_ROOT" bash "${ROOT_DIR}/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "validator accepted a pattern with no owner"
fi

# 4. An owner not prefixed with '@' must fail the gate.
cat >"$BAD_ROOT/CODEOWNERS" <<'EOF'
/real_dir/ someone-without-at-sign
EOF

if ROOT_DIR="$BAD_ROOT" bash "${ROOT_DIR}/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "validator accepted an owner handle missing the '@' prefix"
fi

# 5. A well-formed CODEOWNERS matching a real path must pass.
cat >"$BAD_ROOT/CODEOWNERS" <<'EOF'
# comment lines and blank lines below must be ignored

/real_dir/ @someone
EOF

if ! ROOT_DIR="$BAD_ROOT" bash "${ROOT_DIR}/scripts/check_codeowners.sh" >/dev/null 2>&1; then
  fail "validator rejected a well-formed CODEOWNERS file with a matching path"
fi

echo "codeowners_check_test.sh: all cases passed"
