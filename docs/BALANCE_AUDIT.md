# Audit Report: Accounting State vs. Raw Token Balances (Issue #862)

## Executive Summary
This audit confirms that the Stellar DeFi Vault contract accounting logic strictly relies on internal state counters (`total_deposited`, `total_shares`, `accumulator`, and per-user positions) rather than querying raw token balances via `token::Client::balance()`.

Direct transfers of underlying tokens or reward tokens to the contract address cannot manipulate share prices, dilute depositors, disrupt reward accrual, or bypass configured deposit/withdrawal caps.

---

## Codebase Audit Inventory

| File | Context | Query Target | Risk Assessment | Justification |
| :--- | :--- | :--- | :--- | :--- |
| `src/solvency_report.rs` | `get_solvency_report()` | `contract_address` | Informational | **Permitted**. Reads raw balance solely to compute surplus or deficit relative to internal accounting liabilities. |
| `src/foreign_token_sweep.rs` | `sweep_foreign_tokens()` | `contract_address` | Safe | **Permitted**. Allows rescue of non-vault tokens mistakenly transferred to the vault contract. Explicitly forbids sweeping vault stake/reward tokens. |
| `src/vault.rs` | `preview_deposit()`, `convert_to_shares()` | None (uses `balance::get_total_deposited()`) | None | Correct. Calculated using purely internal ledger state. |
| `src/vault.rs` | `stake()`, `deposit()` | None (uses internal addition) | None | Correct. Internal counters incremented only by tokens explicitly pulled via `transfer_from`. |

---

## Verifications & Test Coverage
Automated test `test_direct_transfer_does_not_alter_accounting` verifies:
1. An attacker transferring tokens directly to the vault contract does not alter `preview_deposit()`, `total_deposited`, or share pricing.
2. Internal accounting counters are completely isolated from unrequested balances.
