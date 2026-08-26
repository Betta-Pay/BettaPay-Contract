#!/usr/bin/env bash
# BettaPay — Security Documentation Consistency Check
#
# SECURITY.md, its report template, and README.md's link to it are three
# separate files that only work together if they stay in sync — nothing
# stops one of them from drifting (a renamed template file, a SECURITY.md
# rewrite that drops the template link, a README that never got the
# pointer added). This script checks the links actually resolve.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="${ROOT_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"

# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

cd "$ROOT_DIR"

SECURITY_FILE="SECURITY.md"
TEMPLATE_FILE=".github/SECURITY_REPORT_TEMPLATE.md"
README_FILE="README.md"

log_info "Checking security documentation consistency..."

ERRORS=0

assert_file_exists "$SECURITY_FILE"
assert_file_exists "$TEMPLATE_FILE"
assert_file_exists "$README_FILE"

if ! grep -q "SECURITY_REPORT_TEMPLATE.md" "$SECURITY_FILE"; then
  log_error "$SECURITY_FILE does not reference $TEMPLATE_FILE."
  ERRORS=1
fi

if ! grep -qi "security@" "$SECURITY_FILE"; then
  log_error "$SECURITY_FILE does not list a security contact address."
  ERRORS=1
fi

if ! grep -q "SECURITY.md" "$README_FILE"; then
  log_error "$README_FILE does not link to $SECURITY_FILE."
  ERRORS=1
fi

if [ "$ERRORS" -ne 0 ]; then
  log_error "Security documentation is inconsistent."
  exit 1
fi

log_success "Security documentation is consistent: SECURITY.md, its report template, and README.md's link all resolve."
