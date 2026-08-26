#!/usr/bin/env bash
# BettaPay — CODEOWNERS Validation Script
#
# .github/CODEOWNERS gates review on cross-cutting paths (ADRs, shared
# constants, tooling scripts, process docs). GitHub silently ignores any
# line it can't parse, and silently stops matching a pattern the day the
# path it names is renamed or removed — neither failure is visible without
# reading the file closely. This script catches both mechanically: every
# non-comment line must have a syntactically valid owner, and every
# pattern must still resolve to a real path in the working tree.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="${ROOT_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"

# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

cd "$ROOT_DIR"

CODEOWNERS_FILE=".github/CODEOWNERS"

assert_file_exists "$CODEOWNERS_FILE"

log_info "Validating $CODEOWNERS_FILE..."

# GitHub accepts a user (@name), a team (@org/team), or an email as an
# owner; this repo only uses usernames and teams, so that's what's
# enforced here.
OWNER_RE='^@[A-Za-z0-9](-?[A-Za-z0-9])*(/[A-Za-z0-9](-?[A-Za-z0-9])*)?$'

ERRORS=0
LINE_NO=0

while IFS= read -r LINE || [ -n "$LINE" ]; do
  LINE_NO=$((LINE_NO + 1))

  # Strip comments and surrounding whitespace.
  TRIMMED="$(echo "$LINE" | sed 's/#.*$//' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  [ -z "$TRIMMED" ] && continue

  # shellcheck disable=SC2206 # word-splitting on whitespace is the point here
  FIELDS=($TRIMMED)
  PATTERN="${FIELDS[0]}"
  OWNERS=("${FIELDS[@]:1}")

  if [ "${#OWNERS[@]}" -eq 0 ]; then
    log_error "$CODEOWNERS_FILE:$LINE_NO: '$PATTERN' has no owner listed."
    ERRORS=1
    continue
  fi

  for OWNER in "${OWNERS[@]}"; do
    if ! [[ "$OWNER" =~ $OWNER_RE ]]; then
      log_error "$CODEOWNERS_FILE:$LINE_NO: '$OWNER' is not a valid @user or @org/team owner."
      ERRORS=1
    fi
  done

  # Resolve the pattern against the working tree. CODEOWNERS globs can be
  # arbitrarily complex, but every pattern actually in use here is a plain
  # path (optionally trailing `/` for a directory), so a direct existence
  # check is sufficient and catches the common drift case: a referenced
  # file or directory was renamed or deleted and CODEOWNERS wasn't
  # updated to match.
  RELATIVE_PATTERN="${PATTERN#/}"
  if [ ! -e "$RELATIVE_PATTERN" ]; then
    log_error "$CODEOWNERS_FILE:$LINE_NO: pattern '$PATTERN' does not match any file or directory in the repository."
    ERRORS=1
  fi
done <"$CODEOWNERS_FILE"

if [ "$ERRORS" -ne 0 ]; then
  log_error "CODEOWNERS validation failed."
  exit 1
fi

log_success "CODEOWNERS is valid: every pattern resolves and every owner is well-formed."
