SOROBAN ?= soroban
# Single source of truth for the soroban-cli version, used locally and in CI.
SOROBAN_VERSION := 21.5.0
# Minimum supported Rust version, read from rust-toolchain.toml.
MSRV := $(shell sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)

.PHONY: build
build:
	@echo "Building all contracts..."
	cargo build --target wasm32-unknown-unknown --release

.PHONY: install_soroban check_soroban
install_soroban:
	cargo install --locked soroban-cli --version $(SOROBAN_VERSION)

check_soroban:
	@SOROBAN="$(SOROBAN)" SOROBAN_VERSION="$(SOROBAN_VERSION)" bash scripts/check_soroban_version.sh

.PHONY: optimize
optimize: check_soroban build
	@mkdir -p target/optimized
	@for contract in $(shell find . -path "*/target/wasm32-unknown-unknown/release/*.wasm" -type f); do \
		output=$$(basename $$contract .wasm)_opt.wasm; \
		$(SOROBAN) contract optimize --wasm $$contract --optimized-wasm target/optimized/$$output; \
	done

.PHONY: clean
clean:
	cargo clean
	@rm -rf target/optimized

# Operator tooling targets that need runtime arguments accept a $(ARGS)
# passthrough, e.g.: make verify_wasm_hash ARGS=--contract-id CAA...
.PHONY: check_interface_parity verify_wasm_hash export_events list_pending_ops
check_interface_parity:
	@bash scripts/check_interface_parity.sh

verify_wasm_hash:
	@bash scripts/verify_wasm_hash.sh $(ARGS)

export_events:
	@bash scripts/export_events.sh $(ARGS)

list_pending_ops:
	@bash scripts/list_pending_ops.sh $(ARGS)

.PHONY: fmt test check clippy print_msrv msrv doc coverage check_snapshots test_scripts wasm_size check_codeowners all

fmt:
	cargo fmt --all --check

test:
	cargo test -p governance_contract -p bettapay_common
	cargo test -p governance_contract -p bettapay_common --release

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace --lib --all-features

print_msrv:
	@echo $(MSRV)

msrv:
	cargo +$(MSRV) check --workspace
	cargo +$(MSRV) test -p governance_contract -p bettapay_common

doc:
	RUSTDOCFLAGS="-D warnings -A rustdoc::private_intra_doc_links" \
		cargo doc --workspace --no-deps --document-private-items

coverage:
	@mkdir -p target/coverage
	cargo llvm-cov --workspace --lcov --output-path target/coverage/lcov.info

check_snapshots:
	bash scripts/check_test_snapshots.sh

test_scripts:
	bash scripts/tests/tooling_smoke_test.sh
	bash scripts/tests/codeowners_check_test.sh

wasm_size: optimize
	bash scripts/check_wasm_size.sh

check_codeowners:
	bash scripts/check_codeowners.sh

all: fmt check clippy test check_snapshots test_scripts check_codeowners check_interface_parity
