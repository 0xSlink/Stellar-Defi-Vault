//! Node operator model with delegated staking and commission (issue #932).
//!
//! Stakers can delegate tokens to a registered node operator who runs a
//! strategy or earns rewards on behalf of the pool. Operators configure a
//! commission (capped at [`MAX_OPERATOR_COMMISSION_BPS`]) that is deducted
//! from delegator rewards and credited to the operator. Commission increases
//! are subject to an admin-configurable notice period, and undelegation is
//! subject to an admin-configurable cooldown.
//!
//! # Storage
//! Uses raw `Symbol`-keyed storage because `DataKey` and `VaultError` are at
//! Soroban's 50-variant cap.

use soroban_sdk::{
    contracterror, contractimpl, contracttype, symbol_short, token, Address, BytesN, Env, Symbol,
    Vec,
};

use crate::admin;
use crate::constants::LEDGERS_PER_DAY;
use crate::vault::{VaultContract, VaultContractClient};

/// Hard ceiling on operator commission in basis points (50% = 5,000 bps).
pub const MAX_OPERATOR_COMMISSION_BPS: u32 = 5_000;

/// Basis-point denominator (100% = 10,000 bps).
const BPS_DENOM: i128 = 10_000;

/// Fixed-point scale for reward-per-delegated-unit accumulator.
const REWARD_SCALE: i128 = 1_000_000_000_000;

/// Default notice period for commission increases (~1 day).
pub const DEFAULT_COMMISSION_NOTICE_LEDGERS: u32 = LEDGERS_PER_DAY;

/// Default undelegation cooldown (~1 day).
pub const DEFAULT_UNDELEGATION_COOLDOWN_LEDGERS: u32 = LEDGERS_PER_DAY;

const NOTICE_KEY: Symbol = symbol_short!("no_not");
const COOLDOWN_KEY: Symbol = symbol_short!("no_cool");
const OP_KEY: Symbol = symbol_short!("no_op");
const PENDING_COMM_KEY: Symbol = symbol_short!("no_pcom");
const DEL_POS_KEY: Symbol = symbol_short!("no_dpos");
const OP_DELS_KEY: Symbol = symbol_short!("no_dels");
const PENDING_UNDEL_KEY: Symbol = symbol_short!("no_pund");

/// Registered node operator state.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorInfo {
    pub operator: Address,
    pub commission_bps: u32,
    pub metadata_hash: BytesN<32>,
    pub total_delegated: i128,
    pub accrued_commission: i128,
    pub reward_index: i128,
    pub active: bool,
    pub registered_at: u32,
}

/// Pending commission increase waiting out its notice period.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCommissionChange {
    pub new_commission_bps: u32,
    pub effective_at: u32,
}

/// Per-(user, operator) delegation position.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationPosition {
    pub amount: i128,
    pub reward_debt_index: i128,
    pub claimable_rewards: i128,
}

/// Pending undelegation waiting out its cooldown period.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingUndelegation {
    pub operator: Address,
    pub amount: i128,
    pub unlock_at: u32,
}

/// Errors for node operator, delegation, commission, and reward operations.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OperatorError {
    NotInitialized = 1,
    Unauthorized = 2,
    OperatorAlreadyRegistered = 3,
    OperatorNotFound = 4,
    CommissionTooHigh = 5,
    ZeroAmount = 6,
    InsufficientDelegation = 7,
    NoticePeriodNotElapsed = 8,
    NoPendingCommissionChange = 9,
    CooldownNotElapsed = 10,
    NoPendingUndelegation = 11,
    PendingUndelegationExists = 12,
    ArithmeticError = 13,
}

pub(crate) fn get_operator_info(env: &Env, operator: &Address) -> Option<OperatorInfo> {
    env.storage()
        .persistent()
        .get(&(OP_KEY, operator.clone()))
}

pub(crate) fn set_operator_info(env: &Env, operator: &Address, info: &OperatorInfo) {
    env.storage()
        .persistent()
        .set(&(OP_KEY, operator.clone()), info);
}

fn get_delegation_pos(env: &Env, user: &Address, operator: &Address) -> DelegationPosition {
    env.storage()
        .persistent()
        .get(&(DEL_POS_KEY, user.clone(), operator.clone()))
        .unwrap_or(DelegationPosition {
            amount: 0,
            reward_debt_index: 0,
            claimable_rewards: 0,
        })
}

fn set_delegation_pos(
    env: &Env,
    user: &Address,
    operator: &Address,
    pos: &DelegationPosition,
) {
    env.storage()
        .persistent()
        .set(&(DEL_POS_KEY, user.clone(), operator.clone()), pos);
}

pub(crate) fn get_operator_delegators(env: &Env, operator: &Address) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&(OP_DELS_KEY, operator.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

fn add_operator_delegator(env: &Env, operator: &Address, user: &Address) {
    let mut list = get_operator_delegators(env, operator);
    if !list.iter().any(|a| &a == user) {
        list.push_back(user.clone());
        env.storage()
            .persistent()
            .set(&(OP_DELS_KEY, operator.clone()), &list);
    }
}

pub fn commission_notice_ledgers(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&NOTICE_KEY)
        .unwrap_or(DEFAULT_COMMISSION_NOTICE_LEDGERS)
}

pub fn undelegation_cooldown_ledgers(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&COOLDOWN_KEY)
        .unwrap_or(DEFAULT_UNDELEGATION_COOLDOWN_LEDGERS)
}

/// Settles accrued delegator rewards for `(user, operator)` up to `info.reward_index`.
pub(crate) fn settle_delegator_position(
    env: &Env,
    user: &Address,
    operator: &Address,
    info: &OperatorInfo,
) -> Result<DelegationPosition, OperatorError> {
    let mut pos = get_delegation_pos(env, user, operator);
    if pos.amount > 0 && info.reward_index > pos.reward_debt_index {
        let delta = info
            .reward_index
            .checked_sub(pos.reward_debt_index)
            .ok_or(OperatorError::ArithmeticError)?;
        let earned = pos
            .amount
            .checked_mul(delta)
            .ok_or(OperatorError::ArithmeticError)?
            .checked_div(REWARD_SCALE)
            .ok_or(OperatorError::ArithmeticError)?;
        pos.claimable_rewards = pos
            .claimable_rewards
            .checked_add(earned)
            .ok_or(OperatorError::ArithmeticError)?;
    }
    pos.reward_debt_index = info.reward_index;
    set_delegation_pos(env, user, operator, &pos);
    Ok(pos)
}

pub(crate) fn get_user_delegation_amount(env: &Env, user: &Address, operator: &Address) -> i128 {
    get_delegation_pos(env, user, operator).amount
}

pub(crate) fn apply_delegator_slash_and_refund(
    env: &Env,
    user: &Address,
    operator: &Address,
    info: &OperatorInfo,
    new_amount: i128,
    refund: i128,
) -> Result<(), OperatorError> {
    let mut pos = settle_delegator_position(env, user, operator, info)?;
    pos.amount = new_amount;
    pos.claimable_rewards = pos
        .claimable_rewards
        .checked_add(refund)
        .ok_or(OperatorError::ArithmeticError)?;
    set_delegation_pos(env, user, operator, &pos);
    Ok(())
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the notice period (in ledgers) required before a commission
    /// increase takes effect.
    pub fn set_commission_notice_period(
        env: Env,
        admin_addr: Address,
        ledgers: u32,
    ) -> Result<(), OperatorError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorError::NotInitialized)? {
            return Err(OperatorError::Unauthorized);
        }
        env.storage().instance().set(&NOTICE_KEY, &ledgers);
        Ok(())
    }

    /// Read-only: the configured commission-increase notice period, in ledgers.
    pub fn get_commission_notice_period(env: Env) -> u32 {
        commission_notice_ledgers(&env)
    }

    /// Admin: set the undelegation cooldown period, in ledgers.
    pub fn set_undelegation_cooldown(
        env: Env,
        admin_addr: Address,
        ledgers: u32,
    ) -> Result<(), OperatorError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorError::NotInitialized)? {
            return Err(OperatorError::Unauthorized);
        }
        env.storage().instance().set(&COOLDOWN_KEY, &ledgers);
        Ok(())
    }

    /// Read-only: the configured undelegation cooldown, in ledgers.
    pub fn get_undelegation_cooldown(env: Env) -> u32 {
        undelegation_cooldown_ledgers(&env)
    }

    /// Register `operator` with `commission_bps` (capped at
    /// `MAX_OPERATOR_COMMISSION_BPS`) and `metadata_hash`.
    pub fn register_operator(
        env: Env,
        operator: Address,
        commission_bps: u32,
        metadata_hash: BytesN<32>,
    ) -> Result<(), OperatorError> {
        operator.require_auth();
        if commission_bps > MAX_OPERATOR_COMMISSION_BPS {
            return Err(OperatorError::CommissionTooHigh);
        }
        if get_operator_info(&env, &operator).is_some() {
            return Err(OperatorError::OperatorAlreadyRegistered);
        }
        let info = OperatorInfo {
            operator: operator.clone(),
            commission_bps,
            metadata_hash,
            total_delegated: 0,
            accrued_commission: 0,
            reward_index: 0,
            active: true,
            registered_at: env.ledger().sequence(),
        };
        set_operator_info(&env, &operator, &info);
        Ok(())
    }

    /// Read-only: registered operator info, if any.
    pub fn get_operator(env: Env, operator: Address) -> Option<OperatorInfo> {
        get_operator_info(&env, &operator)
    }

    /// Operator: update commission. Decreases take effect immediately;
    /// increases are queued behind `commission_notice_ledgers`.
    pub fn propose_commission_change(
        env: Env,
        operator: Address,
        new_commission_bps: u32,
    ) -> Result<(), OperatorError> {
        operator.require_auth();
        if new_commission_bps > MAX_OPERATOR_COMMISSION_BPS {
            return Err(OperatorError::CommissionTooHigh);
        }
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;

        if new_commission_bps <= info.commission_bps {
            info.commission_bps = new_commission_bps;
            set_operator_info(&env, &operator, &info);
            env.storage()
                .persistent()
                .remove(&(PENDING_COMM_KEY, operator));
        } else {
            let effective_at = env
                .ledger()
                .sequence()
                .saturating_add(commission_notice_ledgers(&env));
            env.storage().persistent().set(
                &(PENDING_COMM_KEY, operator),
                &PendingCommissionChange {
                    new_commission_bps,
                    effective_at,
                },
            );
        }
        Ok(())
    }

    /// Apply a pending commission increase once its notice period has elapsed.
    pub fn apply_commission_change(env: Env, operator: Address) -> Result<(), OperatorError> {
        operator.require_auth();
        let pending: PendingCommissionChange = env
            .storage()
            .persistent()
            .get(&(PENDING_COMM_KEY, operator.clone()))
            .ok_or(OperatorError::NoPendingCommissionChange)?;
        if env.ledger().sequence() < pending.effective_at {
            return Err(OperatorError::NoticePeriodNotElapsed);
        }
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        info.commission_bps = pending.new_commission_bps;
        set_operator_info(&env, &operator, &info);
        env.storage()
            .persistent()
            .remove(&(PENDING_COMM_KEY, operator));
        Ok(())
    }

    /// Read-only: pending commission increase for `operator`, if any.
    pub fn get_pending_commission_change(
        env: Env,
        operator: Address,
    ) -> Option<PendingCommissionChange> {
        env.storage()
            .persistent()
            .get(&(PENDING_COMM_KEY, operator))
    }

    /// Delegate `amount` of vault tokens from `user` to `operator`.
    pub fn delegate_to_operator(
        env: Env,
        user: Address,
        operator: Address,
        amount: i128,
    ) -> Result<(), OperatorError> {
        user.require_auth();
        if amount <= 0 {
            return Err(OperatorError::ZeroAmount);
        }
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        if !info.active {
            return Err(OperatorError::OperatorNotFound);
        }

        let token_addr = Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &user,
            &env.current_contract_address(),
            &amount,
        );

        let mut pos = settle_delegator_position(&env, &user, &operator, &info)?;
        pos.amount = pos
            .amount
            .checked_add(amount)
            .ok_or(OperatorError::ArithmeticError)?;
        set_delegation_pos(&env, &user, &operator, &pos);
        add_operator_delegator(&env, &operator, &user);

        info.total_delegated = info
            .total_delegated
            .checked_add(amount)
            .ok_or(OperatorError::ArithmeticError)?;
        set_operator_info(&env, &operator, &info);
        Ok(())
    }

    /// Begin undelegating `amount` from `operator`. If a cooldown is
    /// configured, tokens are locked until `now + cooldown` and released via
    /// `withdraw_undelegated`; if cooldown is `0`, tokens are transferred back
    /// immediately.
    pub fn undelegate_from_operator(
        env: Env,
        user: Address,
        operator: Address,
        amount: i128,
    ) -> Result<(), OperatorError> {
        user.require_auth();
        if amount <= 0 {
            return Err(OperatorError::ZeroAmount);
        }
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        let mut pos = settle_delegator_position(&env, &user, &operator, &info)?;
        if pos.amount < amount {
            return Err(OperatorError::InsufficientDelegation);
        }

        pos.amount = pos
            .amount
            .checked_sub(amount)
            .ok_or(OperatorError::ArithmeticError)?;
        set_delegation_pos(&env, &user, &operator, &pos);

        info.total_delegated = info
            .total_delegated
            .checked_sub(amount)
            .ok_or(OperatorError::ArithmeticError)?;
        set_operator_info(&env, &operator, &info);

        let cooldown = undelegation_cooldown_ledgers(&env);
        if cooldown == 0 {
            let token_addr =
                Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
            token::Client::new(&env, &token_addr).transfer(
                &env.current_contract_address(),
                &user,
                &amount,
            );
        } else {
            if env
                .storage()
                .persistent()
                .has(&(PENDING_UNDEL_KEY, user.clone()))
            {
                return Err(OperatorError::PendingUndelegationExists);
            }
            let unlock_at = env.ledger().sequence().saturating_add(cooldown);
            env.storage().persistent().set(
                &(PENDING_UNDEL_KEY, user),
                &PendingUndelegation {
                    operator,
                    amount,
                    unlock_at,
                },
            );
        }
        Ok(())
    }

    /// Withdraw tokens from a completed undelegation once its cooldown has elapsed.
    pub fn withdraw_undelegated(env: Env, user: Address) -> Result<i128, OperatorError> {
        user.require_auth();
        let pending: PendingUndelegation = env
            .storage()
            .persistent()
            .get(&(PENDING_UNDEL_KEY, user.clone()))
            .ok_or(OperatorError::NoPendingUndelegation)?;
        if env.ledger().sequence() < pending.unlock_at {
            return Err(OperatorError::CooldownNotElapsed);
        }
        env.storage()
            .persistent()
            .remove(&(PENDING_UNDEL_KEY, user.clone()));
        let token_addr = Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &user,
            &pending.amount,
        );
        Ok(pending.amount)
    }

    /// Read-only: pending undelegation for `user`, if any.
    pub fn get_pending_undelegation(env: Env, user: Address) -> Option<PendingUndelegation> {
        env.storage().persistent().get(&(PENDING_UNDEL_KEY, user))
    }

    /// Read-only: `(delegated_amount, claimable_rewards)` for `(user, operator)`.
    pub fn get_delegation(env: Env, user: Address, operator: Address) -> (i128, i128) {
        let pos = get_delegation_pos(&env, &user, &operator);
        let pending = match get_operator_info(&env, &operator) {
            Some(info) if pos.amount > 0 && info.reward_index > pos.reward_debt_index => {
                let delta = info.reward_index - pos.reward_debt_index;
                pos.amount
                    .checked_mul(delta)
                    .and_then(|v| v.checked_div(REWARD_SCALE))
                    .unwrap_or(0)
            }
            _ => 0,
        };
        (pos.amount, pos.claimable_rewards.saturating_add(pending))
    }

    /// Distribute `total_reward` earned for `operator`'s delegators, deducting
    /// `operator.commission_bps` for the operator and crediting the remainder
    /// pro rata to delegators. Returns `(operator_commission, delegator_net)`.
    pub fn distribute_operator_rewards(
        env: Env,
        caller: Address,
        operator: Address,
        total_reward: i128,
    ) -> Result<(i128, i128), OperatorError> {
        caller.require_auth();
        if total_reward <= 0 {
            return Err(OperatorError::ZeroAmount);
        }
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        if info.total_delegated <= 0 {
            return Err(OperatorError::InsufficientDelegation);
        }

        let token_addr = Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &caller,
            &env.current_contract_address(),
            &total_reward,
        );

        let commission = total_reward
            .checked_mul(info.commission_bps as i128)
            .ok_or(OperatorError::ArithmeticError)?
            .checked_div(BPS_DENOM)
            .ok_or(OperatorError::ArithmeticError)?;
        let delegator_net = total_reward
            .checked_sub(commission)
            .ok_or(OperatorError::ArithmeticError)?;

        info.accrued_commission = info
            .accrued_commission
            .checked_add(commission)
            .ok_or(OperatorError::ArithmeticError)?;
        let index_delta = delegator_net
            .checked_mul(REWARD_SCALE)
            .ok_or(OperatorError::ArithmeticError)?
            .checked_div(info.total_delegated)
            .ok_or(OperatorError::ArithmeticError)?;
        info.reward_index = info
            .reward_index
            .checked_add(index_delta)
            .ok_or(OperatorError::ArithmeticError)?;
        set_operator_info(&env, &operator, &info);

        Ok((commission, delegator_net))
    }

    /// Delegator claims their net accrued rewards from `operator`.
    pub fn claim_delegator_rewards(
        env: Env,
        user: Address,
        operator: Address,
    ) -> Result<i128, OperatorError> {
        user.require_auth();
        let info = get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        let mut pos = settle_delegator_position(&env, &user, &operator, &info)?;
        let amount = pos.claimable_rewards;
        if amount <= 0 {
            return Err(OperatorError::ZeroAmount);
        }
        pos.claimable_rewards = 0;
        set_delegation_pos(&env, &user, &operator, &pos);

        let token_addr = Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &user,
            &amount,
        );
        Ok(amount)
    }

    /// Operator claims their accrued commission.
    pub fn claim_operator_commission(env: Env, operator: Address) -> Result<i128, OperatorError> {
        operator.require_auth();
        let mut info =
            get_operator_info(&env, &operator).ok_or(OperatorError::OperatorNotFound)?;
        let amount = info.accrued_commission;
        if amount <= 0 {
            return Err(OperatorError::ZeroAmount);
        }
        info.accrued_commission = 0;
        set_operator_info(&env, &operator, &info);

        let token_addr = Self::token_address(&env).map_err(|_| OperatorError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &operator,
            &amount,
        );
        Ok(amount)
    }
}
