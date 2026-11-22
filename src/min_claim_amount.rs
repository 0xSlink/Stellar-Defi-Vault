//! Admin-configurable minimum claim amount.
//!
//! Prevents dust claims whose gas cost exceeds their value and that bloat the
//! event log. A claim smaller than the configured minimum reverts with
//! [`VaultError::ClaimBelowMinimum`], unless it empties the user's remaining
//! pending reward — so a sub-minimum balance can always be swept out and
//! never gets stranded.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! instance storage, matching `minimum_unstake_amount.rs`.

use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::errors::VaultError;
use crate::vault::VaultContract;

const MIN_CLAIM_KEY: Symbol = symbol_short!("mn_claim");

/// Read the configured minimum claim amount (0 = disabled).
pub fn get_min_claim(env: &Env) -> i128 {
    env.storage().instance().get(&MIN_CLAIM_KEY).unwrap_or(0)
}

/// Enforce the minimum-claim rule for a claim of `claim_amount` taken off a
/// pending reward balance of `pending`.
///
/// Passes when the minimum is disabled (0), the claim is at least the minimum,
/// or the claim empties the whole pending balance.
pub fn enforce_min_claim(
    env: &Env,
    claim_amount: i128,
    pending: i128,
) -> Result<(), VaultError> {
    let min = get_min_claim(env);
    if min > 0 && claim_amount < min && claim_amount < pending {
        return Err(VaultError::ClaimBelowMinimum);
    }
    Ok(())
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the minimum reward amount a single claim must pay out.
    /// `0` disables the minimum. A claim below the minimum is still allowed
    /// when it empties the user's remaining pending reward.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `NotInitialized`: no admin is stored.
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `ZeroAmount`: `amount` is negative.
    ///
    /// # Events
    /// - Topic `("min_claim",)` with data `(admin, amount)`.
    pub fn set_min_claim_amount(env: Env, admin: Address, amount: i128) -> Result<(), VaultError> {
        admin::require_admin_as(&env, &admin)?;
        if amount < 0 {
            return Err(VaultError::ZeroAmount);
        }
        env.storage().instance().set(&MIN_CLAIM_KEY, &amount);
        env.events()
            .publish((symbol_short!("min_claim"),), (admin, amount));
        Ok(())
    }

    /// Read-only: the configured minimum claim amount (`0` = disabled).
    pub fn get_min_claim_amount(env: Env) -> i128 {
        get_min_claim(&env)
    }
}
