#!/usr/bin/env bash
set -euo pipefail

# 1. Check for stellar CLI
if ! command -v stellar >/dev/null 2>&1; then
    echo "Error: stellar CLI is not installed or not on PATH."
    echo "Please install it by running:"
    echo "  cargo install --locked stellar-cli"
    exit 1
fi

# 2. Get configuration
# Use IDENTITY from env, default to "deployer"
IDENTITY="${IDENTITY:-deployer}"
NETWORK="${NETWORK:-testnet}"

echo "Using identity: $IDENTITY"
echo "Using network: $NETWORK"

# Ensure the identity exists, if not generate it
if ! stellar keys address "$IDENTITY" >/dev/null 2>&1; then
    echo "Identity '$IDENTITY' not found. Generating a new one..."
    stellar keys generate --network "$NETWORK" "$IDENTITY"
fi

IDENTITY_ADDRESS=$(stellar keys address "$IDENTITY")
echo "Identity address: $IDENTITY_ADDRESS"

# Fund the identity if on testnet and balance is low/zero
if [ "$NETWORK" = "testnet" ]; then
    echo "Funding identity on testnet using Friendbot..."
    curl -s "https://friendbot.stellar.org?addr=$IDENTITY_ADDRESS" > /dev/null || true
fi

# 3. Build contract
echo "Building contract WASM..."
stellar contract build --optimize

WASM_PATH="target/wasm32v1-none/release/stellar_defi_vault.wasm"

if [ ! -f "$WASM_PATH" ]; then
    echo "Error: WASM file not found at $WASM_PATH"
    exit 1
fi

# 4. Get token address
# Use TOKEN_ID from env, or get native XLM wrapper if not specified
if [ -z "${TOKEN_ID:-}" ]; then
    echo "No TOKEN_ID specified. Fetching native XLM wrapper contract ID..."
    TOKEN_ID=$(stellar contract id asset \
        --asset native \
        --network "$NETWORK")
fi
echo "Using token address: $TOKEN_ID"

# 5. Deploy contract with constructor arguments (atomic deploy & init, preventing front-running)
echo "Deploying and initializing contract atomically to $NETWORK..."
CONTRACT_ID=$(stellar contract deploy \
    --wasm "$WASM_PATH" \
    --source "$IDENTITY" \
    --network "$NETWORK" \
    -- \
    --admin "$IDENTITY_ADDRESS" \
    --token "$TOKEN_ID")

echo "Contract deployed and initialized atomically! ID: $CONTRACT_ID"
echo "----------------------------------------"
echo "Vault Contract ID: $CONTRACT_ID"
echo "Token Contract ID: $TOKEN_ID"
echo "Admin Address:     $IDENTITY_ADDRESS"
echo "----------------------------------------"
echo "Save these to your .env file:"
echo "CONTRACT_ID=$CONTRACT_ID"
echo "TOKEN_ID=$TOKEN_ID"
echo "IDENTITY=$IDENTITY"
echo "NETWORK=$NETWORK"

# 6. Deploy Testnet Faucet Contract (issue #891)
echo "Building and deploying test faucet contract..."
FAUCET_WASM="target/wasm32v1-none/release/test_faucet.wasm"
if [ -d "contracts/test-faucet" ]; then
    (cd contracts/test-faucet && stellar contract build --optimize)
    if [ -f "$FAUCET_WASM" ]; then
        FAUCET_ID=$(stellar contract deploy \
            --wasm "$FAUCET_WASM" \
            --source "$IDENTITY" \
            --network "$NETWORK")
        echo "Initializing faucet contract..."
        stellar contract invoke \
            --id "$FAUCET_ID" \
            --source "$IDENTITY" \
            --network "$NETWORK" \
            -- initialize \
            --admin "$IDENTITY_ADDRESS" \
            --token "$TOKEN_ID" \
            --claim_amount 10000000000 \
            --cooldown_ledgers 17280
        echo "Faucet Contract ID: $FAUCET_ID"
        echo "FAUCET_ID=$FAUCET_ID" >> .env
    fi
fi

# 7. Run post-deploy invariant sweep (issue #950)
if command -v npx >/dev/null 2>&1; then
    echo "Running post-deploy invariant sweep..."
    npx tsx scripts/invariant-sweep.ts --contract-id "$CONTRACT_ID"
else
    echo "Warning: npx not found; run 'npx tsx scripts/invariant-sweep.ts --contract-id $CONTRACT_ID' manually."
fi

