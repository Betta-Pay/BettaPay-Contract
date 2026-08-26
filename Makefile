SOROBAN ?= soroban

.PHONY: build optimize clean fmt test check clippy test_scripts wasm_size all

build:
	@echo "Building all contracts..."
	cargo build --target wasm32-unknown-unknown --release

optimize: build
	@mkdir -p target/optimized
	@for contract in $(shell find . -path "*/target/wasm32-unknown-unknown/release/*.wasm" -type f); do \
		output=$$(basename $$contract .wasm)_opt.wasm; \
		$(SOROBAN) contract optimize --wasm $$contract --optimized-wasm target/optimized/$$output; \
	done

clean:
	cargo clean
	@rm -rf target/optimized

fmt:
	cargo fmt --all -- --check

test:
	cargo test --workspace

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test_scripts:
	bash scripts/tests/tooling_smoke_test.sh

wasm_size: optimize
	bash scripts/check_wasm_size.sh

all: fmt check clippy test test_scripts
