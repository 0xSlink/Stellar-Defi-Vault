# Threat-Model-to-Test Traceability Matrix

This matrix maps the threats documented in [docs/THREAT_MODEL.md](./THREAT_MODEL.md), [docs/SECURITY.md](./SECURITY.md), and [docs/AUDIT_READINESS.md](./AUDIT_READINESS.md) to their on-chain mitigations and automated test coverage.

Every test reference in the **Test File & Name** column uses the format `` `path/to/file.rs::test_fn_name` `` and is verified by [`scripts/check-traceability.py`](../scripts/check-traceability.py) in CI.

## Traceability Matrix

| ID | Threat | Mitigation | Test File & Name | Status |
| --- | --- | --- | --- | --- |
| TM-01 | Contract re-initialization / admin overwrite | `initialize()` checks `DataKey::Admin` and returns `VaultError::AlreadyInitialized` | `src/test.rs::test_double_initialize_fails` | Covered |
| TM-02 | Unauthorized admin actions (pause, yield injection, token rescue, admin transfer) | `admin::require_admin` enforces `admin.require_auth()` on all privileged entrypoints | `src/test.rs::test_pause_requires_admin_auth`, `src/test.rs::test_add_yield_requires_admin_auth`, `src/test.rs::test_rescue_token_requires_admin_auth`, `src/test.rs::test_transfer_admin_requires_admin_auth`, `src/test_admin_contract_auth.rs::rejecting_contract_account_blocks_admin_entrypoint` | Covered |
| TM-03 | Self-referential admin / emergency admin lockout | `transfer_admin` and `set_emergency_admin` reject the vault's own contract address | `src/test_self_referential_admin.rs::test_transfer_admin_to_self_is_blocked`, `src/test_self_referential_admin.rs::test_set_emergency_admin_to_self_is_blocked` | Covered |
| TM-04 | Emergency pause / freeze safety | `pause()` halts deposits and withdrawals cleanly without mutating user share balances | `src/test.rs::test_pause_blocks_deposit`, `src/test.rs::test_pause_blocks_withdraw` | Covered |
| TM-05 | Graceful shutdown trapping existing stakers | `start_graceful_shutdown()` blocks new stakes while allowing existing stakers to exit | `src/test.rs::test_graceful_shutdown_blocks_new_stakes`, `src/test.rs::test_graceful_shutdown_existing_stakers_can_exit` | Covered |
| TM-06 | First-depositor / share-inflation attack | Virtual share/asset offset prevents 1-wei donation share-price manipulation | `src/test_issues_838_860_861_862.rs::test_inflation_attack_protection_with_virtual_offset` | Covered |
| TM-07 | Negative balance / underflow corruption in storage setters | Balance setters reject negative `i128` values before writing to storage | `src/test_issue_621_negative_balance.rs::set_shares_rejects_negative_value`, `src/test_issue_621_negative_balance.rs::set_total_deposited_rejects_negative_value` | Covered |
| TM-08 | Principal vs. reward pool commingling / reward over-claim | Principal and reward pool are tracked separately; claims exceeding reward pool revert | `src/test_issues_863_864_865_867.rs::test_principal_and_reward_pool_isolation`, `src/test_issues_863_864_865_867.rs::test_claims_revert_when_exceeding_reward_pool`, `src/test_solvency_report.rs::principal_and_reward_funds_are_reported_separately` | Covered |
| TM-09 | User operation replay / out-of-order nonce execution | Sequential per-user nonce enforcement rejects replayed or skipped nonces | `src/test_issues_863_864_865_867.rs::test_user_nonce_replay_reverts` | Covered |
| TM-10 | MEV / front-running on large reward claims | Large claims are queued with a ledger delay before execution | `src/test_mev_claim_protection.rs::large_claim_is_queued_with_delay_in_range`, `src/test_mev_claim_protection.rs::execute_before_delay_reverts` | Covered |
| TM-11 | SAC `AUTH_CLAWBACK_ENABLED` silent insolvency deficit | `set_token_risk_flags()` records issuer risk; `get_solvency_report()` detects balance deficits | `src/test_issues_863_864_865_867.rs::test_token_risk_flags_configuration`, `src/test_solvency_report.rs::report_shows_deficit_when_assets_fall_short` | Covered |
| TM-12 | Native XLM SAC compatibility | Native XLM SAC integration verified end-to-end | `src/test_issues_863_864_865_867.rs::test_native_xlm_sac_integration` | Covered |
| TM-13 | Blocked user funds permanently trapped on blocklist | Blocked users can still exit to their own address | `src/test_exit.rs::blocked_user_can_exit_to_their_own_address_only` | Covered |
| TM-14 | Contract upgrade corrupting storage or invariants | Upgrade tests verify share balances and invariants are preserved across WASM upgrades | `src/test_issues_770_upgrade.rs::test_upgrade_preserves_share_balances`, `src/test_issues_770_upgrade.rs::test_upgrade_preserves_invariants` | Covered |
| TM-15 | SAC `AUTH_REVOCABLE` issuer freeze during active transfer | Transaction atomicity reverts failed transfers cleanly without corrupting internal vault state | — | Open |
| TM-16 | SAC `AUTH_REQUIRED` missing vault trustline pre-authorization | Vault address must be pre-authorized by issuer before accepting deposits | — | Open |
| TM-17 | Single-step `transfer_admin` to an unreachable address | Documented operational verification before calling; no on-chain recovery path | — | Open |
| TM-18 | Single-admin key compromise pausing withdrawals without timelock | Mitigated operationally by hardware/multisig admin key and rotation procedure | — | Open |

---

## Open Threats & Follow-Up Issues

Uncovered or accepted-risk threats listed with `Open` status above require either dedicated test coverage or governance tracking:

- **TM-15 (SAC `AUTH_REVOCABLE` freeze simulation):** Add a mock SAC test where the issuer revokes/freezes the vault's authorization mid-lifecycle and asserts `deposit`/`withdraw` revert with internal counters unchanged. (Open gap — track as a test-coverage issue.)
- **TM-16 (SAC `AUTH_REQUIRED` un-authorized trustline test):** Add a test verifying `deposit` fails atomically when the vault contract lacks issuer authorization. (Open gap — track as a test-coverage issue.)
- **TM-17 (Single-step `transfer_admin` recovery):** Accepted design trade-off in [docs/AUDIT_READINESS.md § Known Issues](./AUDIT_READINESS.md#known-issues); consider enforcing two-step handoff (`src/admin_succession.rs` / ADR 0005) as default in a future major release. (Open governance issue.)
- **TM-18 (Single-admin key compromise / pause timelock):** Accepted deployment trust model in [docs/SECURITY.md](./SECURITY.md); track multisig/timelock governance transition prior to mainnet launch. (Open governance issue.)
