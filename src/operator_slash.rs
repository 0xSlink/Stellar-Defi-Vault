//! Evidence-based operator slashing with appeal delay and pro-rata distribution (issue #933).
//!
//! Defines a narrow, evidence-backed slashing path for registered node
//! operators:
//! - `slash_operator(admin_or_governance, operator, amount_bps, evidence_hash)`
//!   is capped per incident (`MAX_SLASH_BPS_PER_INCIDENT`, or a lower admin
//!   cap via `set_max_slash_bps`) and queues the slash behind an appeal delay
//!   window.
//! - During the appeal window, the operator may call `appeal_operator_slash`
//!   with counter-evidence, which pauses execution until resolved by the admin.
//! - Once the appeal window elapses without an active appeal,
//!   `execute_operator_slash` deducts the slashed stake, allocates funds to
//!   the insurance reserve first (up to the configured insurance target /
//!   guarantor coverage), distributes any remainder pro rata to affected
//!   delegators as claimable rewards, and emits `operator_slashed`.
//!
//! # Storage
//! Uses raw `Symbol`-keyed storage because `DataKey` and `VaultError` are at
//! Soroban's 50-variant cap.

use soroban_sdk::{
    contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env, Symbol,
};

use crate::admin;
use crate::constants::LEDGERS_PER_DAY;
use crate::events;
use crate::insurance;
use crate::vault::node_operator;
use crate::vault::{VaultContract, VaultContractClient};

/// Hard per-incident slash cap in basis points (20% = 2,000 bps).
pub const MAX_SLASH_BPS_PER_INCIDENT: u32 = 2_000;

/// Basis-point denominator (100% = 10,000 bps).
const BPS_DENOM: i128 = 10_000;

/// Default appeal delay window (~1 day).
pub const DEFAULT_SLASH_APPEAL_LEDGERS: u32 = LEDGERS_PER_DAY;

const MAX_BPS_KEY: Symbol = symbol_short!("os_mbps");
const APPEAL_WIN_KEY: Symbol = symbol_short!("os_awin");
const INS_TARGET_KEY: Symbol = symbol_short!("os_itgt");
const NEXT_ID_KEY: Symbol = symbol_short!("os_next");
const SLASH_KEY: Symbol = symbol_short!("os_slsh");

/// Queued operator slash record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorSlashRecord {
    pub slash_id: u32,
    pub operator: Address,
    pub amount_bps: u32,
    pub slash_amount: i128,
    pub evidence_hash: BytesN<32>,
    pub proposed_at: u32,
    pub executable_at: u32,
    pub appealed: bool,
    pub counter_evidence_hash: Option<BytesN<32>>,
    pub executed: bool,
    pub cancelled: bool,
}

/// Errors for operator slashing and appeal entrypoints.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OperatorSlashError {
    NotInitialized = 1,
    Unauthorized = 2,
    OperatorNotFound = 3,
    InvalidSlashBps = 4,
    SlashCapExceeded = 5,
    NothingToSlash = 6,
    SlashNotFound = 7,
    AppealWindowClosed = 8,
    AppealWindowActive = 9,
    AlreadyAppealed = 10,
    SlashUnderAppeal = 11,
    NoActiveAppeal = 12,
    AlreadyExecuted = 13,
    AlreadyCancelled = 14,
    ArithmeticError = 15,
}

pub fn max_slash_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&MAX_BPS_KEY)
        .unwrap_or(MAX_SLASH_BPS_PER_INCIDENT)
        .min(MAX_SLASH_BPS_PER_INCIDENT)
}

pub fn slash_appeal_window_ledgers(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&APPEAL_WIN_KEY)
        .unwrap_or(DEFAULT_SLASH_APPEAL_LEDGERS)
}

pub fn slash_insurance_target(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&INS_TARGET_KEY)
        .unwrap_or(0)
        .max(insurance::get_coverage_amount(env))
}

fn get_slash_record(env: &Env, slash_id: u32) -> Option<OperatorSlashRecord> {
    env.storage().persistent().get(&(SLASH_KEY, slash_id))
}

fn set_slash_record(env: &Env, slash_id: u32, rec: &OperatorSlashRecord) {
    env.storage().persistent().set(&(SLASH_KEY, slash_id), rec);
}

fn require_admin_or_governance(env: &Env, caller: &Address) -> Result<(), OperatorSlashError> {
    caller.require_auth();
    let admin_addr = admin::get_admin(env).map_err(|_| OperatorSlashError::NotInitialized)?;
    if *caller != admin_addr && *caller != env.current_contract_address() {
        return Err(OperatorSlashError::Unauthorized);
    }
    Ok(())
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the per-incident slash cap in basis points (1..=`MAX_SLASH_BPS_PER_INCIDENT`).
    pub fn set_max_slash_bps(
        env: Env,
        admin_addr: Address,
        bps: u32,
    ) -> Result<(), OperatorSlashError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorSlashError::NotInitialized)? {
            return Err(OperatorSlashError::Unauthorized);
        }
        if bps == 0 || bps > MAX_SLASH_BPS_PER_INCIDENT {
            return Err(OperatorSlashError::SlashCapExceeded);
        }
        env.storage().instance().set(&MAX_BPS_KEY, &bps);
        Ok(())
    }

    /// Admin: set the appeal delay window (in ledgers) before a queued
    /// operator slash may be executed.
    pub fn set_slash_appeal_window(
        env: Env,
        admin_addr: Address,
        ledgers: u32,
    ) -> Result<(), OperatorSlashError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorSlashError::NotInitialized)? {
            return Err(OperatorSlashError::Unauthorized);
        }
        env.storage().instance().set(&APPEAL_WIN_KEY, &ledgers);
        Ok(())
    }

    /// Admin: set the minimum insurance reserve target filled first by
    /// operator slashes before remaining funds go to affected delegators.
    pub fn set_slash_insurance_target(
        env: Env,
        admin_addr: Address,
        target: i128,
    ) -> Result<(), OperatorSlashError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorSlashError::NotInitialized)? {
            return Err(OperatorSlashError::Unauthorized);
        }
        if target < 0 {
            return Err(OperatorSlashError::ArithmeticError);
        }
        env.storage().instance().set(&INS_TARGET_KEY, &target);
        Ok(())
    }

    /// Queue an evidence-based slash against `operator` for `amount_bps`
    /// (capped per incident). Subject to the appeal delay window before
    /// `execute_operator_slash` can finalize it.
    pub fn slash_operator(
        env: Env,
        admin_or_governance: Address,
        operator: Address,
        amount_bps: u32,
        evidence_hash: BytesN<32>,
    ) -> Result<u32, OperatorSlashError> {
        require_admin_or_governance(&env, &admin_or_governance)?;
        if amount_bps == 0 {
            return Err(OperatorSlashError::InvalidSlashBps);
        }
        if amount_bps > max_slash_bps(&env) {
            return Err(OperatorSlashError::SlashCapExceeded);
        }

        let info = node_operator::get_operator_info(&env, &operator)
            .ok_or(OperatorSlashError::OperatorNotFound)?;
        if info.total_delegated <= 0 {
            return Err(OperatorSlashError::NothingToSlash);
        }

        let slash_amount = info
            .total_delegated
            .checked_mul(amount_bps as i128)
            .ok_or(OperatorSlashError::ArithmeticError)?
            .checked_div(BPS_DENOM)
            .ok_or(OperatorSlashError::ArithmeticError)?;
        if slash_amount <= 0 {
            return Err(OperatorSlashError::NothingToSlash);
        }

        let slash_id: u32 = env.storage().instance().get(&NEXT_ID_KEY).unwrap_or(1);
        env.storage().instance().set(&NEXT_ID_KEY, &(slash_id + 1));

        let now = env.ledger().sequence();
        let executable_at = now.saturating_add(slash_appeal_window_ledgers(&env));
        set_slash_record(
            &env,
            slash_id,
            &OperatorSlashRecord {
                slash_id,
                operator,
                amount_bps,
                slash_amount,
                evidence_hash,
                proposed_at: now,
                executable_at,
                appealed: false,
                counter_evidence_hash: None,
                executed: false,
                cancelled: false,
            },
        );
        Ok(slash_id)
    }

    /// Operator: appeal a queued slash within the appeal window by submitting
    /// `counter_evidence_hash`.
    pub fn appeal_operator_slash(
        env: Env,
        operator: Address,
        slash_id: u32,
        counter_evidence_hash: BytesN<32>,
    ) -> Result<(), OperatorSlashError> {
        operator.require_auth();
        let mut rec = get_slash_record(&env, slash_id).ok_or(OperatorSlashError::SlashNotFound)?;
        if rec.operator != operator {
            return Err(OperatorSlashError::Unauthorized);
        }
        if rec.executed {
            return Err(OperatorSlashError::AlreadyExecuted);
        }
        if rec.cancelled {
            return Err(OperatorSlashError::AlreadyCancelled);
        }
        if rec.appealed {
            return Err(OperatorSlashError::AlreadyAppealed);
        }
        if env.ledger().sequence() >= rec.executable_at {
            return Err(OperatorSlashError::AppealWindowClosed);
        }

        rec.appealed = true;
        rec.counter_evidence_hash = Some(counter_evidence_hash);
        set_slash_record(&env, slash_id, &rec);
        Ok(())
    }

    /// Admin: resolve an appealed operator slash (`uphold = true` allows
    /// execution after the delay; `uphold = false` cancels the slash).
    pub fn resolve_operator_slash_appeal(
        env: Env,
        admin_addr: Address,
        slash_id: u32,
        uphold: bool,
    ) -> Result<(), OperatorSlashError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| OperatorSlashError::NotInitialized)? {
            return Err(OperatorSlashError::Unauthorized);
        }
        let mut rec = get_slash_record(&env, slash_id).ok_or(OperatorSlashError::SlashNotFound)?;
        if rec.executed {
            return Err(OperatorSlashError::AlreadyExecuted);
        }
        if rec.cancelled {
            return Err(OperatorSlashError::AlreadyCancelled);
        }
        if !rec.appealed {
            return Err(OperatorSlashError::NoActiveAppeal);
        }

        rec.appealed = false;
        if !uphold {
            rec.cancelled = true;
        }
        set_slash_record(&env, slash_id, &rec);
        Ok(())
    }

    /// Execute a queued operator slash once its appeal window has elapsed and
    /// no unresolved appeal is active. Slashed funds fill the insurance
    /// reserve first, and any remainder is distributed pro rata to affected
    /// delegators as claimable rewards. Returns `(to_insurance, to_delegators)`.
    pub fn execute_operator_slash(
        env: Env,
        slash_id: u32,
    ) -> Result<(i128, i128), OperatorSlashError> {
        let mut rec = get_slash_record(&env, slash_id).ok_or(OperatorSlashError::SlashNotFound)?;
        if rec.executed {
            return Err(OperatorSlashError::AlreadyExecuted);
        }
        if rec.cancelled {
            return Err(OperatorSlashError::AlreadyCancelled);
        }
        if rec.appealed {
            return Err(OperatorSlashError::SlashUnderAppeal);
        }
        let now = env.ledger().sequence();
        if now < rec.executable_at {
            return Err(OperatorSlashError::AppealWindowActive);
        }

        let mut info = node_operator::get_operator_info(&env, &rec.operator)
            .ok_or(OperatorSlashError::OperatorNotFound)?;
        let total_delegated = info.total_delegated;
        if total_delegated <= 0 {
            return Err(OperatorSlashError::NothingToSlash);
        }

        let slash_amount = rec.slash_amount.min(total_delegated);
        let target = slash_insurance_target(&env);
        let current_reserve = insurance::get_reserve(&env);
        let shortfall = target.saturating_sub(current_reserve).max(0);
        let to_insurance = slash_amount.min(shortfall);
        let to_delegators = slash_amount
            .checked_sub(to_insurance)
            .ok_or(OperatorSlashError::ArithmeticError)?;

        insurance::add_to_reserve(&env, to_insurance);

        let delegators = node_operator::get_operator_delegators(&env, &rec.operator);
        let mut total_deducted: i128 = 0;
        for d in delegators.iter() {
            let user_del = node_operator::get_user_delegation_amount(&env, &d, &rec.operator);
            if user_del <= 0 {
                continue;
            }
            let user_slash = user_del
                .checked_mul(slash_amount)
                .ok_or(OperatorSlashError::ArithmeticError)?
                .checked_div(total_delegated)
                .ok_or(OperatorSlashError::ArithmeticError)?;
            let user_refund = user_del
                .checked_mul(to_delegators)
                .ok_or(OperatorSlashError::ArithmeticError)?
                .checked_div(total_delegated)
                .ok_or(OperatorSlashError::ArithmeticError)?;
            let new_amount = user_del
                .checked_sub(user_slash)
                .ok_or(OperatorSlashError::ArithmeticError)?;
            node_operator::apply_delegator_slash_and_refund(
                &env,
                &d,
                &rec.operator,
                &info,
                new_amount,
                user_refund,
            )
            .map_err(|_| OperatorSlashError::ArithmeticError)?;
            total_deducted = total_deducted
                .checked_add(user_slash)
                .ok_or(OperatorSlashError::ArithmeticError)?;
        }

        info.total_delegated = info.total_delegated.saturating_sub(total_deducted);
        node_operator::set_operator_info(&env, &rec.operator, &info);

        rec.executed = true;
        set_slash_record(&env, slash_id, &rec);

        events::operator_slashed(
            &env,
            &rec.operator,
            slash_id,
            rec.amount_bps,
            slash_amount,
            to_insurance,
            to_delegators,
            &rec.evidence_hash,
            now,
        );

        Ok((to_insurance, to_delegators))
    }

    /// Read-only: queued or executed operator slash record.
    pub fn get_operator_slash(env: Env, slash_id: u32) -> Option<OperatorSlashRecord> {
        get_slash_record(&env, slash_id)
    }
}
