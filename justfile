# Task runner for williams-contracts.
#
# Install `just`: https://github.com/casey/just#installation
# Then run `just --list` to see all recipes, or `just <recipe>`.
#
# These recipes mirror exactly what .github/workflows/test.yml and
# .github/workflows/audit.yml run in CI, so `just ci` locally is a strong
# predictor of whether a PR will pass.

set shell := ["bash", "-uc"]

# Show available recipes.
default:
    just --list

# Run the full test suite (equivalent to CI's `cargo test --workspace`).
test:
    cargo test --workspace

# Run clippy with the exact flags CI enforces — fails on any warning.
lint:
    cargo clippy --all-targets -- -D warnings

# Build the contract for the wasm32v1-none target in release mode.
build:
    cargo build --workspace --release --target wasm32v1-none

# Format all Rust source in the workspace.
fmt:
    cargo fmt --all

# Check formatting without writing changes (useful before opening a PR).
fmt-check:
    cargo fmt --all -- --check

# Scan Cargo.lock for known vulnerabilities (same check as the Security
# Audit CI workflow). Requires cargo-audit: `cargo install cargo-audit`.
audit:
    cargo audit --file Cargo.lock --deny warnings

# Verify the exported contract spec is up to date (fails if stale).
spec:
    scripts/export-spec.sh --check

# Profile resource costs of the contract entry points.
costs:
    scripts/check_resource_costs.sh

# Deploy the contract to testnet.
deploy-testnet:
    scripts/deploy-testnet.sh

# Run the `create_will` fuzz target. Requires cargo-fuzz:
# `cargo install cargo-fuzz` and a nightly toolchain.
fuzz-create:
    cargo +nightly fuzz run create_will

# Run everything CI runs, in order.
ci: fmt-check lint test build spec
