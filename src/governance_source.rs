//! Governance token distinct from the stake token.
//!
//! By default voting power comes from the caller's staked amount. For pools
//! whose stake asset should not carry governance rights, the admin can point
//! voting power at a separate governance token instead, or at the sum of both:
//!
//! | Mode    | Voting power                                   |
//! |---------|------------------------------------------------|
//! | `Stake` | staked token amount (default)                  |
//! | `Token` | governance-token balance                       |
//! | `Both`  | staked amount + governance-token balance       |
//!
//! `set_governance_source(admin, mode, token)` configures the source. `Token`
//! and `Both` require a governance token address; `Stake` clears it.
//!
//! # Snapshot per proposal
//!
//! The source (mode and token) in force when a proposal is created is
//! snapshotted for that proposal and used by `vote` / `batch_vote`, so
//! reconfiguring the source mid-vote never changes the rules of an open
//! proposal. A proposal created while no source is configured behaves as
//! `Stake`.
//!
//! # Limitation
//!
//! Stake weight is read from the bounded stake history by `vote_weight_at`, but
//! an external governance token has no on-chain balance history, so its
//! component is always the live balance. Pools that need flash-loan or
//! transfer-and-revote resistance for the token should pick a token that
//! checkpoints its own balances.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`.

use soroban_sdk::{
    contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, Symbol,
};

use crate::admin;
use crate::balance;
use crate::errors::VaultError;
use crate::vault::VaultContract;

/// Instance-storage key for the current governance source.
const SOURCE_KEY: Symbol = symbol_short!("gv_src");

/// Persistent-storage key prefix for a proposal's source snapshot, keyed by
/// `(PROPOSAL_SOURCE_KEY, proposal_id)`.
const PROPOSAL_SOURCE_KEY: Symbol = symbol_short!("gv_psrc");

/// Where voting power is read from.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum GovernanceSource {
    /// Weight = staked token amount. This is the default.
    Stake,
    /// Weight = governance-token balance.
    Token,
    /// Weight = staked token amount + governance-token balance.
    Both,
}

/// A governance source plus the governance token it reads, if any.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct GovernanceSourceConfig {
    /// Which balances count towards voting power.
    pub mode: GovernanceSource,
    /// Governance token; always `Some` for `Token` and `Both`.
    pub token: Option<Address>,
}

/// Errors for governance-source configuration and voting.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum GovernanceSourceError {
    /// Caller is not the stored admin.
    Unauthorized = 1,
    /// No admin is stored.
    NotInitialized = 2,
    /// `Token` / `Both` mode was requested without a governance token.
    TokenRequired = 3,
}

impl From<VaultError> for GovernanceSourceError {
    fn from(err: VaultError) -> Self {
        match err {
            VaultError::NotInitialized | VaultError::NoAdmin => {
                GovernanceSourceError::NotInitialized
            }
            _ => GovernanceSourceError::Unauthorized,
        }
    }
}

fn default_config() -> GovernanceSourceConfig {
    GovernanceSourceConfig {
        mode: GovernanceSource::Stake,
        token: None,
    }
}

/// The governance source currently configured (`Stake` when unset).
pub fn current_config(env: &Env) -> GovernanceSourceConfig {
    env.storage()
        .instance()
        .get(&SOURCE_KEY)
        .unwrap_or_else(default_config)
}

/// Snapshot the current source for a newly created proposal. Does nothing
/// when no source is configured (the proposal then behaves as `Stake`).
pub fn snapshot(env: &Env, proposal_id: u32) {
    if let Some(config) = env
        .storage()
        .instance()
        .get::<_, GovernanceSourceConfig>(&SOURCE_KEY)
    {
        env.storage()
            .persistent()
            .set(&(PROPOSAL_SOURCE_KEY, proposal_id), &config);
    }
}

/// The source snapshotted for `proposal_id` (`Stake` if none was recorded).
pub fn proposal_config(env: &Env, proposal_id: u32) -> GovernanceSourceConfig {
    env.storage()
        .persistent()
        .get(&(PROPOSAL_SOURCE_KEY, proposal_id))
        .unwrap_or_else(default_config)
}

/// The user's staked token amount, from their current share balance.
pub fn stake_amount(env: &Env, user: &Address) -> Result<i128, VaultError> {
    let shares = balance::get_shares(env, user);
    if shares <= 0 {
        return Ok(0);
    }
    balance::shares_to_amount(
        balance::get_total_shares(env),
        balance::get_total_deposited(env),
        shares,
    )
    .ok_or(VaultError::ArithmeticError)
}

fn token_balance(env: &Env, config: &GovernanceSourceConfig, user: &Address) -> i128 {
    match &config.token {
        Some(token_addr) => token::Client::new(env, token_addr).balance(user),
        None => 0,
    }
}

/// Combine a stake-derived weight with the governance-token balance according
/// to `config.mode`.
pub fn weight_from_stake(
    env: &Env,
    config: &GovernanceSourceConfig,
    user: &Address,
    stake_weight: i128,
) -> Result<i128, VaultError> {
    match config.mode {
        GovernanceSource::Stake => Ok(stake_weight),
        GovernanceSource::Token => Ok(token_balance(env, config, user)),
        GovernanceSource::Both => stake_weight
            .checked_add(token_balance(env, config, user))
            .ok_or(VaultError::ArithmeticError),
    }
}

/// Raw (pre-`VoteWeightMode`) voting weight of `user` under `config`, using
/// their current stake.
pub fn raw_weight(
    env: &Env,
    config: &GovernanceSourceConfig,
    user: &Address,
) -> Result<i128, VaultError> {
    let stake_weight = if config.mode == GovernanceSource::Token {
        0
    } else {
        stake_amount(env, user)?
    };
    weight_from_stake(env, config, user, stake_weight)
}

#[cfg_attr(not(feature = "testutils"), contractimpl)]
impl VaultContract {
    /// Admin: choose where voting power comes from.
    ///
    /// `Stake` uses the staked amount (any `token` is ignored and cleared).
    /// `Token` and `Both` require `token`, the governance token whose balance
    /// counts as voting power. Applies to proposals created after this call;
    /// open proposals keep the source snapshotted at creation.
    ///
    /// # Authorization
    /// Requires admin.
    ///
    /// # Errors
    /// `TokenRequired` when `mode` is `Token` or `Both` and `token` is `None`.
    ///
    /// # Events
    /// Emits `gv_src_set` with `(mode, token, ledger)`.
    pub fn set_governance_source(
        env: Env,
        admin: Address,
        mode: GovernanceSource,
        token: Option<Address>,
    ) -> Result<(), GovernanceSourceError> {
        admin::require_admin_as(&env, &admin)?;
        let token = match mode {
            GovernanceSource::Stake => None,
            GovernanceSource::Token | GovernanceSource::Both => {
                if token.is_none() {
                    return Err(GovernanceSourceError::TokenRequired);
                }
                token
            }
        };
        let config = GovernanceSourceConfig { mode, token };
        env.storage().instance().set(&SOURCE_KEY, &config);
        env.events().publish(
            (symbol_short!("gv_src_s"),),
            (config.mode, config.token, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the currently configured governance source and token.
    pub fn get_governance_source(env: Env) -> GovernanceSourceConfig {
        current_config(&env)
    }

    /// Read-only: the governance source snapshotted for `proposal_id`.
    pub fn get_proposal_governance_source(env: Env, proposal_id: u32) -> GovernanceSourceConfig {
        proposal_config(&env, proposal_id)
    }
}
