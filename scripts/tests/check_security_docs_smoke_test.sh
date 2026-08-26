#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

fail() {
  echo "check_security_docs smoke test failed: $1" >&2
  exit 1
}

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/.github"

write_fixture() {
  cat >"$TEST_ROOT/SECURITY.md" <<'EOF'
# Security Policy

Report to security@bettapay.com. See .github/SECURITY_REPORT_TEMPLATE.md.
EOF
  printf '# Vulnerability Report Template\n' >"$TEST_ROOT/.github/SECURITY_REPORT_TEMPLATE.md"
  cat >"$TEST_ROOT/README.md" <<'EOF'
# Project

See SECURITY.md for the security policy.
EOF
}

# A fully-consistent set of docs must pass.
write_fixture
if ! ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_security_docs.sh" >/dev/null; then
  fail "a consistent set of security docs was rejected"
fi

# Missing template file must fail.
write_fixture
rm "$TEST_ROOT/.github/SECURITY_REPORT_TEMPLATE.md"
if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_security_docs.sh" >/dev/null 2>&1; then
  fail "a missing report template was accepted"
fi

# SECURITY.md not referencing the template must fail.
write_fixture
printf '# Security Policy\n\nReport to security@bettapay.com.\n' >"$TEST_ROOT/SECURITY.md"
if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_security_docs.sh" >/dev/null 2>&1; then
  fail "SECURITY.md missing a template reference was accepted"
fi

# SECURITY.md without a contact address must fail.
write_fixture
printf '# Security Policy\n\nSee .github/SECURITY_REPORT_TEMPLATE.md.\n' >"$TEST_ROOT/SECURITY.md"
if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_security_docs.sh" >/dev/null 2>&1; then
  fail "SECURITY.md missing a contact address was accepted"
fi

# README not linking to SECURITY.md must fail.
write_fixture
printf '# Project\n\nNo security section here.\n' >"$TEST_ROOT/README.md"
if ROOT_DIR="$TEST_ROOT" bash "$ROOT_DIR/scripts/check_security_docs.sh" >/dev/null 2>&1; then
  fail "a README with no SECURITY.md link was accepted"
fi

echo "check_security_docs smoke tests passed"
