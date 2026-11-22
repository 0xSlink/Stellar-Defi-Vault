# Architecture

## Feature interaction / gating matrix (issue #786)

Several independently-added features gate the same four entrypoints —
deposit, withdraw, claim, and a full exit (`unstake_all`, which shares
`withdraw`'s gating exactly since both bottom out in `do_unstake`). This
table is the single source of truth for which gate blocks which entrypoint;
it is kept in sync with `src/test_feature_gating_matrix.rs`, whose
`all_combinations_match_table` test replays every combination of the rows
below against a real contract instance and asserts the result matches this
table.

| Dimension                | Deposit                      | Withdraw                              | Claim   | Exit                                   |
|---------------------------|-------------------------------|-----------------------------------------|---------|------------------------------------------|
| Withdrawals paused (`pause_withdrawals`) | Allowed         | Blocked (`VaultPaused`)                | Allowed | Blocked (`VaultPaused`)                |
| Withdraw-only mode        | Blocked (`PoolShuttingDown`) | Allowed                                | Allowed | Allowed                                 |
| Position locked (`lock_position`) | Allowed             | Blocked (`UseCooldownFlow`)            | Allowed | Blocked (`UseCooldownFlow`)             |
| Daily withdrawal limit exceeded | Allowed              | Blocked (`WithdrawalLimitExceeded`)    | Allowed | Blocked (`WithdrawalLimitExceeded`)    |
| Blocklist (`flag_frozen`/`is_frozen`) | Allowed *(gap)*  | Allowed *(gap)*                        | Allowed *(gap)* | Allowed *(gap)*                 |
| Cooldown (`withdrawal_cooldown.rs`, claim cooldown in `allowlist_rate_limits.rs`) | Allowed *(gap)* | Allowed *(gap)* | Allowed *(gap)* | Allowed *(gap)* |

A cell's outcome applies independently of the other rows: a combination
where multiple dimensions are simultaneously active is blocked if *any*
active dimension blocks that column.

### `(gap)` cells

Two rows are marked `(gap)` across every column — not because those
features don't exist, but because tracing `deposit()`, `withdraw()`,
`claim()`, and `unstake_all()` directly in `vault.rs` found no call to
either's check function from any of the four entrypoints:

- **Blocklist.** `flag_frozen()` / `is_frozen()` / `unfreeze()` fully
  implement storage for a per-user frozen flag, but nothing reads
  `is_frozen()` before a deposit, withdrawal, claim, or exit.
- **Cooldown.** `withdrawal_cooldown.rs`'s configured cooldown and the claim
  cooldown in `allowlist_rate_limits.rs` are likewise never read from any of
  the four entrypoints.

A related, narrower gap: `pause_deposits()` (the deposit-side counterpart to
`pause_withdrawals()`, in `vault_extensions_490_493.rs`) *is* checked, but
only by `do_stake_inner` (used by `stake_and_claim`) — not by `stake()`
itself, which is what `deposit()` actually calls. So `pause_deposits()` has
no effect on the primary deposit path either, even though it's a real,
wired check on a different one.

These are documented here deliberately rather than silently treated as
"working because the feature exists" — fixing them is real follow-up work
that this table and its test now make concrete, rather than something to
quietly paper over while adding the matrix.
