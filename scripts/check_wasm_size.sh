#!/usr/bin/env bash
# BettaPay — Wasm Binary Size Auditing Script
# Validates that compiled WebAssembly files do not exceed defined boundaries.

set -euo pipefail

# Load shared helpers
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

# Limits (in bytes)
# Soroban contracts should optimally be under 64KB (65536 bytes)
WARN_LIMIT=65536
ERROR_LIMIT=131072 # 128KB hard boundary

ROOT_DIR="${ROOT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
WASM_DIR="${WASM_DIR:-$ROOT_DIR/target/optimized}"

log_info "Auditing Wasm binary sizes in $WASM_DIR..."

if [ ! -d "$WASM_DIR" ]; then
  log_error "Optimized Wasm directory not found."
  log_info "Please optimize contracts first: make optimize"
  exit 1
fi

# Find all .wasm files in the target directory
WASM_FILES=()
while IFS=  read -r -d $'\0'; do
    WASM_FILES+=("$REPLY")
done < <(find "$WASM_DIR" -maxdepth 1 -name "*.wasm" -print0)

if [ ${#WASM_FILES[@]} -eq 0 ]; then
  log_error "No .wasm files found in $WASM_DIR."
  exit 1
fi

EXCEEDED_ERROR=0
EXCEEDED_WARN=0

echo "================================================================"
echo -e "${BOLD}Checking deployed, optimized Wasm binaries...${NC}"
echo "================================================================"

for FILE in "${WASM_FILES[@]}"; do
  FILENAME="$(basename "$FILE")"
  
  # Get file size in bytes in a cross-platform way
  if [[ "$OSTYPE" == "darwin"* ]]; then
    FILESIZE=$(stat -f "%z" "$FILE")
  else
    FILESIZE=$(stat -c "%s" "$FILE")
  fi
  
  SIZE_KB=$((FILESIZE / 1024))
  
  if [ "$FILESIZE" -gt "$ERROR_LIMIT" ]; then
    log_error "$FILENAME is ${SIZE_KB}KB ($FILESIZE bytes). Exceeds error boundary of $((ERROR_LIMIT / 1024))KB!"
    EXCEEDED_ERROR=1
  elif [ "$FILESIZE" -gt "$WARN_LIMIT" ]; then
    log_warn "$FILENAME is ${SIZE_KB}KB ($FILESIZE bytes). Exceeds warning boundary of $((WARN_LIMIT / 1024))KB."
    EXCEEDED_WARN=1
  else
    log_success "$FILENAME size is OK: ${SIZE_KB}KB ($FILESIZE bytes)."
  fi
done

echo "================================================================"

if [ "$EXCEEDED_ERROR" -eq 1 ]; then
  log_error "Validation failed: One or more Wasm binaries exceed the maximum allowed size ($((ERROR_LIMIT / 1024))KB)."
  exit 1
elif [ "$EXCEEDED_WARN" -eq 1 ]; then
  log_warn "Validation passed with warnings: One or more Wasm binaries exceed optimal size ($((WARN_LIMIT / 1024))KB)."
  exit 0
else
  log_success "Validation passed: All Wasm binaries are within size boundaries."
  exit 0
fi
