# Threat Model & Issuer Trust Analysis (Issue #864)

## Overview
Stellar Asset Contract (SAC) tokens represent classic Stellar assets bridged into the Soroban smart contract environment. Under Stellar protocol rules, asset issuers can configure specific asset flags that grant significant administrative control over balances and operations.

---

## Stellar Asset Flags & Issuer Powers

### 1. `AUTH_REQUIRED` (0x1)
- **Mechanism**: Users and contracts cannot hold or transact the asset without prior explicit authorization from the issuer.
- **Vault Impact**: The vault contract address must be pre-authorized with a trustline before accepting deposits.

### 2. `AUTH_REVOCABLE` (0x2)
- **Mechanism**: The asset issuer retains the unilateral authority to de-authorize (freeze) any account or contract holding the token.
- **Vault Impact**: If an issuer freezes the vault's contract address:
  - Token transfers will revert on-chain.
  - **Fail-Safe Guarantee**: Reverting transfers roll back the transaction cleanly. Internal vault state (`total_deposited`, `total_shares`, user positions) is never mutated or corrupted when an issuer freeze blocks a transfer.

### 3. `AUTH_CLAWBACK_ENABLED` (0x8)
- **Mechanism**: The issuer can claw back (burn) tokens from any account's balance without holder consent or signature.
- **Vault Impact**: Claws back tokens directly from the vault contract's raw balance. Because vault accounting relies on internal counters rather than `token.balance(contract)`, an unannounced clawback creates a solvency deficit identifiable via `get_solvency_report()`.

---

## Trust Model & Risk Mitigation

1. **Explicit Risk Acknowledgement**:
   Administrators configuring SAC tokens with revocation or clawback flags must configure and record `TokenRiskFlags` via `set_token_risk_flags()`.
2. **Transaction Atomicity**:
   All transfer failures cleanly abort operations, preventing partial state execution.
3. **Solvency Auditing**:
   Routine automated monitoring via `get_solvency_report()` detects external balance modifications.
