#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# shellcheck source=../lib/common.sh
source "${ROOT_DIR}/scripts/lib/common.sh"

SECURITY_FILE="${ROOT_DIR}/SECURITY.md"
README_FILE="${ROOT_DIR}/README.md"
TEMPLATE_FILE="${ROOT_DIR}/.github/SECURITY_REPORT_TEMPLATE.md"

fail() {
  log_error "$1"
  exit 1
}

assert_file_exists "$SECURITY_FILE"
assert_file_exists "$README_FILE"
assert_file_exists "$TEMPLATE_FILE"

grep -q "Reporting a Vulnerability" "$SECURITY_FILE" ||
  fail "SECURITY.md is missing a 'Reporting a Vulnerability' section."

grep -q "Report Template" "$SECURITY_FILE" ||
  fail "SECURITY.md is missing a 'Report Template' section."

grep -q "Security Report Owners" "$SECURITY_FILE" ||
  fail "SECURITY.md is missing a 'Security Report Owners' section."

grep -q "90-day disclosure window" "$SECURITY_FILE" ||
  fail "SECURITY.md is missing the 90-day disclosure window language."

grep -q "SECURITY_REPORT_TEMPLATE.md" "$SECURITY_FILE" ||
  fail "SECURITY.md does not reference the standalone report template file."

grep -qi "SECURITY.md" "$README_FILE" ||
  fail "README.md does not link to SECURITY.md."

log_success "security docs check passed"
