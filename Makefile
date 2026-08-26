SOROBAN ?= soroban

.PHONY: build
build:
	@echo "Building all contracts..."
	cargo build --target wasm32-unknown-unknown --release

.PHONY: optimize
optimize: build
	@mkdir -p target/optimized
	@for contract in $(shell find . -path "*/target/wasm32-unknown-unknown/release/*.wasm" -type f); do \
		output=$$(basename $$contract .wasm)_opt.wasm; \
		$(SOROBAN) contract optimize --wasm $$contract --optimized-wasm target/optimized/$$output; \
	done

.PHONY: clean
clean:
	cargo clean
	@rm -rf target/optimized

.PHONY: fmt test check clippy test_scripts wasm_size check_codeowners all

fmt:
	cargo fmt --all -- --check
.PHONY: fmt test check clippy all

fmt:
	cargo fmt --all --check

test:
	cargo test --workspace
	cargo test --workspace --release

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test_scripts:
	bash scripts/tests/tooling_smoke_test.sh
	bash scripts/tests/codeowners_check_test.sh

wasm_size: optimize
	bash scripts/check_wasm_size.sh

check_codeowners:
	bash scripts/check_codeowners.sh

all: fmt check clippy test test_scripts wasm_size check_codeowners
all: fmt check clippy test
