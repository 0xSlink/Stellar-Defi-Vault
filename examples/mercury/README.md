# Mercury Indexer Integration for Stellar DeFi Vault

This example demonstrates how to subscribe to and index Stellar DeFi Vault events using the [Mercury](https://mercurydata.app/) indexing service.

## Architecture
1. **Subscription Registration**: Webhook subscription configured via `subscription.ts`.
2. **Event Schema Normalization**: Raw payload parsing into typed records in `schema-mapping.ts` matching `docs/EVENTS.md`.
3. **Query Interface**: GraphQL query examples in `sample-query.ts`.
4. **Resilience & Fallback**: Automatic RPC polling fallback (`fallback-poller.ts`) if subscription quotas or rate limits are reached.

## Setup & Running
```bash
cd examples/mercury
npm install
npm start
```
