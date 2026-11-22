# Fee-Bump Sponsorship for User Transactions

Many Stellar users hold stake tokens (such as USDC or wrapped assets) without holding native XLM to pay transaction and Soroban resource fees. Sponsoring those fees lets users deposit, claim, and withdraw without first funding an XLM balance.

## Fee-Bump vs. Source-Account Sponsorship

Stellar supports two primary patterns for paying fees on behalf of a user:

| Dimension | Fee-Bump Transaction (`buildFeeBumpTransaction`) | Source-Account Sponsorship (Sponsor as Outer `sourceAccount`) |
|---|---|---|
| **How it works** | User builds and signs an inner `Transaction` (with user as `sourceAccount`). Sponsor wraps it in a `FeeBumpTransaction`, sets `maxFee`, and signs the outer envelope. | Sponsor builds the `Transaction` with the sponsor as `sourceAccount` and pays the fee directly. User signs only the Soroban authorization entry (`require_auth`). |
| **User XLM requirement** | User account must exist on-chain (or have a valid sequence number), but **0 XLM** is deducted from the user. | User does not even need a classic account sequence number (works seamlessly for contract/passkey accounts). |
| **Sequence number contention** | Uses the **user's** sequence number, so concurrent users do not contend on the sponsor's sequence number. | Uses the **sponsor's** sequence number; requires a pool of sponsor channel accounts under high concurrency. |
| **Fee predictability** | Sponsor explicitly caps `maxFee` on the outer fee-bump envelope after simulating the inner tx. | Sponsor sets `fee` on the transaction directly after simulation. |
| **Contract compatibility** | Works with any entrypoint without changing how `require_auth` is structured. | Requires the user to sign Soroban `SorobanAuthorizationEntry` payloads rather than a full transaction envelope. |

> **Note on Reserve Sponsorship:** Stellar's `beginSponsoringFutureReserves` / `endSponsoringFutureReserves` operations sponsor **base reserves** for new ledger entries (such as accounts or trustlines), not transaction fees. Use a **fee-bump transaction** (or sponsor source account) for Soroban execution and inclusion fees.

## Example Script: Wrapping a User-Signed Deposit

[`scripts/fee-bump-deposit.ts`](../scripts/fee-bump-deposit.ts) demonstrates the end-to-end flow:

1. **User builds and signs the inner transaction**:
   - Calls `VaultContract::deposit(user, amount)` with `user` as the inner `sourceAccount`.
   - Prepares the transaction via Soroban RPC (`server.prepareTransaction`) so footprint and resource fees are populated, then signs with the user's keypair.
2. **Sponsor validates and wraps in a fee-bump**:
   - Verifies the inner transaction targets the expected `CONTRACT_ID` and an allowlisted method (`deposit`), and checks that the simulated resource fee does not exceed `MAX_SPONSOR_FEE`.
   - Wraps the signed inner transaction with `TransactionBuilder.buildFeeBumpTransaction(sponsor.publicKey(), maxFee, innerTx, networkPassphrase)` and signs with the sponsor's keypair.
3. **Submission**:
   - Submits the `FeeBumpTransaction` via `server.sendTransaction` and polls `getTransaction` until confirmed.

```bash
CONTRACT_ID=C... USER_SECRET=S... SPONSOR_SECRET=S... AMOUNT=10000000 \
  npx tsx scripts/fee-bump-deposit.ts
```

## Example Frontend: Optional Sponsorship Mode

The example frontend in [`examples/frontend/`](../examples/frontend/README.md) includes an optional **Fee-Bump Sponsorship Mode** toggle:

- **Standard mode (`sponsorshipMode: "none"`)**: The user signs and submits the deposit transaction directly and pays XLM fees from their own wallet.
- **Sponsored mode (`sponsorshipMode: "fee-bump"`)**: The user signs the inner deposit transaction with `fee: "0"` (or simulated fee) and sends the signed inner XDR to the configured sponsor relayer endpoint, which wraps it in a `FeeBumpTransaction` and submits it to RPC.

## Abuse Considerations and Rate Limiting for Sponsors

Because the sponsor pays XLM for **every** submitted transaction—including transactions that revert on-chain—an unguarded sponsorship endpoint is vulnerable to fee-draining attacks. Sponsors should enforce the following controls before signing any fee-bump:

1. **Strict Target & Method Allowlist**:
   - Inspect the inner transaction operations and reject anything that is not an `invokeHostFunction` targeting the exact deployed `CONTRACT_ID` and an approved method (`deposit`, `claim_rewards`, `withdraw`).
2. **Pre-Flight Simulation & Fee Ceiling**:
   - Simulate the inner transaction (or verify its resource footprint) before signing the fee-bump.
   - Reject transactions whose required `maxFee` exceeds a hard per-tx cap (e.g., `1_000_000` stroops = 0.1 XLM) or whose simulation fails.
3. **Per-User and Per-IP Rate Limiting**:
   - Enforce a sliding-window or token-bucket limit per user address (e.g., max 5 sponsored transactions per hour, 20 per day) and per client IP.
   - Maintain a global daily XLM fee budget on the sponsor account with automatic circuit-breaker shutoff if spent XLM exceeds the budget.
4. **Minimum Economic Thresholds**:
   - Require a minimum deposit/action amount (e.g., `>= min_stake`) so attackers cannot spam zero-value or dust transactions at the sponsor's expense.
5. **Short Transaction Timeouts**:
   - Require `timebounds.maxTime` on the inner transaction to be within a short window (e.g., 60–120 seconds) to prevent stale signed inner transactions from being replayed or held.
6. **Channel Account Isolation**:
   - Keep the sponsor's fee-paying keypair separate from any vault `admin`, `guardian`, or `upgrader` keypair, and hold only a bounded operational XLM float in the sponsor account.
