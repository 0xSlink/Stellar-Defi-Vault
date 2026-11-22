//! Scheduled deposit windows (issue #870).
//!
//! Deposits are only accepted during admin-defined windows that repeat every
//! `period_ledgers`, staying open for the first `open_ledgers` of each period.
//! This suits strategies that rebalance on a cadence: inflows land in a known
//! slot, and everything else is rejected with `DepositWindowClosed`.
//!
//! Withdrawals and claims are never gated — users can always exit.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`.

use soroban_sdk::{contractimpl, panic_with_error, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Length of a deposit cycle in ledgers (0 disables windows).
const PERIOD_KEY: Symbol = symbol_short!("win_per");
/// Number of ledgers at the start of each cycle during which deposits are open.
const OPEN_KEY: Symbol = symbol_short!("win_opn");

/// The configured window period in ledgers (0 when disabled).
pub fn period_ledgers(env: &Env) -> u32 {
    env.storage().instance().get(&PERIOD_KEY).unwrap_or(0)
}

/// The configured open portion in ledgers.
pub fn open_ledgers(env: &Env) -> u32 {
    env.storage().instance().get(&OPEN_KEY).unwrap_or(0)
}

/// Whether deposits are open at `now`. Always `true` when windows are disabled.
pub fn is_open(env: &Env, now: u32) -> bool {
    let period = period_ledgers(env);
    if period == 0 {
        return true;
    }
    now % period < open_ledgers(env)
}

/// Revert a deposit made outside the open portion of a window.
pub fn require_open(env: &Env) {
    if !is_open(env, env.ledger().sequence()) {
        panic_with_error!(env, VaultSafetyError::DepositWindowClosed);
    }
}

/// The `(opens_at, closes_at)` bounds of the window containing `now`.
pub fn window_at(env: &Env, now: u32) -> (u32, u32) {
    let period = period_ledgers(env);
    if period == 0 {
        return (now, now);
    }
    let opens_at = now - (now % period);
    (opens_at, opens_at.saturating_add(open_ledgers(env)))
}

#[contractimpl]
impl VaultContract {
    /// Admin: configure recurring deposit windows (issue #870).
    ///
    /// Deposits are accepted for the first `open_ledgers` ledgers of every
    /// `period_ledgers`-ledger cycle. `open_ledgers` must be positive and no
    /// longer than the period.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    /// - `InvalidDepositWindow`: `period_ledgers == 0`, `open_ledgers == 0`,
    ///   or `open_ledgers > period_ledgers`.
    ///
    /// # Events
    /// - `win_set` with `(period_ledgers, open_ledgers, ledger)`.
    pub fn set_deposit_window(
        env: Env,
        admin: Address,
        period_ledgers: u32,
        open_ledgers: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if period_ledgers == 0 || open_ledgers == 0 || open_ledgers > period_ledgers {
            return Err(VaultSafetyError::InvalidDepositWindow);
        }
        env.storage().instance().set(&PERIOD_KEY, &period_ledgers);
        env.storage().instance().set(&OPEN_KEY, &open_ledgers);
        env.events().publish(
            (symbol_short!("win_set"), admin),
            (period_ledgers, open_ledgers, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Admin: remove the deposit-window configuration so deposits are always
    /// open again (issue #870).
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    pub fn clear_deposit_window(env: Env, admin: Address) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().remove(&PERIOD_KEY);
        env.storage().instance().remove(&OPEN_KEY);
        env.events().publish(
            (symbol_short!("win_clr"), admin),
            env.ledger().sequence(),
        );
        Ok(())
    }

    /// Read-only: `(opens_at, closes_at)` of the current window (issue #870).
    /// When windows are disabled this is `(now, now)`.
    pub fn get_next_window(env: Env) -> (u32, u32) {
        window_at(&env, env.ledger().sequence())
    }
}
