//! Pool-wide reward accrual pause (issue #830).
//!
//! `set_rewards_paused` lets the admin or guardian stop reward accrual across
//! the whole pool without pausing the pool itself: deposits, withdrawals and
//! claims all keep working, and rewards already accrued stay claimable.
//!
//! # No accrual while paused, no retroactive catch-up
//!
//! Ledgers spent in the paused window earn nothing. The paused interval is
//! remembered as `[pause_start, unpause)`:
//!
//! - While paused, [`accrual_bounds`] clamps a position's accrual window to
//!   `pause_start`, so any rewards earned *before* the pause are still
//!   realised on the next touch, and the position's checkpoint advances to
//!   `pause_start` (not past it).
//! - After unpause, a position whose checkpoint is at or before `pause_start`
//!   has its window end pulled back by the pause length, so the paused ledgers
//!   are never retroactively accrued and nothing is counted twice.
//!
//! Claims (`do_claim`) keep working while paused; they simply realise whatever
//! accrued before the pause.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! instance storage, matching `balance.rs` and `withdraw_only_mode.rs`.

use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Instance-storage key for the "rewards paused" flag.
const PAUSED_KEY: Symbol = symbol_short!("rw_pause");
/// Ledger at which the active (or most recent) pause began.
const START_KEY: Symbol = symbol_short!("rwps");
/// Ledger at which the most recent pause was lifted.
const END_KEY: Symbol = symbol_short!("rwpe");

/// Whether reward accrual is currently paused.
pub fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&PAUSED_KEY).unwrap_or(false)
}

fn pause_start(env: &Env) -> Option<u32> {
    env.storage().instance().get(&START_KEY)
}

fn pause_end(env: &Env) -> Option<u32> {
    env.storage().instance().get(&END_KEY)
}

/// The `[start, end)` ledger window `accrue_rewards` should actually accrue
/// over for a position whose stored checkpoint is `checkpoint`, given the
/// current ledger `now` (issue #830). See the module docs for the rules.
pub fn accrual_bounds(env: &Env, checkpoint: u32, now: u32) -> (u32, u32) {
    if is_paused(env) {
        // Accrue no further than the ledger the pause began at.
        let start_of_pause = pause_start(env).unwrap_or(now);
        let end = if now > start_of_pause {
            start_of_pause
        } else {
            now
        };
        return (checkpoint.min(end), end);
    }

    // Not paused: exclude the most recent completed paused interval from the
    // window so it is neither accrued now nor caught up retroactively.
    if let (Some(start_of_pause), Some(end_of_pause)) = (pause_start(env), pause_end(env)) {
        if end_of_pause > start_of_pause && checkpoint <= start_of_pause && now > end_of_pause {
            let paused_ledgers = end_of_pause - start_of_pause;
            let end = now.saturating_sub(paused_ledgers);
            return (checkpoint.min(end), end);
        }
    }

    (checkpoint, now)
}

/// The checkpoint to store after an accrual (issue #830): the pause start
/// while paused, otherwise the current ledger. `checkpoint` is the value the
/// accrual started from; it is never moved backwards, so a position that was
/// already checkpointed past the pause start (for example by the
/// reward-excluded or epoch-mode paths) is not rewound into the paused window.
pub fn checkpoint_after_accrual(env: &Env, checkpoint: u32, now: u32) -> u32 {
    if is_paused(env) {
        let start_of_pause = pause_start(env).unwrap_or(now);
        if checkpoint > start_of_pause {
            checkpoint
        } else {
            start_of_pause
        }
    } else {
        now
    }
}

#[contractimpl]
impl VaultContract {
    /// Admin or guardian: pause (`true`) or resume (`false`) pool-wide reward
    /// accrual (issue #830). Idempotent: re-pausing keeps the original pause
    /// start, so the frozen window is not extended.
    ///
    /// # Authorization
    /// `caller` must sign and be either the stored admin or the stored guardian.
    ///
    /// # Errors
    /// - `Unauthorized`: `caller` is neither the admin nor the guardian.
    ///
    /// # Events
    /// Emits `rw_paused` with `(paused, ledger)` on an actual state change.
    pub fn set_rewards_paused(
        env: Env,
        caller: Address,
        paused: bool,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_or_guardian(&env, &caller)?;
        if is_paused(&env) == paused {
            return Ok(());
        }
        let now = env.ledger().sequence();
        if paused {
            env.storage().instance().set(&PAUSED_KEY, &true);
            env.storage().instance().set(&START_KEY, &now);
        } else {
            env.storage().instance().set(&PAUSED_KEY, &false);
            env.storage().instance().set(&END_KEY, &now);
        }
        env.events()
            .publish((symbol_short!("rw_paused"), caller.clone()), (paused, now));
        Ok(())
    }

    /// Read-only: whether reward accrual is currently paused (issue #830).
    pub fn get_rewards_paused(env: Env) -> bool {
        is_paused(&env)
    }
}
