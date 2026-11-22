# Testnet Token Faucet Contract

A rate-limited token faucet for testnet and local development environments.

## Overview
- **Rate-Limited Claims**: Each address may claim up to a configurable amount per cooldown period (defaults to 100 tokens per 17,280 ledgers / ~24 hours).
- **Safety Gate**: Contains a compile-time assertion preventing it from ever being built or deployed when the `mainnet` cargo feature is enabled.

## Deployment & Usage

### Deploying via Testnet Script
The faucet is deployed alongside the vault via `scripts/deploy_testnet.sh`:
```bash
./scripts/deploy_testnet.sh
```

### Claiming Tokens
```bash
stellar contract invoke \
  --id <FAUCET_CONTRACT_ID> \
  --source <TESTER_IDENTITY> \
  --network testnet \
  -- claim \
  --claimant <TESTER_ADDRESS>
```
