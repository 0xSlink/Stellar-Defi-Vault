//! Issue #836: auto-pause when a core invariant check fails.
//!
//! If vault accounting ever becomes inconsistent after a state change, the
//! pool must stop accepting further mutations instead of continuing to operate
//! on corrupted bookkeeping.
//!
//! # Design
//!
//! Core mutating entrypoints ([`crate::vault::VaultContract::stake`] and the
//! unstake core) run a *cheap* invariant subset on exit and revert the
//! triggering call when it fails, so a mutation can never leave obviously
//! broken state committed.
//!
//! Soroban rolls back **all** storage writes (and discards events) when an
//! invocation fails, so a failing call cannot itself persist a `HealthPaused`
//! flag. The flag is therefore latched by the companion entrypoint
//! [`VaultContract::run_health_check`]: a non-reverting keeper/admin call that
//! re-runs the same subset, and on failure sets `HealthPaused` and emits
//! `invariant_violation`. Once the flag is set, every core mutating entrypoint
//! refuses to run until an admin clears it with
//! [`VaultContract::clear_health_pause`].
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the flag lives under a raw
//! `Symbol` key in instance storage.

use soroban_sdk::{contracterror, contractimpl, panic_with_error, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::errors::VaultError;
use crate::vault::VaultContract;
use crate::vault::VaultContractClient;

/// Instance key for the health-pause latch.
const PAUSED_KEY: Symbol = symbol_short!("hlth_pse");

/// Errors for the health guard.
///
/// `InvariantViolation` sits above `VaultError`'s 1-50 range, matching the
/// convention used by the other safety modules.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum HealthError {
    /// The caller is not the stored admin.
    Unauthorized = 1,
    /// The pool has not been initialized.
    NotInitialized = 2,
    /// A core accounting invariant failed.
    InvariantViolation = 51,
}

/// Whether the health-pause latch is currently set.
pub fn is_paused_inner(env: &Env) -> bool {
    env.storage().instance().get(&PAUSED_KEY).unwrap_or(false)
}

/// Cheap invariant subset evaluated on every core mutation's exit and by
/// [`VaultContract::run_health_check`].
///
/// Deliberately O(1): it validates only the aggregate totals, not the full
/// per-staker sum (that heavier check lives in `invariants::check_invariants`).
/// A healthy pool never fails this, so wiring it into the hot path adds no
/// false positives.
pub fn healthy(env: &Env) -> bool {
    let total_shares = balance::get_total_shares(env);
    let total_deposited = balance::get_total_deposited(env);
    let reward_pool = balance::get_reward_pool_balance(env);

    if total_shares < 0 || total_deposited < 0 || reward_pool < 0 {
        return false;
    }
    // Shares and principal must vanish together: a pool with no shares can
    // never owe principal.
    if total_shares == 0 && total_deposited != 0 {
        return false;
    }
    true
}

/// Reverts the calling core mutation with `InvariantViolation` when the subset
/// fails, so the bad state is rolled back with it.
pub fn assert_healthy(env: &Env) {
    if !healthy(env) {
        panic_with_error!(env, HealthError::InvariantViolation);
    }
}

/// Reverts a core mutating entrypoint while the health-pause latch is set.
pub fn require_not_health_paused(env: &Env) -> Result<(), VaultError> {
    if is_paused_inner(env) {
        Err(VaultError::VaultPaused)
    } else {
        Ok(())
    }
}

#[contractimpl]
impl VaultContract {
    /// Keeper/admin: re-run the cheap invariant subset and latch `HealthPaused`
    /// (emitting `invariant_violation`) when it fails.
    ///
    /// Returns `true` when the pool is healthy, `false` when a violation was
    /// detected and the pool was health-paused. Unlike a failing mutation this
    /// call *succeeds*, so its flag/event survive — see the module docs for why
    /// the latch cannot be written from a reverting call.
    pub fn run_health_check(env: Env, caller: Address) -> Result<bool, HealthError> {
        caller.require_auth();
        let is_healthy = healthy(&env);
        if !is_healthy {
            env.storage().instance().set(&PAUSED_KEY, &true);
            env.events().publish(
                (symbol_short!("inv_viol"),),
                (
                    balance::get_total_shares(&env),
                    balance::get_total_deposited(&env),
                    balance::get_reward_pool_balance(&env),
                    env.ledger().sequence(),
                ),
            );
        }
        Ok(is_healthy)
    }

    /// Read-only: whether the pool is currently health-paused.
    pub fn is_health_paused(env: Env) -> bool {
        is_paused_inner(&env)
    }

    /// Admin: clear the health-pause latch after investigating the violation,
    /// re-enabling core mutations. Emits `health_cleared`. Reverts with
    /// `Unauthorized` when `admin` is not the stored admin.
    pub fn clear_health_pause(env: Env, admin: Address) -> Result<(), HealthError> {
        admin.require_auth();
        let stored = admin::get_admin(&env).map_err(|_| HealthError::NotInitialized)?;
        if admin != stored {
            return Err(HealthError::Unauthorized);
        }
        env.storage().instance().remove(&PAUSED_KEY);
        env.events()
            .publish((symbol_short!("hlth_clr"),), (admin, env.ledger().sequence()));
        Ok(())
    }
}
