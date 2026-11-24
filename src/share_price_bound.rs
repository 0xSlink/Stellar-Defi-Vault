//! Issue #837: per-operation share price change bound.
//!
//! Share price should move smoothly. Every core value-changing operation
//! (deposit, withdraw, and reward injection) is checked against the price
//! recorded after the previous such operation; if a single operation moves the
//! price by more than the configured bound, the call reverts with
//! `PriceMoveExceeded`. This catches donation/injection-style jumps and math
//! bugs in flight.
//!
//! # What "share price" means here
//!
//! Price per share is the pool's backing assets per share:
//! `(total_deposited + reward_pool) / total_shares`, scaled for integer math.
//! Including the reward pool makes a reward injection (`add_yield` /
//! `fund_reward_pool`) the primary thing the bound guards, while ordinary
//! deposits and withdrawals mint/burn shares at the current price and so leave
//! it essentially unchanged (only rounding moves it).
//!
//! The check is *not* applied to reward claims: a large claim legitimately
//! lowers the reward pool (and therefore the price), and must never be
//! blocked.
//!
//! # Staged funding
//!
//! A legitimately large reward injection can exceed the bound in one step. Use
//! [`VaultContract::fund_reward_pool_staged`] instead: it credits the funding
//! in bounded increments so each step passes the check, while an unauthorised
//! or accidental jump still reverts.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the bound and the last recorded
//! price live under raw `Symbol` keys in instance storage.

use soroban_sdk::{
    contracterror, contractimpl, symbol_short, panic_with_error, Address, Env, Symbol,
};

use crate::admin;
use crate::balance;
use crate::constants::BPS_DENOMINATOR;
use crate::errors::VaultError;
use crate::vault::VaultContract;
use crate::vault::VaultContractClient;

/// Instance key for the configured bound, in basis points.
const BOUND_KEY: Symbol = symbol_short!("spb_bps");
/// Instance key for the last recorded price.
const LAST_KEY: Symbol = symbol_short!("spb_lst");

/// Fixed-point scale applied to the price so bps moves stay meaningful for
/// pools whose raw price is below 1.
const PRICE_SCALE: i128 = 1_000_000;

/// Errors for the share price bound.
///
/// `PriceMoveExceeded` sits above `VaultError`'s 1-50 range, matching the
/// convention used by the other safety modules.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PriceBoundError {
    /// The caller is not the stored admin.
    Unauthorized = 1,
    /// The pool has not been initialized.
    NotInitialized = 2,
    /// `bps` exceeded 10 000 (100%).
    InvalidBound = 3,
    /// A staged funding split would produce a zero-sized step.
    InvalidAmount = 4,
    /// The operation would move the share price by more than the bound.
    PriceMoveExceeded = 51,
}

/// The configured bound in basis points, `0` when disabled.
pub fn get_bound_bps(env: &Env) -> u32 {
    env.storage().instance().get(&BOUND_KEY).unwrap_or(0)
}

/// Current price per share, scaled by [`PRICE_SCALE`]. `0` when the pool has no
/// shares (there is nothing meaningful to price yet).
fn price_of(env: &Env) -> i128 {
    let shares = balance::get_total_shares(env);
    if shares <= 0 {
        return 0;
    }
    let assets = balance::get_total_deposited(env)
        .saturating_add(balance::get_reward_pool_balance(env));
    assets
        .saturating_mul(PRICE_SCALE)
        .checked_div(shares)
        .unwrap_or(i128::MAX)
}

/// Records the current price, reverting with `PriceMoveExceeded` when it moved
/// from the previously recorded price by more than the configured bound.
///
/// No-op check (but still records) while the bound is disabled, so switching
/// the bound on always starts from an up-to-date baseline.
pub fn check_and_record(env: &Env) {
    let price = price_of(env);
    let previous: Option<i128> = env.storage().instance().get(&LAST_KEY);
    let bound = get_bound_bps(env);

    if let Some(previous) = previous {
        if bound > 0 && previous > 0 && price > 0 {
            let delta = price.abs_diff(previous);
            let move_bps = delta
                .saturating_mul(BPS_DENOMINATOR)
                .checked_div(previous)
                .unwrap_or(i128::MAX);
            if move_bps > bound as i128 {
                panic_with_error!(env, PriceBoundError::PriceMoveExceeded);
            }
        }
    }

    env.storage().instance().set(&LAST_KEY, &price);
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the maximum share-price move, in basis points, a single
    /// operation may cause. `0` disables the bound. Emits
    /// `max_price_move_updated` as `(admin, bps, ledger)`.
    pub fn set_max_price_move_bps(
        env: Env,
        admin: Address,
        bps: u32,
    ) -> Result<(), PriceBoundError> {
        admin.require_auth();
        let stored = admin::get_admin(&env).map_err(|_| PriceBoundError::NotInitialized)?;
        if admin != stored {
            return Err(PriceBoundError::Unauthorized);
        }
        if bps > BPS_DENOMINATOR as u32 {
            return Err(PriceBoundError::InvalidBound);
        }
        env.storage().instance().set(&BOUND_KEY, &bps);
        // Re-baseline so the bound applies from the current price forward.
        env.storage().instance().set(&LAST_KEY, &price_of(&env));
        env.events().publish(
            (symbol_short!("spb_set"),),
            (admin, bps, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the configured maximum share-price move in bps (`0` =
    /// disabled).
    pub fn get_max_price_move_bps(env: Env) -> u32 {
        get_bound_bps(&env)
    }

    /// Admin: fund the reward pool in `slices` equal increments instead of one
    /// lump sum, so a large injection can satisfy the per-operation price bound
    /// while still being fully credited in this call.
    ///
    /// This is the staged path referenced by the share-price bound docs: each
    /// increment is bounded individually, so a single oversized step still
    /// reverts, but a legitimately large funding succeeds.
    pub fn fund_reward_pool_staged(
        env: Env,
        admin: Address,
        amount: i128,
        slices: u32,
    ) -> Result<(), VaultError> {
        if amount <= 0 || slices == 0 {
            return Err(VaultError::ZeroAmount);
        }
        let step = amount
            .checked_div(slices as i128)
            .ok_or(VaultError::ArithmeticError)?;
        if step <= 0 {
            return Err(VaultError::ZeroAmount);
        }
        // Re-validate admin up front so a bad caller fails fast (and once).
        admin::require_admin_as(&env, &admin)?;

        let mut credited: i128 = 0;
        let mut remaining = slices;
        while remaining > 0 {
            // Give the last step whatever is left so rounding dust is not lost.
            let slice = if remaining == 1 { amount - credited } else { step };
            crate::vault::VaultContract::fund_reward_pool(env.clone(), admin.clone(), slice)?;
            credited = credited
                .checked_add(slice)
                .ok_or(VaultError::ArithmeticError)?;
            remaining -= 1;
        }
        Ok(())
    }
}
