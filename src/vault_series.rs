//! Fixed-term vault series with a maturity date (issue #869).
//!
//! A series opens a deposit window and locks all funds until a maturity
//! ledger, after which everyone can redeem. The phase is derived from the two
//! stored ledgers so it needs no upkeep:
//!
//! - `Open`   — deposits accepted, withdrawals blocked.
//! - `Locked` — deposits closed, withdrawals still blocked.
//! - `Matured` — deposits closed and withdrawals allowed.
//!
//! An unconfigured vault is always `Open` and behaves exactly as before, so
//! the series is opt-in. Deposits and withdrawals call the gate helpers below,
//! which revert with the matching `VaultSafetyError` / `VaultError` when the
//! phase does not allow the action.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`.

use soroban_sdk::{contractimpl, contracttype, panic_with_error, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Ledger after which new deposits are rejected.
const DEPOSIT_END_KEY: Symbol = symbol_short!("ser_dend");
/// Ledger at which withdrawals open.
const MATURES_KEY: Symbol = symbol_short!("ser_mat");

/// Lifecycle phase of the configured series.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum SeriesPhase {
    /// Deposits accepted; withdrawals blocked.
    Open,
    /// Deposits closed; withdrawals still blocked.
    Locked,
    /// Matured: deposits closed and withdrawals open.
    Matured,
}

/// The ledger after which deposits close, if a series is configured.
pub fn deposit_ends_at(env: &Env) -> Option<u32> {
    env.storage().instance().get(&DEPOSIT_END_KEY)
}

/// The maturity ledger, if a series is configured.
pub fn matures_at(env: &Env) -> Option<u32> {
    env.storage().instance().get(&MATURES_KEY)
}

/// The current phase. Unconfigured vaults report `Open`.
pub fn phase(env: &Env) -> SeriesPhase {
    match (deposit_ends_at(env), matures_at(env)) {
        (Some(deposit_end), Some(matures)) => {
            let now = env.ledger().sequence();
            if now > matures {
                SeriesPhase::Matured
            } else if now > deposit_end {
                SeriesPhase::Locked
            } else {
                SeriesPhase::Open
            }
        }
        _ => SeriesPhase::Open,
    }
}

/// Revert a deposit made after the configured deposit window closes.
pub fn require_deposit_allowed(env: &Env) {
    if let Some(deposit_end) = deposit_ends_at(env) {
        if env.ledger().sequence() > deposit_end {
            panic_with_error!(env, VaultSafetyError::SeriesDepositClosed);
        }
    }
}

/// Revert a withdrawal made before the configured maturity ledger.
pub fn require_withdraw_allowed(env: &Env) {
    if let Some(matures) = matures_at(env) {
        if env.ledger().sequence() < matures {
            panic_with_error!(env, VaultSafetyError::SeriesNotMatured);
        }
    }
}

#[contractimpl]
impl VaultContract {
    /// Admin: configure a fixed-term series (issue #869).
    ///
    /// `deposit_ends_at` closes the series to new deposits and `matures_at`
    /// opens withdrawals; both are absolute ledgers and maturity must be
    /// strictly after the deposit close.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    /// - `InvalidSeriesConfig`: `matures_at <= deposit_ends_at`.
    ///
    /// # Events
    /// - `series_cfg` with `(deposit_ends_at, matures_at, ledger)`.
    pub fn configure_series(
        env: Env,
        admin: Address,
        deposit_ends_at: u32,
        matures_at: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if matures_at <= deposit_ends_at {
            return Err(VaultSafetyError::InvalidSeriesConfig);
        }
        env.storage().instance().set(&DEPOSIT_END_KEY, &deposit_ends_at);
        env.storage().instance().set(&MATURES_KEY, &matures_at);
        env.events().publish(
            (symbol_short!("series_cfg"), admin),
            (deposit_ends_at, matures_at, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Admin: start a fresh series once the current one has matured
    /// (issue #869). The new bounds replace the old ones, reopening deposits.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    /// - `SeriesNotMatured`: the current series has not matured yet.
    /// - `InvalidSeriesConfig`: `matures_at <= deposit_ends_at`.
    ///
    /// # Events
    /// - `series_roll` with `(deposit_ends_at, matures_at, ledger)`.
    pub fn rollover_series(
        env: Env,
        admin: Address,
        deposit_ends_at: u32,
        matures_at: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if phase(&env) != SeriesPhase::Matured {
            return Err(VaultSafetyError::SeriesNotMatured);
        }
        if matures_at <= deposit_ends_at {
            return Err(VaultSafetyError::InvalidSeriesConfig);
        }
        env.storage().instance().set(&DEPOSIT_END_KEY, &deposit_ends_at);
        env.storage().instance().set(&MATURES_KEY, &matures_at);
        env.events().publish(
            (symbol_short!("series_roll"), admin),
            (deposit_ends_at, matures_at, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Admin: remove the series configuration, returning the vault to its
    /// unconfigured (always-open) behaviour (issue #869).
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    pub fn clear_series(env: Env, admin: Address) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().remove(&DEPOSIT_END_KEY);
        env.storage().instance().remove(&MATURES_KEY);
        env.events().publish(
            (symbol_short!("series_clr"), admin),
            env.ledger().sequence(),
        );
        Ok(())
    }

    /// Read-only: the current series phase (issue #869).
    pub fn get_series_status(env: Env) -> SeriesPhase {
        phase(&env)
    }
}
