//! Issue #835: per-ledger deposit throughput limit.
//!
//! Caps how much can be deposited within a short rolling window of ledgers,
//! to blunt flash-style inflows and give monitoring time to react.
//!
//! # Semantics
//!
//! The admin configures a `(limit, window_ledgers)` pair. Every successful
//! deposit records the amount it credited against the *current* window. A
//! window only rolls forward once `window_ledgers` have elapsed since it
//! opened, so old inflows age out gradually rather than all resetting at a
//! fixed boundary an attacker could time around. A deposit that would push the
//! window's running total over `limit` reverts with `ThroughputExceeded`.
//!
//! `limit == 0` disables the feature (default), so existing pools keep their
//! behaviour unchanged.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the config lives under a raw
//! `Symbol` key in instance storage, and the running window counter lives in
//! *temporary* storage (as the acceptance criteria require) — expired or
//! absent state simply means an empty window.

use soroban_sdk::{
    contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address, Env, Symbol,
};

use crate::admin;
use crate::vault::VaultContract;
use crate::vault::VaultContractClient;

/// Instance key for the `(limit, window_ledgers)` config.
const CONFIG_KEY: Symbol = symbol_short!("dtp_cfg");
/// Temporary key for the current rolling-window counter.
const WINDOW_KEY: Symbol = symbol_short!("dtp_win");

/// Errors for the deposit throughput limit.
///
/// `VaultError` already uses every code from 1 to 50, so `ThroughputExceeded`
/// sits above that range (mirroring `max_deposit_cap::DepositCapError`): a
/// client decoding the failure as `VaultError` then sees an unknown contract
/// error rather than misreading it as an unrelated variant.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ThroughputError {
    /// The caller is not the stored admin.
    Unauthorized = 1,
    /// The pool has not been initialized.
    NotInitialized = 2,
    /// `limit` was negative, or a positive limit was paired with a zero window.
    InvalidLimit = 3,
    /// The deposit would push the current window's total over the configured
    /// limit.
    ThroughputExceeded = 51,
}

/// Admin-configured throughput limit. `limit == 0` means the limit is off.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ThroughputConfig {
    /// Maximum amount depositable per rolling window, in stake-token units.
    pub limit: i128,
    /// Length of the rolling window, in ledgers.
    pub window_ledgers: u32,
}

/// The running total for the current rolling window.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ThroughputWindow {
    /// Ledger at which the current window opened.
    pub start_ledger: u32,
    /// Amount deposited so far in the current window.
    pub total: i128,
}

/// The stored config, or `None` when the limit has never been configured.
pub fn get_config(env: &Env) -> Option<ThroughputConfig> {
    env.storage().instance().get(&CONFIG_KEY)
}

/// The current window's counter, or a fresh one when the stored counter has
/// aged out (`window_ledgers` elapsed) or was never written.
fn current_window(env: &Env, config: &ThroughputConfig) -> ThroughputWindow {
    let now = env.ledger().sequence();
    match env.storage().temporary().get::<_, ThroughputWindow>(&WINDOW_KEY) {
        Some(window) if now.saturating_sub(window.start_ledger) < config.window_ledgers => window,
        _ => ThroughputWindow {
            start_ledger: now,
            total: 0,
        },
    }
}

/// Records `amount` against the current rolling window, reverting with
/// `ThroughputExceeded` when that would exceed the configured limit.
///
/// No-op while the limit is disabled (`limit == 0`). Called from the deposit
/// path after the credited amount is known, so a rejection aborts the whole
/// deposit (Soroban rolls back the token transfer with it).
pub fn enforce_and_record(env: &Env, amount: i128) {
    let Some(config) = get_config(env) else {
        return;
    };
    if config.limit == 0 {
        return;
    }

    let mut window = current_window(env, &config);
    let new_total = match window.total.checked_add(amount) {
        Some(total) => total,
        None => panic_with_error!(env, ThroughputError::ThroughputExceeded),
    };
    if new_total > config.limit {
        panic_with_error!(env, ThroughputError::ThroughputExceeded);
    }

    window.total = new_total;
    env.storage().temporary().set(&WINDOW_KEY, &window);
    // Keep the counter alive across the window plus a margin so a slow window
    // never expires out from under an in-flight measurement.
    let ttl = config.window_ledgers.saturating_add(1);
    env.storage().temporary().extend_ttl(&WINDOW_KEY, ttl, ttl);
}

/// Remaining throughput available in the current window. `0` when the limit is
/// disabled or already exhausted.
pub fn remaining(env: &Env) -> i128 {
    let Some(config) = get_config(env) else {
        return 0;
    };
    if config.limit == 0 {
        return 0;
    }
    let window = current_window(env, &config);
    config.limit.saturating_sub(window.total).max(0)
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the maximum amount depositable per rolling window of
    /// `window_ledgers` ledgers. `limit == 0` disables the limit. A positive
    /// limit requires a non-zero window. Emits
    /// `deposit_throughput_updated` as `(admin, limit, window_ledgers, ledger)`.
    pub fn set_deposit_throughput_limit(
        env: Env,
        admin: Address,
        limit: i128,
        window_ledgers: u32,
    ) -> Result<(), ThroughputError> {
        admin.require_auth();
        let stored = admin::get_admin(&env).map_err(|_| ThroughputError::NotInitialized)?;
        if admin != stored {
            return Err(ThroughputError::Unauthorized);
        }
        if limit < 0 || (limit > 0 && window_ledgers == 0) {
            return Err(ThroughputError::InvalidLimit);
        }

        let config = ThroughputConfig {
            limit,
            window_ledgers,
        };
        env.storage().instance().set(&CONFIG_KEY, &config);
        // A fresh config starts a fresh window, so a limit change cannot be
        // dodged (or tripped) by a stale counter from the previous config.
        env.storage().temporary().remove(&WINDOW_KEY);
        env.events().publish(
            (symbol_short!("dtp_set"),),
            (admin, limit, window_ledgers, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the configured per-window deposit limit (`0` = disabled).
    pub fn get_deposit_throughput_limit(env: Env) -> i128 {
        get_config(&env).map(|c| c.limit).unwrap_or(0)
    }

    /// Read-only: amount still depositable in the current rolling window.
    /// `0` when the limit is disabled or the window is already exhausted.
    pub fn get_remaining_throughput(env: Env) -> i128 {
        remaining(&env)
    }
}
