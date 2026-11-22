//! Protocol take rate on claimed rewards (issue #934).
//!
//! Separate from deposit and unstake fees, this module charges an
//! admin-configured percentage (`reward_fee_bps`) on rewards at claim/compound
//! time and routes the collected fee through the configured treasury split
//! (`set_treasury_split`), falling back to fee recipients, slash treasury, or
//! admin when no split is configured.
//!
//! # Safety guards
//! - Hard ceiling: `MAX_REWARD_FEE_BPS` (2,000 bps = 20%) can never be exceeded.
//! - Notice period for increases: when `reward_fee_notice_ledgers > 0` (or
//!   `fee_change_notice::get_fee_notice_ledgers > 0`), any increase is queued
//!   as a `PendingRewardFeeChange` and only takes effect once the notice
//!   period has elapsed. Decreases take effect immediately.

use soroban_sdk::{contractimpl, contracttype, symbol_short, token, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::constants::BPS_DENOMINATOR;
use crate::errors::VaultError;
use crate::vault::VaultContract;

/// Hard ceiling for the reward fee: 2,000 bps (20%).
pub const MAX_REWARD_FEE_BPS: u32 = 2_000;

const REWARD_FEE_BPS_KEY: Symbol = symbol_short!("rew_fee");
const REWARD_FEE_NOTICE_KEY: Symbol = symbol_short!("rf_delay");
const PENDING_REWARD_FEE_KEY: Symbol = symbol_short!("rf_pend");

/// A queued reward-fee increase awaiting its notice period.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PendingRewardFeeChange {
    /// New reward fee in basis points.
    pub new_bps: u32,
    /// Earliest ledger sequence at which `new_bps` becomes active.
    pub effective_at: u32,
}

/// Read the stored reward fee in basis points (without settling pending).
pub fn get_reward_fee_bps_internal(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&REWARD_FEE_BPS_KEY)
        .unwrap_or(0)
}

fn set_reward_fee_bps_storage(env: &Env, bps: u32) {
    env.storage().instance().set(&REWARD_FEE_BPS_KEY, &bps);
}

/// Configured notice delay in ledgers for reward-fee increases.
/// Falls back to `fee_change_notice::get_fee_notice_ledgers` if not
/// separately configured.
pub fn get_reward_fee_notice_ledgers_internal(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&REWARD_FEE_NOTICE_KEY)
        .unwrap_or_else(|| crate::fee_change_notice::get_fee_notice_ledgers(env))
}

/// Settle any matured pending reward-fee increase.
pub fn settle_pending_reward_fee(env: &Env) {
    if let Some(pending) = env
        .storage()
        .instance()
        .get::<_, PendingRewardFeeChange>(&PENDING_REWARD_FEE_KEY)
    {
        if env.ledger().sequence() >= pending.effective_at {
            set_reward_fee_bps_storage(env, pending.new_bps);
            env.storage().instance().remove(&PENDING_REWARD_FEE_KEY);
            env.events().publish(
                (symbol_short!("rf_apld"),),
                (pending.new_bps, env.ledger().sequence()),
            );
        }
    }
}

/// Route `fee_amount` of `reward_token_addr` through the configured treasury
/// split, or fallback recipients if no split is set.
fn route_reward_fee(
    env: &Env,
    reward_token_addr: &Address,
    fee_amount: i128,
) -> Result<(), VaultError> {
    if fee_amount <= 0 {
        return Ok(());
    }

    if let Some(split) = VaultContract::get_treasury_split(env.clone()) {
        let token_client = token::Client::new(env, reward_token_addr);
        let mut distributed_to_rest: i128 = 0;
        let len = split.recipients.len();
        for i in 1..len {
            let recipient = split.recipients.get(i).unwrap();
            let bps = split.bps_shares.get(i).unwrap();
            let share = fee_amount
                .checked_mul(bps as i128)
                .ok_or(VaultError::ArithmeticError)?
                .checked_div(BPS_DENOMINATOR)
                .ok_or(VaultError::ArithmeticError)?;
            distributed_to_rest = distributed_to_rest
                .checked_add(share)
                .ok_or(VaultError::ArithmeticError)?;
            if share > 0 {
                token_client.transfer(&env.current_contract_address(), &recipient, &share);
            }
        }
        if len > 0 {
            let first = split.recipients.get(0).unwrap();
            let first_share = fee_amount
                .checked_sub(distributed_to_rest)
                .ok_or(VaultError::ArithmeticError)?;
            if first_share > 0 {
                token_client.transfer(&env.current_contract_address(), &first, &first_share);
            }
        }
        return Ok(());
    }

    let recipients = balance::get_fee_recipients(env);
    if !recipients.is_empty() {
        VaultContract::distribute_fee(env, reward_token_addr, fee_amount, &recipients);
        return Ok(());
    }

    let fallback = match balance::get_slash_treasury(env) {
        Some(treasury) => treasury,
        None => admin::get_admin(env)?,
    };
    token::Client::new(env, reward_token_addr).transfer(
        &env.current_contract_address(),
        &fallback,
        &fee_amount,
    );
    Ok(())
}

/// Apply the protocol reward take rate to `reward_before_fee`.
///
/// Returns `(net_reward, fee_amount)`. When `reward_fee_bps == 0` or
/// `reward_before_fee <= 0`, returns `(reward_before_fee, 0)` without mutating
/// state or emitting fee events.
pub fn apply_reward_fee(
    env: &Env,
    user: &Address,
    reward_token_addr: &Address,
    reward_before_fee: i128,
) -> Result<(i128, i128), VaultError> {
    settle_pending_reward_fee(env);

    let fee_bps = get_reward_fee_bps_internal(env);
    if fee_bps == 0
        || reward_before_fee <= 0
        || crate::vault::withdraw_only_mode::is_active(env)
    {
        return Ok((reward_before_fee, 0));
    }

    let fee_amount = reward_before_fee
        .checked_mul(fee_bps as i128)
        .ok_or(VaultError::ArithmeticError)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or(VaultError::ArithmeticError)?;
    if fee_amount == 0 {
        return Ok((reward_before_fee, 0));
    }

    let net_reward = reward_before_fee
        .checked_sub(fee_amount)
        .ok_or(VaultError::ArithmeticError)?;

    route_reward_fee(env, reward_token_addr, fee_amount)?;

    env.events().publish(
        (symbol_short!("rew_fee"), user.clone()),
        (
            reward_before_fee,
            fee_amount,
            net_reward,
            env.ledger().sequence(),
        ),
    );

    Ok((net_reward, fee_amount))
}

#[contractimpl]
impl VaultContract {
    /// Admin: configure the protocol take rate on claimed/compounded rewards
    /// in basis points, subject to `MAX_REWARD_FEE_BPS` and the notice period
    /// for increases.
    pub fn set_reward_fee_bps(env: Env, admin_addr: Address, bps: u32) -> Result<(), VaultError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env)? {
            return Err(VaultError::Unauthorized);
        }
        if bps > MAX_REWARD_FEE_BPS {
            return Err(VaultError::UnstakeFeeTooHigh);
        }

        settle_pending_reward_fee(&env);

        let current_bps = get_reward_fee_bps_internal(&env);
        let notice_ledgers = get_reward_fee_notice_ledgers_internal(&env);

        if bps <= current_bps || notice_ledgers == 0 {
            set_reward_fee_bps_storage(&env, bps);
            env.storage().instance().remove(&PENDING_REWARD_FEE_KEY);
            env.events().publish(
                (symbol_short!("rf_set"),),
                (bps, env.ledger().sequence()),
            );
        } else {
            let effective_at = env.ledger().sequence().saturating_add(notice_ledgers);
            let pending = PendingRewardFeeChange {
                new_bps: bps,
                effective_at,
            };
            env.storage()
                .instance()
                .set(&PENDING_REWARD_FEE_KEY, &pending);
            env.events()
                .publish((symbol_short!("rf_prop"),), (bps, effective_at));
        }

        crate::ttl_management::bump(&env);
        Ok(())
    }

    /// Read-only: current active reward fee in basis points.
    pub fn get_reward_fee_bps(env: Env) -> u32 {
        if let Some(pending) = env
            .storage()
            .instance()
            .get::<_, PendingRewardFeeChange>(&PENDING_REWARD_FEE_KEY)
        {
            if env.ledger().sequence() >= pending.effective_at {
                return pending.new_bps;
            }
        }
        get_reward_fee_bps_internal(&env)
    }

    /// Admin: set the notice period (in ledgers) required before a reward-fee
    /// increase takes effect.
    pub fn set_reward_fee_notice_ledgers(
        env: Env,
        admin_addr: Address,
        ledgers: u32,
    ) -> Result<(), VaultError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env)? {
            return Err(VaultError::Unauthorized);
        }
        env.storage()
            .instance()
            .set(&REWARD_FEE_NOTICE_KEY, &ledgers);
        crate::ttl_management::bump(&env);
        Ok(())
    }

    /// Read-only: configured notice period (in ledgers) for reward-fee increases.
    pub fn get_reward_fee_notice_ledgers(env: Env) -> u32 {
        get_reward_fee_notice_ledgers_internal(&env)
    }

    /// Read-only: pending reward-fee increase, if any.
    pub fn get_pending_reward_fee_change(env: Env) -> Option<PendingRewardFeeChange> {
        env.storage().instance().get(&PENDING_REWARD_FEE_KEY)
    }
}
