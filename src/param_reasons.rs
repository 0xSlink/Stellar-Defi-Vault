//! Reason strings on admin parameter changes.
//!
//! Each `*_with_reason` setter takes a short justification (max
//! `MAX_REASON_LEN` bytes) in addition to the normal arguments. It applies the
//! change through the existing setter's logic, then records the reason in both
//! the `params_updated` event and the `pool_parameter_change_log` entry, so the
//! change explains itself.
//!
//! Sensitive setters (reward rates, fees and caps) reject an empty reason with
//! `EmptyReason`; the rest accept an empty one. The original setters without a
//! reason are unchanged.

use soroban_sdk::{contractimpl, Address, Env, String, Symbol};

use crate::admin;
use crate::balance;
use crate::errors::ParamReasonError;
use crate::max_deposit_cap::DepositCapError;
use crate::storage::DataKey;
use crate::vault::VaultContract;

/// Maximum length of a reason string, in bytes.
pub const MAX_REASON_LEN: u32 = 128;

impl From<DepositCapError> for ParamReasonError {
    fn from(err: DepositCapError) -> Self {
        match err {
            DepositCapError::Unauthorized => ParamReasonError::Unauthorized,
            DepositCapError::NotInitialized => ParamReasonError::NotInitialized,
            _ => ParamReasonError::InvalidValue,
        }
    }
}

/// Check the reason's length and, for sensitive setters, that it is non-empty.
pub fn validate_reason(reason: &String, sensitive: bool) -> Result<(), ParamReasonError> {
    if reason.len() > MAX_REASON_LEN {
        return Err(ParamReasonError::ReasonTooLong);
    }
    if sensitive && reason.len() == 0 {
        return Err(ParamReasonError::EmptyReason);
    }
    Ok(())
}

/// Record an applied change in the change log and emit `params_updated`.
///
/// Event topic `("params_updated",)`, data
/// `(changed_by, parameter, old_value, new_value, reason, ledger)`.
fn record_change(
    env: &Env,
    admin: &Address,
    parameter: &str,
    old_value: i128,
    new_value: i128,
    reason: &String,
) {
    crate::vault_extensions_463_466::log_parameter_change_with_reason(
        env, admin, parameter, old_value, new_value, reason,
    );
    env.events().publish(
        (Symbol::new(env, "params_updated"),),
        (
            admin.clone(),
            String::from_str(env, parameter),
            old_value,
            new_value,
            reason.clone(),
            env.ledger().sequence(),
        ),
    );
}

#[contractimpl]
impl VaultContract {
    /// `set_reward_rate_bps` with a mandatory reason (sensitive).
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `Unauthorized` / `NotInitialized`: admin check failed.
    /// - `EmptyReason` / `ReasonTooLong`: invalid reason.
    /// - `InvalidValue`: the rate was rejected by the underlying setter.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "reward_rate_bps", old, new, reason, ledger)`.
    pub fn set_reward_rate_bps_with_reason(
        env: Env,
        admin: Address,
        rate_bps: u32,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, true)?;
        let old = balance::get_reward_rate_bps(&env);
        Self::set_reward_rate_bps(env.clone(), rate_bps)?;
        record_change(&env, &admin, "reward_rate_bps", old as i128, rate_bps as i128, &reason);
        Ok(())
    }

    /// `set_unstake_fee_bps` with a mandatory reason (sensitive). A fee
    /// increase may be queued by the fee-notice period; the log and event
    /// record the requested value either way.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// See `set_reward_rate_bps_with_reason`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "unstake_fee_bps", old, new, reason, ledger)`.
    pub fn set_unstake_fee_bps_with_reason(
        env: Env,
        admin: Address,
        bps: u32,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, true)?;
        let old = balance::get_unstake_fee_bps(&env);
        crate::fee_change_notice::propose_fee_change(&env, bps)?;
        record_change(&env, &admin, "unstake_fee_bps", old as i128, bps as i128, &reason);
        Ok(())
    }

    /// Claim-fee setter with a mandatory reason (sensitive).
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// See `set_reward_rate_bps_with_reason`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "claim_fee_bps", old, new, reason, ledger)`.
    pub fn set_claim_fee_bps_with_reason(
        env: Env,
        admin: Address,
        fee_bps: u32,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, true)?;
        let old = crate::claim_fee::get_claim_fee_bps_internal(&env);
        crate::claim_fee::set_claim_fee_bps(env.clone(), admin.clone(), fee_bps)?;
        record_change(&env, &admin, "claim_fee_bps", old as i128, fee_bps as i128, &reason);
        Ok(())
    }

    /// `set_max_deposit_cap` with a mandatory reason (sensitive).
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// See `set_reward_rate_bps_with_reason`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "max_deposit_cap", old, new, reason, ledger)`.
    pub fn set_max_deposit_cap_with_reason(
        env: Env,
        admin: Address,
        amount: i128,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, true)?;
        let old = crate::max_deposit_cap::get_cap(&env);
        Self::set_max_deposit_cap(env.clone(), admin.clone(), amount)?;
        record_change(&env, &admin, "max_deposit_cap", old, amount, &reason);
        Ok(())
    }

    /// `set_claim_cap` with a mandatory reason (sensitive). The log records the
    /// per-window cap amount; the window length is not logged.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// See `set_reward_rate_bps_with_reason`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "claim_cap", old, new, reason, ledger)`.
    pub fn set_claim_cap_with_reason(
        env: Env,
        admin: Address,
        max_amount: i128,
        window_ledgers: u32,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, true)?;
        let old = balance::get_claim_cap(&env);
        Self::set_claim_cap(env.clone(), admin.clone(), max_amount, window_ledgers)?;
        record_change(&env, &admin, "claim_cap", old, max_amount, &reason);
        Ok(())
    }

    /// `set_min_claim_amount` with a reason. Not sensitive: the reason may be
    /// empty.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `Unauthorized` / `NotInitialized`, `ReasonTooLong`, `InvalidValue`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "min_claim_amount", old, new, reason, ledger)`.
    pub fn set_min_claim_amount_with_reason(
        env: Env,
        admin: Address,
        amount: i128,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, false)?;
        let old = crate::min_claim_amount::get_min_claim(&env);
        Self::set_min_claim_amount(env.clone(), admin.clone(), amount)?;
        record_change(&env, &admin, "min_claim_amount", old, amount, &reason);
        Ok(())
    }

    /// `set_vesting_period` with a reason. Not sensitive: the reason may be
    /// empty.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `Unauthorized` / `NotInitialized`, `ReasonTooLong`.
    ///
    /// # Events
    /// - `params_updated` with `(admin, "vesting_period", old, new, reason, ledger)`.
    pub fn set_vesting_period_with_reason(
        env: Env,
        admin: Address,
        ledgers: u32,
        reason: String,
    ) -> Result<(), ParamReasonError> {
        admin::require_admin_as(&env, &admin)?;
        validate_reason(&reason, false)?;
        let old: u32 = env
            .storage()
            .instance()
            .get(&DataKey::VestingPeriod)
            .unwrap_or(0);
        Self::set_vesting_period(env.clone(), admin.clone(), ledgers)?;
        record_change(&env, &admin, "vesting_period", old as i128, ledgers as i128, &reason);
        Ok(())
    }
}
