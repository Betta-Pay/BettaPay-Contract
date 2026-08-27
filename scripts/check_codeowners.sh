#!/usr/bin/env bash
# BettaPay — CODEOWNERS Validation Script
#
# Validates that:
#   1. A CODEOWNERS file exists in one of the locations GitHub recognizes
#      (repo root, .github/, or docs/).
#   2. Every non-comment, non-blank entry has a pattern plus at least one
#      `@`-prefixed owner.
#   3. Every pattern resolves to at least one real file (or file under a
#      real directory) in the repository, so patterns can't silently rot
#      as the tree is restructured.
#
# This intentionally does NOT verify that owner handles/teams exist on
# GitHub — that requires API access this script does not assume.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

ROOT_DIR="${ROOT_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"

# GitHub checks these locations, in this order.
CODEOWNERS_FILE=""
for candidate in "CODEOWNERS" ".github/CODEOWNERS" "docs/CODEOWNERS"; do
  if [ -f "${ROOT_DIR}/${candidate}" ]; then
    CODEOWNERS_FILE="${ROOT_DIR}/${candidate}"
    break
  fi
done

if [ -z "$CODEOWNERS_FILE" ]; then
  log_error "No CODEOWNERS file found at repo root, .github/, or docs/."
  exit 1
fi

log_info "Validating $CODEOWNERS_FILE"

# Build the list of real files in the repo (relative paths), skipping VCS
# and build directories.
FILES_LIST="$(cd "$ROOT_DIR" && find . \
  -type f \
  -not -path './.git/*' \
  -not -path './target/*' \
  | sed 's#^\./##')"

# Returns 0 if `pattern` (CODEOWNERS/gitignore-style) matches at least one
# path in FILES_LIST.
pattern_matches_a_real_path() {
  local pattern="$1"
  local anchored=0
  local is_dir=0
  local p="$pattern"

  if [[ "$p" == /* ]]; then
    anchored=1
    p="${p#/}"
  fi
  if [[ "$p" == */ ]]; then
    is_dir=1
    p="${p%/}"
  fi

  local glob
  if [[ $is_dir -eq 1 ]]; then
    if [[ $anchored -eq 1 ]]; then
      glob="$p/*"
    else
      glob="*$p/*"
    fi
  else
    if [[ $anchored -eq 1 ]]; then
      glob="$p"
    else
      glob="*$p"
    fi
  fi

  local f
  while IFS= read -r f; do
    [ -z "$f" ] && continue
    if [[ "$f" == $glob ]] || [[ "$f" == "$p" ]]; then
      return 0
    fi
  done <<<"$FILES_LIST"

  return 1
}

ERRORS=0
LINE_NO=0
ENTRIES_CHECKED=0

while IFS= read -r raw_line || [ -n "$raw_line" ]; do
  LINE_NO=$((LINE_NO + 1))

  # Strip comments and surrounding whitespace.
  line="${raw_line%%#*}"
  line="$(echo "$line" | xargs || true)"

  [ -z "$line" ] && continue

  # First field is the pattern, remaining fields are owners.
  read -r -a fields <<<"$line"
  pattern="${fields[0]}"
  owners=("${fields[@]:1}")

  ENTRIES_CHECKED=$((ENTRIES_CHECKED + 1))

  if [ "${#owners[@]}" -eq 0 ]; then
    log_error "Line ${LINE_NO}: pattern '${pattern}' has no owners assigned."
    ERRORS=$((ERRORS + 1))
    continue
  fi

  for owner in "${owners[@]}"; do
    if [[ "$owner" != @* ]]; then
      log_error "Line ${LINE_NO}: owner '${owner}' for pattern '${pattern}' must start with '@' (user or team handle)."
      ERRORS=$((ERRORS + 1))
    fi
  done

  if ! pattern_matches_a_real_path "$pattern"; then
    log_error "Line ${LINE_NO}: pattern '${pattern}' does not match any real file or directory in the repository."
    ERRORS=$((ERRORS + 1))
  fi
done <"$CODEOWNERS_FILE"

if [ "$ENTRIES_CHECKED" -eq 0 ]; then
  log_error "CODEOWNERS file has no ownership entries."
  exit 1
fi

echo "================================================================"
if [ "$ERRORS" -gt 0 ]; then
  log_error "CODEOWNERS validation failed with ${ERRORS} error(s) across ${ENTRIES_CHECKED} entries."
  exit 1
fi

log_success "CODEOWNERS validation passed: ${ENTRIES_CHECKED} entries all resolve to real paths and have owners."
