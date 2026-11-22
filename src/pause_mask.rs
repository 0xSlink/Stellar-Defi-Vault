//! Function-level pause bitmask.
//!
//! Instead of a single all-or-nothing pause, each group of entrypoints has its
//! own bit, so an incident can disable (say) deposits and claims while leaving
//! withdrawals open.
//!
//! | Bit | Constant           | Gated entrypoints                                          |
//! |-----|--------------------|------------------------------------------------------------|
//! | 0   | `PAUSE_DEPOSIT`    | `stake`, `deposit`, `stake_for`, `gift_stake`, `flash_stake`, `stake_and_claim` |
//! | 1   | `PAUSE_WITHDRAW`   | `withdraw`, `unstake`, `unstake_all`, `withdraw_to`, `execute_withdrawal`, ... |
//! | 2   | `PAUSE_CLAIM`      | `claim`, `claim_partial`, `claim_in_token`, `claim_vesting`, `zap_compound`, auto-claim on withdraw |
//! | 3   | `PAUSE_TRANSFER`   | `transfer_position`, position moves, `accept_swap_offer`    |
//! | 4   | `PAUSE_FUNDING`    | `add_yield`                                                 |
//! | 5   | `PAUSE_QUIZ`       | `add_quiz`, `submit_quiz_answer`                            |
//!
//! `PAUSE_ALL` (`0x3F`) sets every bit.
//!
//! # Relationship to the existing pause
//!
//! The effective mask returned by `get_pause_mask` is the union of:
//!
//! - the bits stored by `set_pause_mask`,
//! - `PAUSE_ALL` while the legacy pause flag is set, so `pause()` (and the
//!   guardian, circuit-breaker and scheduled pauses that set the same flag) is
//!   exactly "all bits set", and
//! - `PAUSE_DEPOSIT` / `PAUSE_WITHDRAW` while the legacy `pause_deposits` /
//!   `pause_withdrawals` flags are set.
//!
//! `unpause()` clears the stored bits as well as the flag, so it is exactly
//! "no bits set". `set_pause_mask` is authoritative: it replaces the stored
//! bits, sets or clears the legacy flag to match, and clears the legacy
//! deposit/withdrawal flags.
//!
//! # Authorization
//!
//! The admin may set any mask. The guardian may only *add* bits (it can pause,
//! never resume), matching its role in `guardian_pause`.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the stored bits are a raw
//! `Symbol`-keyed instance entry, matching `balance.rs`.

use soroban_sdk::{contracterror, contractimpl, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::errors::VaultError;
use crate::storage::{DataKey, PauseReason};
use crate::vault::VaultContract;

/// Deposits: `stake`, `deposit`, `stake_for`, `gift_stake`, `flash_stake`, `stake_and_claim`.
pub const PAUSE_DEPOSIT: u32 = 1 << 0;
/// Withdrawals: every path through the unstake core.
pub const PAUSE_WITHDRAW: u32 = 1 << 1;
/// Reward claims, including `zap_compound` and auto-claim on withdraw.
pub const PAUSE_CLAIM: u32 = 1 << 2;
/// Position transfers and swap offers.
pub const PAUSE_TRANSFER: u32 = 1 << 3;
/// Reward funding (`add_yield`).
pub const PAUSE_FUNDING: u32 = 1 << 4;
/// Stake-to-learn quizzes.
pub const PAUSE_QUIZ: u32 = 1 << 5;
/// Every defined bit; what `pause()` amounts to.
pub const PAUSE_ALL: u32 = PAUSE_DEPOSIT
    | PAUSE_WITHDRAW
    | PAUSE_CLAIM
    | PAUSE_TRANSFER
    | PAUSE_FUNDING
    | PAUSE_QUIZ;

const MASK_KEY: Symbol = symbol_short!("pse_mask");
// Legacy per-direction flags owned by `vault_extensions_490_493`.
const LEGACY_DEPOSITS_KEY: Symbol = symbol_short!("p_dep");
const LEGACY_WITHDRAWALS_KEY: Symbol = symbol_short!("p_with");

/// Errors for `set_pause_mask`. `VaultError` is at Soroban's 50-variant cap,
/// so these get their own enum.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PauseMaskError {
    /// Caller is neither the admin nor the guardian, or the guardian tried to
    /// clear a bit.
    Unauthorized = 1,
    /// No admin is stored.
    NotInitialized = 2,
    /// `mask` sets a bit outside `PAUSE_ALL`.
    InvalidMask = 3,
    /// The contract has been permanently stopped.
    ContractStopped = 4,
    /// A forced unpause recently happened; raising the pause is blocked for
    /// the cooldown (same rule as `pause()`).
    RepauseBlocked = 5,
}

impl From<VaultError> for PauseMaskError {
    fn from(err: VaultError) -> Self {
        match err {
            VaultError::NotInitialized | VaultError::NoAdmin => PauseMaskError::NotInitialized,
            VaultError::ContractStopped => PauseMaskError::ContractStopped,
            _ => PauseMaskError::Unauthorized,
        }
    }
}

fn stored_mask(env: &Env) -> u32 {
    env.storage().instance().get(&MASK_KEY).unwrap_or(0)
}

/// Drop the stored bits. Called by `unpause`.
pub(crate) fn clear_stored(env: &Env) {
    env.storage().instance().remove(&MASK_KEY);
}

/// The effective pause mask: stored bits, plus `PAUSE_ALL` under the legacy
/// pause flag, plus the legacy deposit / withdrawal flags.
pub fn effective_mask(env: &Env) -> u32 {
    // Lazily lift a `pause_until` pause that is due, as `is_paused` does.
    balance::apply_scheduled_unpause_if_due(env);
    let mut mask = stored_mask(env);
    if env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
    {
        mask |= PAUSE_ALL;
    }
    if env
        .storage()
        .instance()
        .get(&LEGACY_DEPOSITS_KEY)
        .unwrap_or(false)
    {
        mask |= PAUSE_DEPOSIT;
    }
    if env
        .storage()
        .instance()
        .get(&LEGACY_WITHDRAWALS_KEY)
        .unwrap_or(false)
    {
        mask |= PAUSE_WITHDRAW;
    }
    mask
}

/// Revert with `VaultPaused` if any bit in `bits` is paused.
pub(crate) fn require_not_paused(env: &Env, bits: u32) -> Result<(), VaultError> {
    if effective_mask(env) & bits != 0 {
        Err(VaultError::VaultPaused)
    } else {
        Ok(())
    }
}

/// Whether any bit in `bits` is paused.
pub(crate) fn is_paused(env: &Env, bits: u32) -> bool {
    effective_mask(env) & bits != 0
}

#[contractimpl]
impl VaultContract {
    /// Set the pause bitmask: each set bit disables one group of entrypoints
    /// (see the table in the module docs). `0` resumes everything, `PAUSE_ALL`
    /// (`0x3F`) is a full pause.
    ///
    /// The admin may set any mask. The guardian may only raise it: a mask that
    /// clears any currently paused bit is rejected with `Unauthorized`.
    ///
    /// # Authorization
    /// `caller` must sign and be the admin or the guardian.
    ///
    /// # Errors
    /// - `InvalidMask`: `mask` has bits outside `PAUSE_ALL`.
    /// - `Unauthorized`: caller is neither, or the guardian tried to unpause.
    /// - `ContractStopped`: the contract was permanently stopped.
    /// - `RepauseBlocked`: raising the pause inside the post-force-unpause cooldown.
    ///
    /// # Events
    /// Emits `pmask_set` with `(old_mask, new_mask, ledger)`.
    pub fn set_pause_mask(env: Env, caller: Address, mask: u32) -> Result<(), PauseMaskError> {
        caller.require_auth();
        if mask & !PAUSE_ALL != 0 {
            return Err(PauseMaskError::InvalidMask);
        }
        let is_admin = admin::get_admin(&env)? == caller;
        let is_guardian = crate::guardian_pause::get_guardian(&env).as_ref() == Some(&caller);
        if !is_admin && !is_guardian {
            return Err(PauseMaskError::Unauthorized);
        }
        Self::require_not_stopped(&env)?;

        let old = effective_mask(&env);
        // The guardian can pause but never resume.
        if !is_admin && mask & old != old {
            return Err(PauseMaskError::Unauthorized);
        }
        // Same re-pause cooldown `pause()` honours after a forced unpause.
        if mask & !old != 0 && crate::pause_grace_period::repause_blocked(&env) {
            return Err(PauseMaskError::RepauseBlocked);
        }

        let now = env.ledger().sequence();
        // The mask is now authoritative over the legacy per-direction flags.
        env.storage().instance().remove(&LEGACY_DEPOSITS_KEY);
        env.storage().instance().remove(&LEGACY_WITHDRAWALS_KEY);

        if mask == PAUSE_ALL {
            // A full pause is the legacy flag, so every legacy pauser agrees.
            clear_stored(&env);
            if !env
                .storage()
                .instance()
                .get(&DataKey::Paused)
                .unwrap_or(false)
            {
                env.storage().instance().set(&DataKey::Paused, &true);
                balance::set_pause_info(
                    &env,
                    &crate::storage::PauseInfo {
                        reason: PauseReason::SecurityIncident,
                        message: soroban_sdk::String::from_str(&env, "pause mask"),
                        paused_at: now,
                    },
                );
            }
        } else {
            env.storage().instance().set(&DataKey::Paused, &false);
            balance::clear_pause_info(&env);
            balance::clear_scheduled_unpause(&env);
            if mask == 0 {
                clear_stored(&env);
            } else {
                env.storage().instance().set(&MASK_KEY, &mask);
            }
        }

        balance::set_last_updated_ledger(&env, now);
        env.events()
            .publish((symbol_short!("pmask_set"), caller), (old, mask, now));
        Ok(())
    }

    /// Read-only: the effective pause mask. Bit `n` set means that function
    /// group is paused; see the table in the `pause_mask` module docs.
    pub fn get_pause_mask(env: Env) -> u32 {
        effective_mask(&env)
    }
}
