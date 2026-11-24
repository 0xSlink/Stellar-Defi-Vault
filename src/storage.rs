/// Maximum admin-configurable rate-limit window stored in temporary state.
/// Kept below Soroban's protocol maximum entry TTL; used where configuration
/// can be lengthened after a stamp or usage record is written.
pub(crate) const MAX_TEMPORARY_RATE_LIMIT_TTL: u32 = 5_000_000;

use soroban_sdk::{contracttype, Address, String, Vec, Env, Symbol, symbol_short};

/// Consolidated per-user settings (issue #796).
///
/// Stores all user preferences in a single storage entry to reduce rent costs.
/// Uses versioned-enum pattern so new settings can be added later.
///
/// - `version`: struct version for future migrations
/// - `claim_destination`: address to receive claimed rewards (None = self)
/// - `auto_compound`: whether to automatically compound rewards
/// - `keepers`: list of addresses authorized to act on behalf of user
/// - `notifications_enabled`: whether user wants notifications
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct UserSettings {
    pub version: u32,
    pub claim_destination: Option<Address>,
    pub auto_compound: bool,
    pub keepers: Vec<Address>,
    pub notifications_enabled: bool,
}

impl UserSettings {
    /// Create default settings for a new user.
    pub fn default() -> Self {
        Self {
            version: 1,
            claim_destination: None,
            auto_compound: false,
            keepers: Vec::new(&soroban_sdk::Env::default()),
            notifications_enabled: true,
        }
    }
}

/// Storage keys for all persistent and instance state in the vault.
///
/// Instance keys (fast, small): Admin, Token, TotalShares, TotalDeposited,
/// MinStake, RewardRateBps, RewardPoolBalance, BoostSchedule, Paused,
/// WithdrawalLimit, LockPeriod, EarlyExitPenaltyBps, TotalStakers,
/// TotalRewardsPaid, WhitelistEnabled, CooldownPeriod,
/// UnstakeFeeBps, AllStakers, InactivityThreshold, Changelog,
/// LastRateChangeLedger, InitializedAtLedger.
///
/// Persistent keys (per-user, long-lived): ShareBalance, StakeHistory,
/// RewardCheckpointLedger, LastClaimLedger, AccruedReward, StakedAtLedger,
/// Delegate, Whitelisted, UnbondingPosition, UserClaimWindow, FrozenAt,
/// VestingEntries.
/// Domain sub-enum for administrator and protocol-wide configuration keys (Issue #646).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdminKey {
    Admin,
    PendingAdmin,
    EmergencyAdmin,
    AdminRenounced,
    Deployer,
    SlashTreasury,
    Token,
    TotalShares,
    TotalDeposited,
    MinStake,
    Paused,
    WithdrawalLimit,
    LockPeriod,
    EarlyExitPenaltyBps,
    TotalStakers,
    CooldownPeriod,
    UnstakeFeeBps,
}

/// Domain sub-enum for staker positions, delegation, and user-specific accounting (Issue #646).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PositionKey {
    ShareBalance(Address),
    StakeHistory(Address),
    StakedAtLedger(Address),
    LastUnstakeLedger(Address),
    UnbondingPosition(Address),
    FrozenAt(Address),
    FirstStakedAt(Address),
    Restaked(Address),
    Delegate(Address),
    RewardCheckpointLedger(Address),
    LastClaimLedger(Address),
    AccruedReward(Address),
    RewardRemainder(Address),
    UserClaimWindow(Address),
    VestingEntries(Address),
    RewardAccumulatorCheckpoint(Address),
}

/// Domain sub-enum for reward distribution, emissions, and boost campaigns (Issue #646).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RewardKey {
    RewardRateBps,
    RewardPoolBalance,
    RewardToken,
    TotalRewardsPaid,
    TotalEverClaimed,
    RateHistory,
    BoostSchedule,
    BoostCampaign,
    VestingPeriod,
    EpochMode,
    CurrentEpoch,
    EpochLedgers,
    EpochRewardPerEpoch,
    EpochRewardFactor(u32),
    GlobalRewardIndex,
    LastAccumulatorLedger,
}

/// Domain sub-enum for governance, access control, and pool lifecycle (Issue #646).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GovernanceKey {
    WhitelistEnabled,
    Whitelisted(Address),
    AllStakers,
    Leaderboard,
    LeaderboardSize,
    InactivityThreshold,
    KycRequired,
    KycApproved(Address),
    Stopped,
    ShuttingDown,
}

/// Top-level key enum wrapping namespaced domain sub-enums (Issue #646).
///
/// This removes Soroban's 50-variant limit on a single `#[contracttype]` enum
/// by dividing keys into domain-specific sub-enums while preserving type safety.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageKey {
    Admin(AdminKey),
    Position(PositionKey),
    Reward(RewardKey),
    Governance(GovernanceKey),
}

/// Storage keys for all persistent and instance state in the vault.
///
/// Instance keys (fast, small): Admin, Token, TotalShares, TotalDeposited,
/// MinStake, RewardRateBps, RewardPoolBalance, BoostSchedule, Paused,
/// WithdrawalLimit, LockPeriod, EarlyExitPenaltyBps, TotalStakers,
/// TotalRewardsPaid, SlashTreasury, WhitelistEnabled, CooldownPeriod,
/// PoolCap, ClaimCap, ClaimCapWindow, StakeDecimals, RewardDecimals,
/// UnstakeFeeBps, AllStakers, InactivityThreshold, Changelog,
/// LastRateChangeLedger, InitializedAtLedger.
///
/// Persistent keys (per-user, long-lived): ShareBalance, StakeHistory,
/// RewardCheckpointLedger, LastClaimLedger, AccruedReward, StakedAtLedger,
/// Delegate, Whitelisted, UnbondingPosition, UserClaimWindow, FrozenAt,
/// VestingEntries.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    TotalShares,
    TotalDeposited,
    MinStake,
    RewardRateBps,
    RewardPoolBalance,
    BoostSchedule,
    ShareBalance(Address),
    StakeHistory(Address),
    RewardCheckpointLedger(Address),
    LastClaimLedger(Address),
    AccruedReward(Address),
    Paused,
    WithdrawalLimit,
    LockPeriod,
    EarlyExitPenaltyBps,
    StakedAtLedger(Address),
    TotalStakers,
    TotalRewardsPaid,
    Delegate(Address),
    LastUnstakeLedger(Address),
    TotalEverClaimed,
    Restaked(Address),
    WhitelistEnabled,
    Whitelisted(Address),
    CooldownPeriod,
    UnbondingPosition(Address),
    RewardRemainder(Address),
    UserClaimWindow(Address),
    UnstakeFeeBps,
    AllStakers,
    RateHistory,
    BoostCampaign,
    Leaderboard,
    LeaderboardSize,
    // Issue #101: frozen positions
    InactivityThreshold,
    FrozenAt(Address),
    KycRequired,
    KycApproved(Address),
    Stopped,
    // Pool deposit cap (used by balance.rs and vault.rs)
    PoolCap,
    // Issue #939: on-chain upgrade history
    UpgradeHistory(u32),
    // Issue #940: successor pointer for deprecated pools
    SuccessorPool(Address),
}

/// Issue #42: enum of all admin actions for the audit log.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum AdminAction {
    SetRewardRate,
    Pause,
    Unpause,
    TransferAdmin,
    SetLockPeriod,
    SetCap,
    Slash,
    RescueToken,
    SetEarlyExitPenalty,
    SetMinStake,
    FundRewardPool,
    AddYield,
    SetBoostSchedule,
    SetNftContract,
    SetRestakeWindow,
    SetRewardToken,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct UnbondingPosition {
    pub amount: i128,
    pub unbonding_since: u32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VaultState {
    pub total_shares: i128,
    pub total_deposited: i128,
    pub paused: bool,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolStats {
    pub total_staked: i128,
    pub total_stakers: u32,
    pub reward_rate_bps: i128,
    pub reward_token_balance: i128,
    pub paused: bool,
    pub total_rewards_paid: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RewardTier {
    pub max_amount: i128,
    pub rate_bps: i128,
}

/// Aggregate user stats used by `user_stats`.
///
/// - `position_amount`: the user's current position size expressed in token units.
/// - `pending_reward`: rewards accrued but not yet claimed.
/// - `staked_at_ledger`: the ledger sequence when the position was first opened.
/// - `last_claim_ledger`: the most recent ledger at which rewards were claimed.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct UserStats {
    pub position_amount: i128,
    pub pending_reward: i128,
    pub staked_at_ledger: u32,
    pub last_claim_ledger: u32,
}

/// Matching program configuration (issue #242).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct MatchingProgram {
    pub match_rate_bps: u32,
    pub per_user_cap: i128,
    pub total_budget: i128,
    pub budget_used: i128,
    pub active: bool,
}

/// Per-user matching stats (issue #242).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct UserMatchingStats {
    pub total_matched: i128,
}

/// Cohort statistics for weekly staker cohorts.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CohortStats {
    pub cohort_id: u32,
    pub member_count: u32,
    pub total_staked: i128,
    pub avg_position: i128,
    pub total_rewards_claimed: i128,
    pub active_members: u32,
}

/// Active boost campaign set by admin (#48).
///
/// - `multiplier_bps`: reward multiplier stacked on top of tier multipliers (10000 = 1x).
/// - `starts_at_ledger`: ledger when the campaign was activated.
/// - `ends_at_ledger`: ledger after which the campaign no longer applies.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignInfo {
    pub multiplier_bps: u32,
    pub starts_at_ledger: u32,
    pub ends_at_ledger: u32,
}

/// A single entry in the staking leaderboard (#46).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct LeaderboardEntry {
    pub staker: Address,
    pub amount: i128,
}

/// Type alias for the leaderboard vector used in storage and queries.
#[allow(dead_code)]
pub type Leaderboard = Vec<LeaderboardEntry>;

/// Current stake position for a user.
///
/// - `amount`: the user's current position size expressed in token units.
/// - `staked_at_ledger`: the ledger sequence when the position was first opened.
/// - `last_claim_ledger`: the most recent ledger at which rewards were claimed.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct StakePosition {
    pub amount: i128,
    pub staked_at_ledger: u32,
    pub last_claim_ledger: u32,
}

/// Snapshot of all pool-level configuration returned by `get_pool_config`.
///
/// Allows frontends to fetch all settings in a single RPC call instead of
/// querying each key individually.
///
/// - `admin`: current admin address.
/// - `stake_token`: token accepted for staking and used to pay rewards.
/// - `reward_token`: same as `stake_token` (single-token vault).
/// - `reward_rate_bps`: annual reward rate in basis points.
/// - `paused`: whether deposits and withdrawals are currently paused.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolConfig {
    pub admin: Address,
    pub stake_token: Address,
    pub reward_token: Address,
    pub reward_rate_bps: u32,
    pub paused: bool,
}

/// Contract metadata returned by `contract_metadata`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ContractMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
}

/// Per-user reward claim window used to enforce the optional claim cap.
///
/// - `claimed_in_window`: cumulative rewards claimed by this user in the current window.
/// - `window_started_at`: ledger sequence at which the current window began.
///
/// The window resets automatically when `current_ledger > window_started_at + window_ledgers`.
/// Any unclaimed remainder is deferred to the next window — it is not lost.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimWindow {
    pub claimed_in_window: i128,
    pub window_started_at: u32,
}

/// Dynamic unstake fee model configured via `set_dynamic_fee_config` (issue #213).
///
/// Below `utilization_threshold_bps` pool utilization, the unstake fee is
/// `base_fee_bps`. Above it, the fee interpolates linearly up to
/// `max_fee_bps` at 100% utilization. While a config is set, `unstake` uses
/// this dynamic fee instead of the static fee from `set_unstake_fee_bps`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicFeeConfig {
    pub base_fee_bps: u32,
    pub max_fee_bps: u32,
    pub utilization_threshold_bps: u32,
}

/// Composite staker reputation score returned by `get_reputation_score` (issue #214).
///
/// Each sub-score is bounded to 2500 basis points; `total_score` is the sum,
/// bounded to 10 000 (100%). All zero for an address with no active position.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ReputationScore {
    pub duration_score: u32,
    pub consistency_score: u32,
    pub size_score: u32,
    pub streak_score: u32,
    pub total_score: u32,
}

/// Single entry in the on-chain changelog exposed by `get_changelog`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ChangelogEntry {
    pub change_type: String,
    pub old_value: i128,
    pub new_value: i128,
}

/// One entry in the on-chain reward-rate changelog exposed by
/// `get_rate_history` (issue #522).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RateChange {
    pub old_rate_bps: u32,
    pub new_rate_bps: u32,
    /// Ledger sequence number of the update, matching the timestamp carried by
    /// the `rate_changed` event.
    pub changed_at: u32,
}

/// One entry in the rich reward-rate history exposed by `get_reward_rate_history` (issue #124).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RateHistoryEntry {
    pub old_rate_bps: i128,
    pub new_rate_bps: i128,
    pub changed_at_ledger: u32,
    pub changed_by: Address,
}

/// Accumulated referral stats stored per referrer (issue #757).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ReferralStats {
    pub referee_count: u32,
    pub total_referred_deposits: i128,
    pub earned: i128,
    pub claimed: i128,
}

/// Receipt for a cast governance vote (issue #760).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VoteReceipt {
    pub support: bool,
    pub weight: i128,
    pub ledger: u32,
}

/// Vote-escrow style reward boost lock (issue #758).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VeBoostLock {
    pub amount: i128,
    pub unlock_ledger: u32,
    pub duration_ledgers: u32,
}

/// Result of `get_boost_tier_progress` showing where a user is toward the next boost tier.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct BoostTierProgress {
    pub current_tier: u32,
    pub current_multiplier_bps: i128,
    pub next_tier_in_ledgers: Option<u32>,
    pub next_multiplier_bps: Option<i128>,
}

/// One entry in the referral leaderboard returned by `referral_leaderboard`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ReferralLeaderboardEntry {
    pub referrer: Address,
    pub total_referred_stake: i128,
    pub referral_count: u32,
}

/// Comprehensive health snapshot of the pool returned by `pool_health_report`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PoolHealthReport {
    pub reward_token_balance: i128,
    pub total_staked: i128,
    pub total_stakers: u32,
    pub total_rewards_paid: i128,
    pub reward_rate_bps: i128,
    pub is_paused: bool,
    pub is_stopped: bool,
    pub uptime_ledgers: u32,
    pub estimated_daily_obligations: i128,
    pub is_solvent_7_days: bool,
}

/// Admin-only operational snapshot for monitoring and alerting dashboards.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct OperatorDashboard {
    pub pool_health: PoolHealthReport,
    pub staker_count: u32,
    pub inactive_staker_count: u32,
    pub pending_exit_queue_count: u32,
    pub total_ever_staked: i128,
    pub total_ever_claimed: i128,
    pub largest_position: i128,
    pub smallest_active_position: i128,
    pub sunset_state: SunsetState,
    pub open_governance_proposals: u32,
    pub reward_token_runway_days: u32,
}

/// Effective reward rate breakdown returned by `reward_multiplier_preview` (issue #181).
///
/// Each `*_multiplier_bps` field defaults to 10000 (1x) if that boost mechanism
/// is not active for the user. `effective_rate_bps` is the fully stacked rate.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RewardMultiplierBreakdown {
    pub base_rate_bps: i128,
    pub tier_multiplier_bps: i128,
    pub campaign_multiplier_bps: i128,
    pub effective_rate_bps: i128,
}

/// Aggregate score used by `staking_efficiency_score`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct StakingEfficiencyScore {
    pub total_claimed: i128,
    pub estimated_if_compounded: i128,
    pub efficiency_bps: i128,
}

/// Aggregated user state returned by `user_summary` (issue #103).
///
/// - `position`: 0 or 1 `StakePosition` entries; empty when user has no stake.
/// - `pending_reward`: rewards accrued but not yet claimed.
/// - `pool_share_bps`: user's share of the total pool in basis points (10000 = 100%).
///
/// Note: `position` uses `Vec<StakePosition>` (0-or-1 elements) because
/// `Option<ContractType>` is not supported in `#[contracttype]` structs in
/// soroban-sdk 21.x testutils mode.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct UserSummary {
    pub position: Vec<StakePosition>,
    pub pending_reward: i128,
    pub pool_share_bps: i128,
}

// ── Issue #105: stake/unstake history ────────────────────────────────────────

/// Discriminant for a stake history entry.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StakeAction {
    Stake,
    Unstake,
}

/// One entry in a user's recent staking activity log.
///
/// - `action`: whether the user staked or unstaked.
/// - `amount`: token amount involved (not shares).
/// - `ledger`: ledger sequence number at which the action was recorded.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct StakeHistoryEntry {
    pub action: StakeAction,
    pub amount: i128,
    pub ledger: u32,
}

// ── Issue #104: interface detection ──────────────────────────────────────────

/// Feature interface identifiers for `supports_interface`.
///
/// `Base` is always supported. All others are only true when the corresponding
/// feature is compiled into this deployment.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum InterfaceId {
    Base,
    Lockup,
    Whitelist,
    Compounding,
    EpochMode,
    VestingSchedule,
}

/// Result of a `can_unstake` pre-flight check (issue #98).
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UnstakeCheckResult {
    /// The unstake would succeed.
    Ok,
    /// The user has no active staking position.
    NoPosition,
    /// The user's position is smaller than the requested amount (in token units).
    InsufficientAmount,
    /// The pool is currently paused.
    PoolPaused,
    /// The lock-up period has not yet elapsed (early exit penalty would apply).
    StillLocked,
}

/// Per-user staking streak data (issue #99).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct StakeStreak {
    pub current_streak: u32,
    pub longest_streak: u32,
    pub last_active_wave: u32,
}

// ── Issue #114: on-chain changelog ───────────────────────────────────────────

/// One entry in the rolling admin configuration changelog (issue #114).
///
/// - `changed_by`: admin address that triggered the change.
/// - `change_type`: human-readable label, e.g. "rate_changed", "paused".
/// - `old_value`: previous numeric value; boolean states encode as 0/1.
/// - `new_value`: new numeric value after the change.
/// - `ledger`: ledger sequence at which the change was recorded.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ChangelogEntry {
    pub changed_by: Address,
    pub change_type: soroban_sdk::String,
    pub old_value: i128,
    pub new_value: i128,
    pub ledger: u32,
}

// ── Issue #116: per-user vesting entries ─────────────────────────────────────

/// One scheduled vesting entry for a user (issue #116).
///
/// - `amount`: token amount that vests at `vested_at_ledger`.
/// - `vested_at_ledger`: ledger sequence after which the entry can be claimed.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct VestingEntry {
    pub amount: i128,
    pub vested_at_ledger: u32,
}
