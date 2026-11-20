#!/usr/bin/env bash
# BettaPay — Soroban CLI Version Check
# Fails when the local soroban-cli does not match the version pinned in the Makefile.

set -euo pipefail

SOROBAN="${SOROBAN:-soroban}"
: "${SOROBAN_VERSION:?SOROBAN_VERSION must be set (run via: make check_soroban)}"

if ! command -v "$SOROBAN" >/dev/null 2>&1; then
  echo "soroban-cli not found (looked for '$SOROBAN'). Pinned version is $SOROBAN_VERSION." >&2
  echo "Install it with: make install_soroban" >&2
  exit 1
fi

# First line looks like: "soroban 21.5.0 (<commit>)"
LOCAL_VERSION="$("$SOROBAN" --version | head -n 1 | awk '{print $2}')"

if [ "$LOCAL_VERSION" != "$SOROBAN_VERSION" ]; then
  echo "soroban-cli version mismatch: local $LOCAL_VERSION, pinned $SOROBAN_VERSION." >&2
  echo "Install the pinned version with: make install_soroban" >&2
  exit 1
fi

echo "soroban-cli $LOCAL_VERSION matches the pinned version."
