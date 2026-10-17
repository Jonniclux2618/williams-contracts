#!/usr/bin/env bash
#
# run-deploy-sandbox.sh — smoke-test scripts/deploy-testnet.sh against a
# local Stellar standalone (Quickstart) container (#78).
#
# This script starts a throwaway Stellar standalone network inside Docker,
# generates a funded identity, builds the contract, deploys it, and verifies
# that get_contract_version returns the expected value.  It mirrors exactly
# what deploy-testnet.sh does against the live testnet, so any regression in
# the deployment logic is caught before it reaches CI or testnet.
#
# Prerequisites (all are available in the GitHub Actions ubuntu-latest runner):
#   - Docker
#   - stellar-cli >= 22.0.0  (`cargo install --locked stellar-cli`)
#   - rustup target wasm32v1-none
#
# Usage (local):
#   ./scripts/run-deploy-sandbox.sh
#
# Usage (CI): this script is invoked by .github/workflows/scripts-check.yml
# in the deploy-sandbox job.  It exits 0 on success and non-zero on any
# failure, causing the CI job to fail.
#
# Environment variables (all optional):
#   SANDBOX_IMAGE   Docker image for the Stellar quickstart container.
#                   Default: stellar/quickstart:latest
#   SANDBOX_PORT    Host port to expose the RPC on. Default: 8000
#   KEEP_CONTAINER  If set to "true", the container is not removed on exit.
#                   Useful for debugging failed runs locally.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SANDBOX_IMAGE="${SANDBOX_IMAGE:-stellar/quickstart:latest}"
SANDBOX_PORT="${SANDBOX_PORT:-8000}"
KEEP_CONTAINER="${KEEP_CONTAINER:-false}"
CONTAINER_NAME="williams-sandbox-$$"
RPC_URL="http://localhost:${SANDBOX_PORT}/soroban/rpc"
NETWORK_PASSPHRASE="Standalone Network ; February 2017"
DEPLOY_IDENTITY="sandbox-deployer"

log() { echo "[sandbox] $*"; }

cleanup() {
    if [[ "$KEEP_CONTAINER" != "true" ]]; then
        log "Stopping and removing container $CONTAINER_NAME …"
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    else
        log "KEEP_CONTAINER=true — leaving container $CONTAINER_NAME running."
    fi
}
trap cleanup EXIT

# ── Check dependencies ───────────────────────────────────────────────────────
for cmd in docker stellar; do
    command -v "$cmd" >/dev/null 2>&1 || {
        echo "error: '$cmd' not found on PATH." >&2
        exit 1
    }
done

# ── Start Stellar standalone container ───────────────────────────────────────
log "Pulling $SANDBOX_IMAGE …"
docker pull "$SANDBOX_IMAGE"

log "Starting Stellar standalone container on port $SANDBOX_PORT …"
docker run --detach \
    --name "$CONTAINER_NAME" \
    -p "${SANDBOX_PORT}:8000" \
    "$SANDBOX_IMAGE" \
    --standalone \
    --enable-soroban-rpc 2>&1 | tail -1

# Wait for the RPC to be ready (up to 60 s).
log "Waiting for Soroban RPC to be ready …"
for i in $(seq 1 60); do
    if curl --silent --fail "${RPC_URL}" >/dev/null 2>&1; then
        log "RPC ready after ${i}s."
        break
    fi
    if [[ $i -eq 60 ]]; then
        echo "error: Soroban RPC at $RPC_URL did not become ready within 60 s." >&2
        exit 1
    fi
    sleep 1
done

# ── Configure stellar-cli to talk to the sandbox ─────────────────────────────
log "Configuring stellar-cli network alias 'sandbox' …"
stellar network add sandbox \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" 2>/dev/null || true

# ── Generate and fund a deployer identity ────────────────────────────────────
log "Generating deployer identity '$DEPLOY_IDENTITY' …"
stellar keys generate "$DEPLOY_IDENTITY" --network sandbox --fund 2>/dev/null || true

# ── Build the contract ───────────────────────────────────────────────────────
log "Building contract (wasm32v1-none release) …"
cargo build --package will --release --target wasm32v1-none

WASM_PATH="target/wasm32v1-none/release/will.wasm"
[[ -f "$WASM_PATH" ]] || { echo "error: wasm artifact not found at $WASM_PATH" >&2; exit 1; }

# ── Deploy to sandbox ────────────────────────────────────────────────────────
log "Deploying will.wasm to standalone sandbox …"
CONTRACT_ID="$(stellar contract deploy \
    --wasm "$WASM_PATH" \
    --source "$DEPLOY_IDENTITY" \
    --network sandbox)"

[[ -n "$CONTRACT_ID" ]] || { echo "error: deploy returned empty contract id." >&2; exit 1; }
log "Deployed: $CONTRACT_ID"

# ── Verify get_contract_version ──────────────────────────────────────────────
log "Verifying get_contract_version matches source …"
LIB_RS="contracts/will/src/lib.rs"
EXPECTED_VERSION_RAW="$(grep -E '^pub const CONTRACT_VERSION: u32 = ' "$LIB_RS" \
    | sed -E 's/^pub const CONTRACT_VERSION: u32 = ([0-9_]+);.*/\1/')"
EXPECTED_VERSION="${EXPECTED_VERSION_RAW//_/}"

ONCHAIN_VERSION_RAW="$(stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source "$DEPLOY_IDENTITY" \
    --network sandbox \
    -- get_contract_version)"
ONCHAIN_VERSION="$(echo "$ONCHAIN_VERSION_RAW" | tr -d '"[:space:]')"

if [[ "$ONCHAIN_VERSION" != "$EXPECTED_VERSION" ]]; then
    echo "error: on-chain get_contract_version=$ONCHAIN_VERSION != source CONTRACT_VERSION=$EXPECTED_VERSION" >&2
    exit 1
fi
log "OK: on-chain version $ONCHAIN_VERSION matches source CONTRACT_VERSION $EXPECTED_VERSION."

log "Sandbox deployment smoke-test PASSED."
