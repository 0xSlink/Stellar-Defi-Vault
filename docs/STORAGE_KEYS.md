# Storage key registry

This document lists every storage key used by each module. It exists to make
cross-module collisions visible at review time and to satisfy the
`DataKey sub-enum refactor` requirement from issue #848.

## Rules

1. **No two variants may produce the same XDR discriminant.** For
   `#[contracttype]` enums, the XDR discriminant is the variant's index in
   declaration order. Adding a new variant at the *start* of the enum shifts
   all existing indices — always append at the end.

2. **symbol_short! strings must be ≤ 9 ASCII bytes.** Soroban silently
   truncates longer strings in XDR. The
   `symbol_short_strings_do_not_exceed_nine_bytes` test in `src/test.rs`
   enforces this.

3. **Each module owns its key enum.** `VaultContract` uses `DataKey`;
   `StakeReceiptNFT` uses `NftDataKey`. Their storage namespaces are separate
   at runtime (different contract instances), but the registry here makes the
   split explicit.

---

## `DataKey` — `src/storage.rs` — used by `VaultContract`

| Variant | Storage tier | Notes |
|---|---|---|
| `Admin` | instance | Contract admin address |
| `Token` | instance | Stake/reward token address |
| `TotalShares` | instance | Aggregate minted shares |
| `TotalDeposited` | instance | Aggregate deposited tokens |
| `MinStake` | instance | Minimum stake per user |
| `RewardRateBps` | instance | Annual reward rate in bps |
| `RewardPoolBalance` | instance | Tokens reserved for rewards |
| `BoostSchedule` | instance | Tier-boost schedule |
| `ShareBalance(Address)` | persistent | Per-user share balance |
| `StakeHistory(Address)` | persistent | Per-user stake/unstake log |
| `RewardCheckpointLedger(Address)` | persistent | Last reward-accrual ledger |
| `LastClaimLedger(Address)` | persistent | Last claim ledger |
| `AccruedReward(Address)` | persistent | Unclaimed accrued reward |
| `Paused` | instance | Pool pause flag |
| `WithdrawalLimit` | instance | Max withdrawal per tx |
| `LockPeriod` | instance | Ledgers position is locked |
| `EarlyExitPenaltyBps` | instance | Penalty bps on early exit |
| `StakedAtLedger(Address)` | persistent | Ledger position was opened |
| `TotalStakers` | instance | Count of active stakers |
| `TotalRewardsPaid` | instance | Cumulative rewards paid |
| `Delegate(Address)` | persistent | Voting delegate |
| `LastUnstakeLedger(Address)` | persistent | Last unstake ledger |
| `TotalEverClaimed` | instance | Lifetime claimed rewards |
| `Restaked(Address)` | persistent | Auto-restake flag |
| `WhitelistEnabled` | instance | Whitelist gate flag |
| `Whitelisted(Address)` | persistent | Per-user whitelist entry |
| `CooldownPeriod` | instance | Ledgers between unstake and withdrawal |
| `UnbondingPosition(Address)` | persistent | In-flight unbonding |
| `RewardRemainder(Address)` | persistent | Sub-unit reward carry |
| `UserClaimWindow(Address)` | persistent | Claim-rate-limit window |
| `UnstakeFeeBps` | instance | Static unstake fee |
| `AllStakers` | instance | Vec of all staker addresses |
| `RateHistory` | instance | Vec of rate-change entries |
| `BoostCampaign` | instance | Active campaign info |
| `Leaderboard` | instance | Top-staker leaderboard |
| `LeaderboardSize` | instance | Max leaderboard entries |
| `InactivityThreshold` | instance | Ledgers before freeze |
| `FrozenAt(Address)` | persistent | Ledger a position was frozen |
| `KycRequired` | instance | KYC gate flag |
| `KycApproved(Address)` | persistent | Per-user KYC approval |
| `Stopped` | instance | Emergency-stop flag |
| `ShuttingDown` | instance | Graceful-shutdown flag |
| `FirstStakedAt(Address)` | persistent | First-ever stake ledger |
| `VestingPeriod` | instance | Global vesting cliff (ledgers) |
| `VestingEntries(Address)` | persistent | Per-user vesting schedule |
| `EpochMode` | instance | Epoch mode enabled flag |
| `CurrentEpoch` | instance | Active epoch number |
| `EpochLedgers` | instance | Ledgers per epoch |
| `EpochRewardPerEpoch` | instance | Reward tokens per epoch |
| `EpochRewardFactor(u32)` | persistent | Per-epoch reward factor |

---

## `NftDataKey` — `src/nft.rs` — used by `StakeReceiptNFT`

| Variant | Storage tier | Notes |
|---|---|---|
| `Minter` | instance | Address allowed to mint/burn |
| `Receipt(Address)` | persistent | Per-user stake receipt |

---

## Adding a new key

1. Append the variant at the **end** of the relevant enum in `src/storage.rs`
   (or `src/nft.rs`).
2. Add a matching label to `all_data_key_labels()` (or
   `all_nft_data_key_labels()`) in `src/test.rs`.
3. Add a row to this table.
4. If the key uses a `symbol_short!` string, add it to the
   `symbol_short_strings_do_not_exceed_nine_bytes` test.
5. Run `cargo test --features testutils storage_keys` to confirm no collisions.
