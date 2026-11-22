//! Pro-rata ("haircut") withdrawal mode for insolvency events (issue #833).
//!
//! When a recorded loss leaves the vault's assets below its liabilities
//! (principal plus accrued-but-unclaimed rewards), a first-come-first-served
//! run on withdrawals pays the early withdrawers in full and leaves the last
//! ones with nothing.
//!
//! `enter_insolvency_mode` freezes the pool into a state where every
//! outstanding share is redeemed at the same rate:
//!
//! ```text
//! redemption_value_per_share = assets / total_shares
//! ```
//!
//! Because each exit pays `shares * assets / total_shares` and both the assets
//! and the shares fall by that same fraction, the per-share rate is invariant —
//! so a sequence of withdrawers all receive the identical (haircut) rate
//! instead of racing for the dwindling balance.
//!
//! # Inflows blocked, exits open
//!
//! New deposits are rejected through the same deposit gate withdraw-only mode
//! uses ([`VaultContract::require_no_sunset`] and
//! `require_not_sunsetting`), with the same `PoolShuttingDown` error
//! (`VaultError` has no free slot). Exits remain fully open and are priced at
//! the pro-rata rate above.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! instance storage, matching `balance.rs` and `withdraw_only_mode.rs`.

use soroban_sdk::{contractimpl, contracttype, symbol_short, token, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::constants::BPS_DENOMINATOR;
use crate::errors::VaultSafetyError;
use crate::storage::DataKey;
use crate::vault::VaultContract;

/// Instance-storage key for the insolvency-mode flag.
const MODE_KEY: Symbol = symbol_short!("insv_on");
/// Instance-storage key for the deficit recorded when the mode was entered.
const DEFICIT_KEY: Symbol = symbol_short!("insv_def");
/// Instance-storage key for the recovery ratio (bps) recorded on entry.
const RATIO_KEY: Symbol = symbol_short!("insv_rat");

/// Point-in-time insolvency snapshot (issue #833).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct InsolvencyStatus {
    /// Whether pro-rata (haircut) withdrawal mode is active.
    pub active: bool,
    /// The deficit (`surplus_or_deficit`) recorded when the mode was entered.
    /// Zero when the mode is not active.
    pub deficit: i128,
    /// The recovery ratio in basis points recorded on entry: assets / (principal
    /// + accrued rewards), capped at 10 000 (100%). Zero when not active.
    pub recovery_ratio_bps: u32,
}

/// Whether the pool is currently in pro-rata insolvency mode.
pub fn is_active(env: &Env) -> bool {
    env.storage().instance().get(&MODE_KEY).unwrap_or(false)
}

/// The recovery ratio (bps) recorded when the mode was entered, or 10 000
/// (100%, i.e. fully solvent) when the mode is not active.
pub fn recovery_ratio_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&RATIO_KEY)
        .unwrap_or(BPS_DENOMINATOR as u32)
}

/// The deficit recorded when the mode was entered (0 when not active).
pub fn recorded_deficit(env: &Env) -> i128 {
    env.storage().instance().get(&DEFICIT_KEY).unwrap_or(0)
}

/// Tokens the vault currently holds, read from the token contract.
fn assets_held(env: &Env) -> i128 {
    match env
        .storage()
        .instance()
        .get::<_, Address>(&DataKey::Token)
    {
        Some(addr) => token::Client::new(env, &addr).balance(&env.current_contract_address()),
        None => 0,
    }
}

/// Current relation between held assets and user liabilities, in the stake
/// token: `(token_balance, total_principal, accrued_unclaimed, surplus)`. A
/// negative `surplus` is a deficit. Mirrors the solvency accounting used
/// elsewhere: `token_balance - (total_principal + accrued_unclaimed)`.
fn solvency(env: &Env) -> (i128, i128, i128, i128) {
    let token_balance = assets_held(env);
    let total_principal = balance::get_total_deposited(env);
    let accrued_unclaimed = balance::get_total_accrued_unclaimed(env);
    let surplus = token_balance
        .saturating_sub(total_principal)
        .saturating_sub(accrued_unclaimed);
    (token_balance, total_principal, accrued_unclaimed, surplus)
}

/// Price `shares` for a withdrawal.
///
/// Outside insolvency mode this is the ordinary
/// [`balance::shares_to_amount`] (principal-backed) conversion. Inside the
/// mode it is the pro-rata `shares * assets / total_shares`, applied to every
/// withdrawer so they all realise the same rate. Returns `None` on overflow.
pub fn redemption_amount(env: &Env, shares: i128) -> Option<i128> {
    let total_shares = balance::get_total_shares(env);
    if !is_active(env) {
        let total_deposited = balance::get_total_deposited(env);
        return balance::shares_to_amount(total_shares, total_deposited, shares);
    }
    if total_shares <= 0 {
        return Some(0);
    }
    let assets = assets_held(env).max(0);
    crate::math::mul_div_floor(shares, assets, total_shares).ok()
}

#[contractimpl]
impl VaultContract {
    /// Admin or guardian: enter pro-rata insolvency mode (issue #833). Only
    /// allowed when the current solvency report shows a deficit, so the
    /// haircut is never applied to a solvent pool.
    ///
    /// # Authorization
    /// `caller` must sign and be either the stored admin or the stored
    /// guardian (when one is configured).
    ///
    /// # Errors
    /// - `Unauthorized`: `caller` is neither the admin nor the guardian.
    /// - `InsolvencyAlreadyActive`: the mode is already active.
    /// - `InsolvencyNotDetected`: the solvency report shows no deficit.
    ///
    /// # Events
    /// Emits `insolvency_entered` with `(deficit, recovery_ratio_bps, ledger)`.
    pub fn enter_insolvency_mode(env: Env, caller: Address) -> Result<(), VaultSafetyError> {
        admin::require_admin_or_guardian(&env, &caller)?;
        if is_active(&env) {
            return Err(VaultSafetyError::InsolvencyAlreadyActive);
        }
        let (token_balance, total_principal, accrued_unclaimed, surplus) = solvency(&env);
        if surplus >= 0 {
            return Err(VaultSafetyError::InsolvencyNotDetected);
        }

        let liabilities = total_principal.saturating_add(accrued_unclaimed);
        let ratio_bps = if liabilities <= 0 {
            BPS_DENOMINATOR as u32
        } else {
            let assets = token_balance.max(0);
            crate::math::mul_div_floor(assets, BPS_DENOMINATOR, liabilities)
                .unwrap_or(0)
                .clamp(0, BPS_DENOMINATOR) as u32
        };

        env.storage().instance().set(&MODE_KEY, &true);
        env.storage().instance().set(&DEFICIT_KEY, &surplus);
        env.storage().instance().set(&RATIO_KEY, &ratio_bps);

        env.events().publish(
            (Symbol::new(&env, "insolvency_entered"), caller.clone()),
            (surplus, ratio_bps, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Admin: leave pro-rata insolvency mode and reopen ordinary pricing and
    /// inflows (issue #833). Admin-only: lifting a haircut is a solvency
    /// decision the guardian cannot make unilaterally.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `InsolvencyNotActive`: the mode is not currently active.
    ///
    /// # Events
    /// Emits `insolvency_exited` with the ledger.
    pub fn exit_insolvency_mode(env: Env, admin: Address) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if !is_active(&env) {
            return Err(VaultSafetyError::InsolvencyNotActive);
        }
        env.storage().instance().remove(&MODE_KEY);
        env.storage().instance().remove(&DEFICIT_KEY);
        env.storage().instance().remove(&RATIO_KEY);
        env.events().publish(
            (Symbol::new(&env, "insolvency_exited"), admin.clone()),
            env.ledger().sequence(),
        );
        Ok(())
    }

    /// Read-only: whether pro-rata insolvency mode is active (issue #833).
    pub fn get_insolvency_mode(env: Env) -> bool {
        is_active(&env)
    }

    /// Read-only: the recorded deficit, recovery ratio, and active flag
    /// (issue #833).
    pub fn get_insolvency_status(env: Env) -> InsolvencyStatus {
        InsolvencyStatus {
            active: is_active(&env),
            deficit: recorded_deficit(&env),
            recovery_ratio_bps: recovery_ratio_bps(&env),
        }
    }
}
