#!/usr/bin/env bash
# Deploy and configure Stellar DeFi Vault using native XLM via Stellar Asset Contract (Issue #865)
set -euo pipefail

NETWORK="${1:-testnet}"
RPC_URL="${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
NETWORK_PASSPHRASE="${STELLAR_NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}"

# Native XLM Stellar Asset Contract on Testnet:
# Native XLM address on Soroban testnet: CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
NATIVE_XLM_SAC="CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"

echo "=== Deploying Vault with Native XLM on $NETWORK ==="
echo "Using Native XLM SAC Address: $NATIVE_XLM_SAC"

# Deploy contract wasm
WASM_PATH="target/wasm32-unknown-unknown/release/stellar_defi_vault.wasm"

if [ ! -f "$WASM_PATH" ]; then
    echo "Building contract WASM..."
    cargo build --target wasm32-unknown-unknown --release
fi

VAULT_ID=$(stellar contract deploy \
    --wasm "$WASM_PATH" \
    --source-account default \
    --network "$NETWORK")

echo "Vault Contract deployed at: $VAULT_ID"

echo "Initializing vault with Native XLM SAC..."
stellar contract invoke \
    --id "$VAULT_ID" \
    --source-account default \
    --network "$NETWORK" \
    -- initialize \
    --admin "$(stellar keys address default)" \
    --token "$NATIVE_XLM_SAC"

echo "=== Deployment and initialization complete ==="
