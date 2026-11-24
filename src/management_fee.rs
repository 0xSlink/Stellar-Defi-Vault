//! Annual management fee collected as share dilution (issue #868).
//!
//! Rather than moving tokens out of the pool, the fee accrues continuously and
//! is minted as new shares to a configured recipient (the treasury). Minting
//! shares grows `total_shares` without changing `total_deposited`, so the
//! recipient's claim on the pool grows while every existing share is diluted.
//! That is the same economic effect as a percentage fee on assets under
//! management, but the vault's TVL accounting is untouched.
//!
//! The fee is only ever materialised on interaction: [`accrue`] measures the
//! ledgers elapsed since the last checkpoint against the *current* share count,
//! and [`mint`] moves the pending amount into the recipient's balance. Deposit
//! and withdrawal paths call [`accrue_and_mint`] before pricing shares, so the
//! share price always reflects the pending fee shares.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`.

use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::constants::BPS_DENOMINATOR;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Annual fee in basis points.
const BPS_KEY: Symbol = symbol_short!("mgmt_bps");
/// Address that receives the minted fee shares.
const DEST_KEY: Symbol = symbol_short!("mgmt_dst");
/// Ledger the accrual was last checkpointed at.
const LAST_KEY: Symbol = symbol_short!("mgmt_lst");
/// Fee shares accrued since the last mint, not yet credited.
const ACCRUED_KEY: Symbol = symbol_short!("mgmt_acc");

/// Hard annual ceiling: 20% (2 000 bps) of assets under management.
const MAX_MANAGEMENT_FEE_BPS: u32 = 2_000;

/// Ledgers per year at Stellar's ~5 second close time.
const LEDGERS_PER_YEAR: i128 = 6_307_200;

/// The configured annual fee in basis points (0 when unset).
pub fn fee_bps(env: &Env) -> u32 {
    env.storage().instance().get(&BPS_KEY).unwrap_or(0)
}

/// The configured fee recipient, if any.
pub fn recipient(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DEST_KEY)
}

/// Fee shares accrued since the last mint.
pub fn accrued_shares(env: &Env) -> i128 {
    env.storage().instance().get(&ACCRUED_KEY).unwrap_or(0)
}

fn set_accrued(env: &Env, value: i128) {
    env.storage().instance().set(&ACCRUED_KEY, &value);
}

/// Accrue the fee owed for the ledgers elapsed since the last checkpoint. Safe
/// to call from any state-changing path; a no-op when the fee is disabled or
/// the pool has no shares.
pub fn accrue(env: &Env) {
    let now = env.ledger().sequence();
    let last: u32 = env.storage().instance().get(&LAST_KEY).unwrap_or(now);
    if now <= last {
        return;
    }
    let elapsed = (now - last) as i128;
    env.storage().instance().set(&LAST_KEY, &now);

    let bps = fee_bps(env);
    if bps == 0 {
        return;
    }
    let total_shares = balance::get_total_shares(env);
    if total_shares <= 0 {
        return;
    }

    // total_shares * bps * elapsed / (10_000 * ledgers_per_year)
    let numerator = match total_shares
        .checked_mul(bps as i128)
        .and_then(|v| v.checked_mul(elapsed))
    {
        Some(v) => v,
        None => return,
    };
    let new_shares = numerator / (BPS_DENOMINATOR * LEDGERS_PER_YEAR);
    if new_shares <= 0 {
        return;
    }
    set_accrued(env, accrued_shares(env).saturating_add(new_shares));
}

/// Move the accrued fee shares into the recipient's balance, diluting every
/// other holder. A no-op when there is nothing pending or no recipient is set.
pub fn mint(env: &Env) {
    let pending = accrued_shares(env);
    if pending <= 0 {
        return;
    }
    let recipient = match recipient(env) {
        Some(address) => address,
        None => return,
    };
    let total_shares = balance::get_total_shares(env);
    let current = balance::get_shares(env, &recipient);
    balance::set_shares(env, &recipient, current.saturating_add(pending));
    balance::set_total_shares(env, total_shares.saturating_add(pending));
    set_accrued(env, 0);
    env.events().publish(
        (symbol_short!("mgmt_mint"), recipient),
        (pending, env.ledger().sequence()),
    );
}

/// Accrue then mint: the interaction hook used by the deposit and withdrawal
/// paths so the share price always includes the pending fee shares.
pub fn accrue_and_mint(env: &Env) {
    accrue(env);
    mint(env);
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the annual management fee, in basis points (issue #868).
    ///
    /// Any fee owed under the old rate is accrued and minted before the rate
    /// changes, so a rate change cannot retroactively re-price past ledgers.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    /// - `InvalidManagementFeeBps`: `bps` exceeds the 2 000 bps ceiling.
    ///
    /// # Events
    /// - `mgmt_fee` with the new rate and the ledger.
    pub fn set_management_fee_bps(
        env: Env,
        admin: Address,
        bps: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if bps > MAX_MANAGEMENT_FEE_BPS {
            return Err(VaultSafetyError::InvalidManagementFeeBps);
        }
        accrue_and_mint(&env);
        env.storage().instance().set(&BPS_KEY, &bps);
        env.events().publish(
            (symbol_short!("mgmt_fee"), admin),
            (bps, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Admin: set the address that receives minted fee shares (issue #868).
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    ///
    /// # Events
    /// - `mgmt_dest` with the recipient and the ledger.
    pub fn set_management_fee_recipient(
        env: Env,
        admin: Address,
        recipient: Address,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().set(&DEST_KEY, &recipient);
        env.events().publish(
            (symbol_short!("mgmt_dest"), admin),
            (recipient, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the configured annual management fee in basis points.
    pub fn get_management_fee_bps(env: Env) -> u32 {
        fee_bps(&env)
    }

    /// Read-only: fee shares accrued but not yet minted (issue #868).
    pub fn get_accrued_management_fee_shares(env: Env) -> i128 {
        accrued_shares(&env)
    }
}
