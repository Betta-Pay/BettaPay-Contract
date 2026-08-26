# Contributing to BettaPay Contracts

Thank you for contributing to the BettaPay Soroban smart contracts. This guide covers workspace setup, development workflow, and testing expectations so you can get productive quickly.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Workspace Configuration](#workspace-configuration)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Code Ownership](#code-ownership)
- [Upgrades and Storage Migrations](DEVELOPMENT.md)
- [Testing](#testing)
- [Building WASM Binaries](#building-wasm-binaries)
- [Optional: Soroban CLI Scripts](#optional-soroban-cli-scripts)
- [Pull Request Checklist](#pull-request-checklist)
- [Commit Message Conventions](#commit-message-conventions)
- [Reporting Issues](#reporting-issues)

## Prerequisites

Install the following before working on this repository:

| Tool | Purpose | Notes |
|------|---------|-------|
| [Rust](https://rustup.rs/) | Build and test contracts | Version pinned in `rust-toolchain.toml` (currently **1.85.0**) |
| `wasm32-unknown-unknown` target | Compile Soroban WASM | Installed automatically via `rust-toolchain.toml` |
| [Soroban CLI](https://developers.stellar.org/docs/tools/developer-tools/cli) | Deploy and simulate on testnet | Required only for `scripts/` workflows |

Clone the repository and enter the workspace root:

```bash
git clone https://github.com/Betta-Pay/BettaPay-Contract.git
cd BettaPay-Contract
```

Rustup reads `rust-toolchain.toml` on first `cargo` invocation and installs the correct toolchain and WASM target.

## Workspace Configuration

This repository is a **Cargo workspace** containing two independently deployable Soroban contracts and a shared library crate. Understanding the config files helps when adding dependencies, running targeted builds, or debugging CI failures.

### Root `Cargo.toml`

The workspace root ties the contracts and shared crate together:

```toml
[workspace]
members = ["settlement_contract", "governance_contract", "bettapay_common"]
resolver = "2"

[workspace.package]
edition = "2021"
license = "MIT"
publish = false

[workspace.dependencies]
soroban-sdk = "21.7.7"
bettapay_common = { path = "bettapay_common" }
```

| Section | Purpose |
|---------|---------|
| `members` | Lists each crate in the workspace |
| `resolver = "2"` | Uses Cargo's dependency resolver v2 (required for edition 2021) |
| `[workspace.package]` | Shared metadata inherited by member crates |
| `[workspace.dependencies]` | Single source of truth for shared dependency versions |

Both contracts reference the SDK with `soroban-sdk = { workspace = true }`, and the shared `bettapay_common` crate with `bettapay_common = { workspace = true }`, so version bumps happen in one place.

### `rust-toolchain.toml`

Pins the Rust toolchain for reproducible builds across local machines and CI:

```toml
[toolchain]
channel = "1.85.0"
targets = ["wasm32-unknown-unknown"]
```

- `channel` — exact Rust version used for compilation and tests
- `targets` — ensures the WASM compilation target is available

### Per-crate `Cargo.toml`

Each contract under `settlement_contract/` and `governance_contract/` is a `cdylib` crate:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
soroban-sdk = { workspace = true }

[dev-dependencies]
soroban-sdk = { workspace = true, features = ["testutils"] }
```

- `cdylib` — produces a dynamic library suitable for Soroban WASM output
- `testutils` feature — enabled only in dev-dependencies for in-memory contract tests

### Repository layout

```
BettaPay-Contract/
├── Cargo.toml                  # Workspace root
├── Cargo.lock                  # Locked dependency graph (committed)
├── rust-toolchain.toml         # Pinned Rust + WASM target
├── settlement_contract/        # Merchant registration, fee splits, payment refs
│   ├── Cargo.toml
│   ├── src/lib.rs
│   └── test_snapshots/         # Soroban test snapshot artifacts
├── governance_contract/        # Fee config, anchor registry, system params
│   ├── Cargo.toml
│   ├── src/lib.rs
│   └── test_snapshots/
├── bettapay_common/             # Shared types, events, storage helpers, error codes
│   ├── Cargo.toml
│   └── src/lib.rs
└── scripts/
    ├── deploy_testnet.sh       # Build + deploy both contracts to testnet
    └── simulate.sh             # Local deploy + init for simulation
```

## Getting Started

After cloning, verify your environment:

```bash
# Confirm toolchain (should report 1.85.0)
rustc --version

# Install dependencies and compile host targets
cargo build

# Build WASM (required before first test run — see Testing)
cargo build --target wasm32-unknown-unknown --release

# Run the full test suite
cargo test --workspace
```

If `cargo` prompts to install the toolchain, accept — `rust-toolchain.toml` handles the rest.

## Development Workflow

1. **Find or open an issue** — check [open issues](https://github.com/Betta-Pay/BettaPay-Contract/issues) before starting work.
2. **Create a feature branch** from `main`:
   ```bash
   git checkout main
   git pull origin main
   git checkout -b your-name/short-description
   ```
3. **Make focused changes** — keep PRs scoped to a single concern (one contract fix, one feature, or one docs change).
4. **Run local checks** before pushing (see [Testing](#testing) and [Pull Request Checklist](#pull-request-checklist)).
5. **Open a pull request** against `main` with a clear description and test plan.

## Code Ownership

[`.github/CODEOWNERS`](.github/CODEOWNERS) gates review on paths where an unreviewed change can silently diverge from the rest of the workspace:

- [`adr/`](adr/) — Architecture Decision Records. Changing an accepted decision, or the rationale behind it, needs sign-off from someone who understands the original tradeoff.
- [`bettapay_common/src/constants.rs`](bettapay_common/src/constants.rs) — constants shared by both contracts (TTL policy, `BPS_DENOMINATOR`, the recovery delay, ...). A change here silently affects both contracts at once.
- [`scripts/`](scripts/), [`Makefile`](Makefile), [`.github/workflows/`](.github/workflows/) — deployment and CI-invoked tooling. Breaking these breaks every contributor's local `make all` and the CI gate itself.
- [`CONTRIBUTING.md`](CONTRIBUTING.md), [`SECURITY.md`](SECURITY.md), [`DEVELOPMENT.md`](DEVELOPMENT.md) — contributor-facing process docs.

**How it's enforced:** GitHub requests a review from the matched owner(s) automatically when a PR touches one of these paths — that's advisory on its own. To make it a hard merge gate, a repo admin needs to enable **"Require review from Code Owners"** under Settings → Branches → branch protection rule for `main` (this is a GitHub repository setting, not something a config file in the repo can turn on by itself). `make check_codeowners` (part of `make all`, so it runs in CI on every PR) validates the file itself: every pattern must still resolve to a real path in the repository, and every owner must be a syntactically valid `@user` or `@org/team` handle. It catches CODEOWNERS drifting out of sync with the repo (e.g. a gated path getting renamed) — it does not and cannot enforce that reviews actually happened; that half is the branch protection setting above.

If you're touching a gated path and don't have another maintainer to loop in, say so in the PR description — the owner list can always be adjusted as the contributor base grows.

## Testing

Contract logic is tested with Soroban's in-memory `Env` in each crate's `#[cfg(test)] mod tests` block inside `src/lib.rs`.

### Run all tests

`governance_contract` test compilation embeds the release WASM via `include_bytes!`, so build WASM once before the first test run:

```bash
cargo build --target wasm32-unknown-unknown --release
cargo test --workspace
```

### Run tests for a single contract

```bash
cargo test -p settlement_contract
cargo test -p governance_contract
```

### Run a specific test by name

```bash
cargo test -p settlement_contract merchant_lifecycle_uses_canonical_topics
```

### Test snapshots

Some tests write JSON snapshots under `test_snapshots/tests/`. If you intentionally change contract behavior that affects emitted events or storage layout, update snapshots as part of your PR and explain the change in the PR description.

### CI parity

GitHub Actions (`.github/workflows/auto-merge.yml`) runs the contributor gate on every pull request:

```bash
make all
```

This deterministically runs formatting, workspace compilation, Clippy with all
targets and features, workspace tests, script smoke tests, WASM optimization,
and the deployed-artifact size check. Run the same command locally to avoid CI
failures.

## Building WASM Binaries

Optimized WASM artifacts are required for deployment:

```bash
make optimize
```

Output paths:

- `target/optimized/settlement_contract_opt.wasm`
- `target/optimized/governance_contract_opt.wasm`

Build a single contract:

```bash
cargo build --target wasm32-unknown-unknown --release -p settlement_contract
```

## Optional: Soroban CLI Scripts

Deployment and simulation scripts live in `scripts/` and require the Soroban CLI.

### Simulate locally (testnet RPC)

```bash
bash scripts/simulate.sh
```

Deploys both contracts, initializes admin, and writes contract IDs to `.soroban/` (gitignored).

### Deploy to testnet

```bash
bash scripts/deploy_testnet.sh
```

Environment variables (all optional, with defaults for testnet):

| Variable | Default |
|----------|---------|
| `SOROBAN_RPC_URL` | `https://soroban-testnet.stellar.org` |
| `SOROBAN_NETWORK_PASSPHRASE` | `Test SDF Network ; September 2015` |
| `BETTAPAY_IDENTITY` | `bettapay-admin` (deploy script) |
| `SOROBAN_SOURCE` | `bettapay-sim` (simulate script) |

Never commit Soroban identity keys or `.soroban/` directory contents.

## Pull Request Checklist

Before requesting review, confirm:

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `bash scripts/tests/tooling_smoke_test.sh` passes
- [ ] `make wasm_size` succeeds
- [ ] New behavior has corresponding unit tests
- [ ] Snapshot changes (if any) are intentional and documented
- [ ] PR description references the related issue (e.g. `Closes #125`)

## Commit Message Conventions

Use concise, descriptive messages in the imperative mood:

```
docs: create CONTRIBUTING.md guide
fix(settlement): reject zero-address admin transfer
test(governance): add fee bps boundary coverage
```

Prefixes: `feat`, `fix`, `test`, `docs`, `refactor`, `chore`, `tooling`.

## Reporting Issues

Use the GitHub issue templates when filing bugs or feature requests:

- **Bug Report** — include reproduction steps, expected vs actual behavior, and environment details (network, Rust/soroban-cli versions, commit hash).
- **Feature Request** — describe the problem, proposed solution, and affected sub-system.

For contract-specific work, select **Smart Contracts (BettaPay-Contract)** in the subsystem dropdown.
