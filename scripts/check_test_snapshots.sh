#!/usr/bin/env bash
# BettaPay — Test Snapshot Drift Check
#
# Soroban's `Env` test harness writes a JSON snapshot file for every #[test]
# under each crate's `test_snapshots/` directory, named after the test's
# module path. Running the suite silently rewrites those files in place, so a
# snapshot that no longer matches committed contract behavior (or one that
# was never committed at all) is easy to miss in review. This script re-runs
# the workspace test suite and fails if that leaves any test_snapshots/ file
# modified, added, or removed relative to what's committed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

cd "$ROOT_DIR"

log_info "Running workspace tests to regenerate test_snapshots/ artifacts..."
cargo test --workspace >/dev/null

log_info "Checking for test_snapshots/ drift..."

DRIFT="$(git status --porcelain -- '*/test_snapshots/')"

if [ -n "$DRIFT" ]; then
  log_error "test_snapshots/ is out of sync with the working tree:"
  echo "$DRIFT" >&2
  log_info "If this change is intentional, commit the updated snapshot files and explain the change in your PR description."
  log_info "If it isn't, your change altered emitted events or storage layout unexpectedly — investigate before committing."
  exit 1
fi

log_success "test_snapshots/ matches committed state — no drift detected."
