//! Rage-quit window: a fee-free exit during the notice period before an
//! adverse change takes effect.
//!
//! When the admin announces an adverse change — a queued unstake-fee increase
//! (`fee_change_notice`) or a time-locked config-change announcement
//! (`time_locked_admin_proposal`) — a `rage_quit_until` window opens that
//! lasts until the change can take effect. While `ledger < rage_quit_until`:
//!
//! - withdrawals skip the unstake fee, and
//! - withdrawals skip the unstake cooldown, so users can leave in one step.
//!
//! Locks are **not** waived: a position that is locked still pays its
//! early-exit penalty / stays locked exactly as usual.
//!
//! Overlapping announcements extend the window to the latest end; a window is
//! never shortened. Once the ledger reaches `rage_quit_until`, normal rules
//! apply again.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses a raw `Symbol` key.

use soroban_sdk::{contractimpl, symbol_short, Env, Symbol};

use crate::vault::VaultContract;

const RAGE_QUIT_KEY: Symbol = symbol_short!("rq_until");

fn stored_until(env: &Env) -> Option<u32> {
    env.storage().instance().get(&RAGE_QUIT_KEY)
}

/// Open (or extend) the window so it lasts until ledger `until`.
///
/// A no-op when `until` is not in the future or does not extend the current
/// window. Emits `rq_open` with `(until, ledger)` when the window changes.
pub fn open_window(env: &Env, until: u32) {
    let now = env.ledger().sequence();
    if until <= now {
        return;
    }
    if let Some(current) = stored_until(env) {
        if current >= until {
            return;
        }
    }
    env.storage().instance().set(&RAGE_QUIT_KEY, &until);
    env.events()
        .publish((symbol_short!("rq_open"),), (until, now));
}

/// The ledger the window closes at, while it is open.
pub fn get_window(env: &Env) -> Option<u32> {
    stored_until(env).filter(|until| env.ledger().sequence() < *until)
}

/// Whether a rage-quit window is currently open.
pub fn is_active(env: &Env) -> bool {
    get_window(env).is_some()
}

#[contractimpl]
impl VaultContract {
    /// Read-only: the ledger (`rage_quit_until`) at which the current
    /// rage-quit window closes, or `None` when no window is open.
    ///
    /// While a window is open, withdrawals skip the unstake fee and the
    /// unstake cooldown; locks are unaffected.
    pub fn get_rage_quit_window(env: Env) -> Option<u32> {
        get_window(&env)
    }
}
