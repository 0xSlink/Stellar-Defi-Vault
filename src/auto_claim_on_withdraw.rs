//! Opt-in auto-claim on withdraw.
//!
//! By default pending rewards keep accruing separately when a user withdraws.
//! A user who calls `set_auto_claim_on_withdraw(user, true)` instead has their
//! pending rewards paid out as part of every withdrawal (`withdraw`, `unstake`,
//! `unstake_all`, `execute_withdrawal`, `withdraw_from`, ...), through the same
//! claim path `claim` uses. The payout therefore goes wherever `claim` pays
//! (the staker) and the claim's own fee, vesting and bookkeeping all apply.
//!
//! # Claim rules are respected, never bypassed
//!
//! Auto-claim is best-effort: a withdrawal must never be blocked because a
//! reward rule says "not now". When any of these would stop a standalone
//! claim, the auto-claim is skipped, the rewards stay pending for a later
//! claim, the withdrawal proceeds, and an `acw_skip` event records why:
//!
//! - **claim cooldown**: the per-user claim rate limit has not elapsed,
//! - **minimum**: the claim is below the configured minimum claim amount (a
//!   claim that empties the pending balance is always allowed),
//! - **vesting**: the vesting queue is full (when vesting is enabled the
//!   reward is queued into vesting, exactly as `claim` does),
//! - claims paused through the pause mask, epoch mode, halted emissions, or an
//!   under-funded reward pool.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the flag is a raw
//! `Symbol`-keyed persistent entry, matching `balance.rs`.

use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol, Vec};

use crate::balance;
use crate::errors::VaultError;
use crate::storage::{DataKey, VestingEntry};
use crate::vault::VaultContract;

const AUTO_CLAIM_KEY: Symbol = symbol_short!("acw_on");

/// Most vesting entries a user may hold (matches `do_claim`).
const MAX_VESTING_ENTRIES: u32 = 10;

fn key(user: &Address) -> (Symbol, Address) {
    (AUTO_CLAIM_KEY, user.clone())
}

/// Whether `user` has opted in to auto-claim on withdraw.
pub fn is_enabled(env: &Env, user: &Address) -> bool {
    env.storage().persistent().get(&key(user)).unwrap_or(false)
}

fn skip(env: &Env, user: &Address, reason: Symbol) {
    env.events()
        .publish((symbol_short!("acw_skip"), user.clone()), reason);
}

/// Pay `staker`'s pending rewards as part of a withdrawal, if they opted in
/// and the normal claim rules allow it. Returns the amount paid.
///
/// Must run after rewards were accrued and before the shares are burned.
pub(crate) fn maybe_claim_on_withdraw(env: &Env, staker: &Address) -> Result<i128, VaultError> {
    if !is_enabled(env, staker) {
        return Ok(0);
    }
    let accrued = balance::get_accrued_reward(env, staker);
    if accrued <= 0 {
        return Ok(0);
    }

    // Claims paused via the pause mask: withdrawals stay open, rewards stay pending.
    if crate::pause_mask::is_paused(env, crate::pause_mask::PAUSE_CLAIM) {
        skip(env, staker, symbol_short!("paused"));
        return Ok(0);
    }

    let now = env.ledger().sequence();

    // Claim cooldown: same window `claim` enforces between claim actions.
    let min_ledgers = balance::get_claim_rate_limit(env);
    if min_ledgers > 0 {
        if let Some(last) = balance::get_last_claim_action_ledger(env, staker) {
            if now.saturating_sub(last) < min_ledgers {
                skip(env, staker, symbol_short!("cooldown"));
                return Ok(0);
            }
        }
    }

    let epoch_mode: bool = env
        .storage()
        .instance()
        .get(&DataKey::EpochMode)
        .unwrap_or(false);
    if epoch_mode {
        skip(env, staker, symbol_short!("epoch"));
        return Ok(0);
    }
    if crate::peg_stabilization::emissions_halted(env) {
        skip(env, staker, symbol_short!("halted"));
        return Ok(0);
    }
    if balance::get_reward_pool_balance(env) < accrued {
        skip(env, staker, symbol_short!("pool"));
        return Ok(0);
    }

    // Vesting: the claim queues into vesting when a period is configured, and
    // a full queue would make it fail.
    let vesting_period: u32 = env
        .storage()
        .instance()
        .get(&DataKey::VestingPeriod)
        .unwrap_or(0);
    if vesting_period > 0 {
        let entries: Vec<VestingEntry> = env
            .storage()
            .persistent()
            .get(&DataKey::VestingEntries(staker.clone()))
            .unwrap_or_else(|| Vec::new(env));
        if entries.len() >= MAX_VESTING_ENTRIES {
            skip(env, staker, symbol_short!("vest_full"));
            return Ok(0);
        }
    }

    // `do_claim` enforces the minimum claim amount (and the per-user claim
    // cap) before it changes any state, so a rejection here is a clean skip.
    match VaultContract::do_claim(env, staker) {
        Ok(paid) => {
            balance::set_last_claim_action_ledger(env, staker, now);
            Ok(paid)
        }
        Err(err) if err == VaultError::ClaimBelowMinimum => {
            skip(env, staker, symbol_short!("min_claim"));
            Ok(0)
        }
        Err(VaultError::EpochModeConflict) => {
            skip(env, staker, symbol_short!("epoch"));
            Ok(0)
        }
        Err(err) => Err(err),
    }
}

#[cfg_attr(not(feature = "testutils"), contractimpl)]
impl VaultContract {
    /// Opt in (`true`) or out (`false`) of having pending rewards paid out on
    /// every withdrawal. Off by default.
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Events
    /// Emits `acw_set` with `(enabled, ledger)`.
    pub fn set_auto_claim_on_withdraw(env: Env, user: Address, enabled: bool) {
        user.require_auth();
        if enabled {
            env.storage().persistent().set(&key(&user), &true);
        } else {
            env.storage().persistent().remove(&key(&user));
        }
        env.events().publish(
            (symbol_short!("acw_set"), user),
            (enabled, env.ledger().sequence()),
        );
    }

    /// Read-only: whether `user` has auto-claim on withdraw enabled.
    pub fn get_auto_claim_on_withdraw(env: Env, user: Address) -> bool {
        is_enabled(&env, &user)
    }
}
