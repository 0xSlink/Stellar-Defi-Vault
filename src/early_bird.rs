//! Early depositor bonus for the first N unique depositors (issue #871).
//!
//! Launch campaigns want to reward the earliest users. The admin configures a
//! cap `max_depositors`, a `bonus_bps` multiplier and a `duration_ledgers`
//! window. On an address's **first** deposit it is assigned a sequence number;
//! while `sequence <= max_depositors` and the bonus has not yet expired
//! (`first_deposit_ledger + duration_ledgers`), every deposit that address
//! makes mints extra shares at `bonus_bps / 10_000` of the base amount.
//!
//! The record is written once and never cleared, so a full withdrawal followed
//! by a re-deposit keeps the original sequence and cannot regain an expired
//! bonus or jump the queue for a slot that was already handed out.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`.

use soroban_sdk::{contractimpl, contracttype, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::constants::{BPS_DENOMINATOR, BPS_DENOMINATOR_U32};
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Number of unique depositors that qualify for the launch bonus.
const MAX_KEY: Symbol = symbol_short!("eb_max");
/// Bonus multiplier in basis points of the minted shares.
const BPS_KEY: Symbol = symbol_short!("eb_bps");
/// Ledgers the bonus stays live after an address's first deposit.
const DUR_KEY: Symbol = symbol_short!("eb_dur");
/// Number of addresses that have been assigned a sequence number.
const COUNT_KEY: Symbol = symbol_short!("eb_cnt");
/// Per-address prefix for the eligibility record.
const USER_KEY: Symbol = symbol_short!("eb_usr");

/// Hard ceiling on the bonus multiplier: 100% (10 000 bps).
const MAX_BONUS_BPS: u32 = BPS_DENOMINATOR_U32;

/// An address's immutable early-bird assignment.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct EarlyBirdRecord {
    /// 1-based order in which this address first deposited.
    pub sequence: u32,
    /// Ledger of this address's first deposit.
    pub first_deposit_ledger: u32,
}

/// Read-only view of an address's early-bird standing (issue #871).
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct EarlyBirdStatus {
    /// Whether a campaign is currently configured.
    pub configured: bool,
    /// Whether the address claimed one of the first `max_depositors` slots.
    pub eligible: bool,
    /// 1-based first-deposit order (0 if the address has never deposited).
    pub sequence: u32,
    /// Ledger at which the bonus expires (0 if never recorded).
    pub expires_at: u32,
    /// Whether the bonus applies to a deposit made now.
    pub active: bool,
}

/// The configured number of qualifying depositors (0 when unset).
pub fn max_depositors(env: &Env) -> u32 {
    env.storage().instance().get(&MAX_KEY).unwrap_or(0)
}

/// The configured bonus multiplier in basis points (0 when unset).
pub fn bonus_bps(env: &Env) -> u32 {
    env.storage().instance().get(&BPS_KEY).unwrap_or(0)
}

/// The configured bonus duration in ledgers (0 when unset).
pub fn duration_ledgers(env: &Env) -> u32 {
    env.storage().instance().get(&DUR_KEY).unwrap_or(0)
}

fn user_key(user: &Address) -> (Symbol, Address) {
    (USER_KEY, user.clone())
}

/// The address's stored early-bird record, if it has ever deposited.
pub fn record_of(env: &Env, user: &Address) -> Option<EarlyBirdRecord> {
    env.storage().persistent().get(&user_key(user))
}

/// The bonus bps that currently applies to `record` (0 when the address missed
/// the cutoff or the bonus has expired).
fn active_bonus_bps(env: &Env, record: &EarlyBirdRecord) -> u32 {
    let max = max_depositors(env);
    if max == 0 || record.sequence > max {
        return 0;
    }
    let expires_at = record.first_deposit_ledger.saturating_add(duration_ledgers(env));
    if env.ledger().sequence() >= expires_at {
        return 0;
    }
    bonus_bps(env)
}

/// Record first-deposit eligibility and return the total shares to mint for
/// this deposit: `base_shares` plus the bonus when the depositor is an active
/// early bird. Records the sequence number on the very first call and never
/// rewrites it, so `withdrawal → re-deposit` cannot regain eligibility.
pub fn record_and_bonus(env: &Env, user: &Address, base_shares: i128) -> i128 {
    let record = match record_of(env, user) {
        Some(existing) => existing,
        None => {
            // Unconfigured campaign: leave the address unrecorded so enabling
            // early-bird later still rewards genuinely new depositors.
            if max_depositors(env) == 0 {
                return base_shares;
            }
            let sequence = env
                .storage()
                .instance()
                .get::<_, u32>(&COUNT_KEY)
                .unwrap_or(0)
                .saturating_add(1);
            let record = EarlyBirdRecord {
                sequence,
                first_deposit_ledger: env.ledger().sequence(),
            };
            env.storage().persistent().set(&user_key(user), &record);
            env.storage().instance().set(&COUNT_KEY, &sequence);
            record
        }
    };

    let bps = active_bonus_bps(env, &record);
    if bps == 0 {
        return base_shares;
    }
    match base_shares.checked_mul(bps as i128) {
        Some(numerator) => base_shares.saturating_add(numerator / BPS_DENOMINATOR),
        None => base_shares,
    }
}

/// Read-only early-bird status for `user` (issue #871).
pub fn status(env: &Env, user: &Address) -> EarlyBirdStatus {
    let configured = max_depositors(env) > 0;
    match record_of(env, user) {
        Some(record) => EarlyBirdStatus {
            configured,
            eligible: configured && record.sequence <= max_depositors(env),
            sequence: record.sequence,
            expires_at: record.first_deposit_ledger.saturating_add(duration_ledgers(env)),
            active: active_bonus_bps(env, &record) > 0,
        },
        None => EarlyBirdStatus {
            configured,
            eligible: false,
            sequence: 0,
            expires_at: 0,
            active: false,
        },
    }
}

#[contractimpl]
impl VaultContract {
    /// Admin: configure the early depositor bonus (issue #871).
    ///
    /// `max_depositors` is the number of unique addresses that qualify,
    /// `bonus_bps` the multiplier applied to their minted shares, and
    /// `duration_ledgers` how long the bonus stays live from each address's
    /// first deposit.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    ///
    /// # Errors
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `NotInitialized`: the vault has no admin.
    /// - `InvalidEarlyBirdConfig`: a zero argument, or `bonus_bps` above the
    ///   10 000 bps ceiling.
    ///
    /// # Events
    /// - `eb_set` with `(max_depositors, bonus_bps, duration_ledgers, ledger)`.
    pub fn set_early_bird(
        env: Env,
        admin: Address,
        max_depositors: u32,
        bonus_bps: u32,
        duration_ledgers: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if max_depositors == 0
            || bonus_bps == 0
            || bonus_bps > MAX_BONUS_BPS
            || duration_ledgers == 0
        {
            return Err(VaultSafetyError::InvalidEarlyBirdConfig);
        }
        env.storage().instance().set(&MAX_KEY, &max_depositors);
        env.storage().instance().set(&BPS_KEY, &bonus_bps);
        env.storage().instance().set(&DUR_KEY, &duration_ledgers);
        env.events().publish(
            (symbol_short!("eb_set"), admin),
            (
                max_depositors,
                bonus_bps,
                duration_ledgers,
                env.ledger().sequence(),
            ),
        );
        Ok(())
    }

    /// Admin: disable the early depositor bonus (issue #871). Existing records
    /// are retained, so re-enabling cannot hand an already-expired address a
    /// fresh slot.
    ///
    /// # Authorization
    /// `admin` must sign and be the stored admin.
    pub fn clear_early_bird(env: Env, admin: Address) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().remove(&MAX_KEY);
        env.storage().instance().remove(&BPS_KEY);
        env.storage().instance().remove(&DUR_KEY);
        env.events().publish(
            (symbol_short!("eb_clr"), admin),
            env.ledger().sequence(),
        );
        Ok(())
    }

    /// Read-only: an address's early-bird status (issue #871).
    pub fn get_early_bird_status(env: Env, user: Address) -> EarlyBirdStatus {
        status(&env, &user)
    }
}
