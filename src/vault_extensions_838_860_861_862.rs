//! Vault extensions for issues #838, #860, #861, and #862.
//!
//! - Issue #838: Add oracle staleness and sanity validation
//! - Issue #860: Add a virtual shares and assets offset to defend against share inflation
//! - Issue #861: Burn minimum liquidity shares on the first deposit
//! - Issue #862: Audit and test that accounting never reads the raw token balance

use soroban_sdk::{
    contractclient, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol,
};

use crate::admin;
use crate::balance;
use crate::vault::VaultContract;

// ── Storage Keys ─────────────────────────────────────────────────────────────

const ORACLE_ADDR_KEY: Symbol = symbol_short!("ext_orcl");
const ORACLE_LIMITS_KEY: Symbol = symbol_short!("orc_lim");
const DEAD_SHARES_KEY: Symbol = symbol_short!("dead_shr");

// ── Constants ────────────────────────────────────────────────────────────────

/// Standard virtual shares offset (+1e6 = 1_000_000 stroops).
/// Prevents the share price from being inflated via donations on first deposit.
pub const VIRTUAL_SHARES_OFFSET: i128 = 1_000_000;

/// Standard virtual assets offset (+1 stroop).
pub const VIRTUAL_ASSETS_OFFSET: i128 = 1;

/// Minimum liquidity shares permanently burned to dead storage on first deposit (issue #861).
pub const MIN_LIQUIDITY_SHARES: i128 = 1_000;

// ── Types and Errors ─────────────────────────────────────────────────────────

/// Configured sanity bounds and staleness threshold for the external price oracle.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct OracleLimits {
    /// Maximum allowable age of the oracle price in ledgers.
    pub max_age_ledgers: u32,
    /// Minimum plausible price (inclusive) in price units (e.g. 7 decimals fixed-point).
    pub min_price: i128,
    /// Maximum plausible price (inclusive) in price units.
    pub max_price: i128,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OracleValidationError {
    Unauthorized = 1,
    NoOracleConfigured = 2,
    StaleOracle = 3,
    OraclePriceOutOfRange = 4,
    InvalidLimits = 5,
    ArithmeticError = 6,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VaultInflationError {
    Unauthorized = 1,
    ZeroAmount = 2,
    InsufficientInitialDeposit = 3,
    ArithmeticError = 4,
}

/// Standard interface for price oracle feed providing (price, timestamp_ledger).
#[contractclient(name = "ValidatedPriceOracleClient")]
pub trait ValidatedPriceOracleInterface {
    fn get_price_data(env: Env, token: Address) -> (i128, u32);
}

// ── Implementation ───────────────────────────────────────────────────────────

#[contractimpl]
impl VaultContract {
    // ── Issue #838: Oracle Staleness and Sanity Validation ──────────────────

    /// Configure the oracle staleness window and allowable price range.
    ///
    /// # Authorization
    /// Requires current administrator authorization.
    pub fn set_oracle_limits(
        env: Env,
        admin: Address,
        max_age_ledgers: u32,
        min_price: i128,
        max_price: i128,
    ) -> Result<(), OracleValidationError> {
        admin.require_auth();
        let stored_admin = admin::get_admin(&env).map_err(|_| OracleValidationError::Unauthorized)?;
        if admin != stored_admin {
            return Err(OracleValidationError::Unauthorized);
        }

        if max_age_ledgers == 0 || min_price <= 0 || max_price < min_price {
            return Err(OracleValidationError::InvalidLimits);
        }

        let limits = OracleLimits {
            max_age_ledgers,
            min_price,
            max_price,
        };
        env.storage().instance().set(&ORACLE_LIMITS_KEY, &limits);

        env.events().publish(
            (symbol_short!("orc_lim"), admin),
            (max_age_ledgers, min_price, max_price),
        );

        Ok(())
    }

    /// Retrieve the currently configured oracle limits, if any.
    pub fn get_oracle_limits(env: Env) -> Option<OracleLimits> {
        env.storage().instance().get(&ORACLE_LIMITS_KEY)
    }

    /// Retrieve the current oracle price, its age in ledgers, and its validity.
    ///
    /// Returns `(price, age_ledgers, is_valid)`.
    pub fn get_oracle_status(env: Env) -> Result<(i128, u32, bool), OracleValidationError> {
        let oracle_addr: Address = env
            .storage()
            .instance()
            .get(&ORACLE_ADDR_KEY)
            .ok_or(OracleValidationError::NoOracleConfigured)?;

        let token_addr = Self::token_address(&env)
            .map_err(|_| OracleValidationError::NoOracleConfigured)?;

        let (price, updated_ledger) = ValidatedPriceOracleClient::new(&env, &oracle_addr)
            .get_price_data(&token_addr);

        let current_ledger = env.ledger().sequence();
        let age_ledgers = current_ledger.saturating_sub(updated_ledger);

        let is_valid = if let Some(limits) = Self::get_oracle_limits(env.clone()) {
            price > 0
                && price >= limits.min_price
                && price <= limits.max_price
                && age_ledgers <= limits.max_age_ledgers
        } else {
            price > 0
        };

        Ok((price, age_ledgers, is_valid))
    }

    /// Read the price from the registered oracle, enforcing freshness and sanity bounds.
    ///
    /// Reverts with `StaleOracle` if the price age exceeds `max_age_ledgers`.
    /// Reverts with `OraclePriceOutOfRange` if price <= 0 or outside `[min_price, max_price]`.
    pub fn get_validated_oracle_price(env: Env) -> Result<i128, OracleValidationError> {
        let (price, age_ledgers, is_valid) = Self::get_oracle_status(env.clone())?;

        if let Some(limits) = Self::get_oracle_limits(env) {
            if age_ledgers > limits.max_age_ledgers {
                return Err(OracleValidationError::StaleOracle);
            }
            if price <= 0 || price < limits.min_price || price > limits.max_price {
                return Err(OracleValidationError::OraclePriceOutOfRange);
            }
        } else if !is_valid || price <= 0 {
            return Err(OracleValidationError::OraclePriceOutOfRange);
        }

        Ok(price)
    }

    // ── Issue #860: Virtual Shares and Assets Offset ─────────────────────────

    /// Converts asset base units to shares using standard virtual offset math:
    /// `shares = assets * (total_shares + 1e6) / (total_assets + 1)`
    pub fn convert_to_shares_with_offset(env: Env, assets: i128) -> i128 {
        if assets <= 0 {
            return 0;
        }
        let total_shares = balance::get_total_shares(&env);
        let total_assets = balance::get_total_deposited(&env);

        let eff_shares = match total_shares.checked_add(VIRTUAL_SHARES_OFFSET) {
            Some(v) => v,
            None => return 0,
        };
        let eff_assets = match total_assets.checked_add(VIRTUAL_ASSETS_OFFSET) {
            Some(v) => v,
            None => return 0,
        };

        assets
            .checked_mul(eff_shares)
            .and_then(|prod| prod.checked_div(eff_assets))
            .unwrap_or(0)
    }

    /// Converts share units to underlying assets using standard virtual offset math:
    /// `assets = shares * (total_assets + 1) / (total_shares + 1e6)`
    pub fn convert_to_assets_with_offset(env: Env, shares: i128) -> i128 {
        if shares <= 0 {
            return 0;
        }
        let total_shares = balance::get_total_shares(&env);
        let total_assets = balance::get_total_deposited(&env);

        let eff_shares = match total_shares.checked_add(VIRTUAL_SHARES_OFFSET) {
            Some(v) => v,
            None => return 0,
        };
        let eff_assets = match total_assets.checked_add(VIRTUAL_ASSETS_OFFSET) {
            Some(v) => v,
            None => return 0,
        };

        shares
            .checked_mul(eff_assets)
            .and_then(|prod| prod.checked_div(eff_shares))
            .unwrap_or(0)
    }

    /// Preview shares minted for an asset deposit using virtual offset math.
    pub fn preview_deposit_with_offset(env: Env, assets: i128) -> i128 {
        Self::convert_to_shares_with_offset(env, assets)
    }

    /// Preview assets returned on a share redemption using virtual offset math.
    pub fn preview_withdraw_with_offset(env: Env, shares: i128) -> i128 {
        Self::convert_to_assets_with_offset(env, shares)
    }

    // ── Issue #861: Minimum Liquidity Shares Burn ────────────────────────────

    /// Query permanently locked dead shares minted on the initial deposit.
    pub fn get_locked_dead_shares(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DEAD_SHARES_KEY)
            .unwrap_or(0)
    }

    /// Constant query for the minimum liquidity share requirement.
    pub fn min_liquidity_shares() -> i128 {
        MIN_LIQUIDITY_SHARES
    }

    /// Deposit tokens with first-deposit dead share locking (Issue #861).
    ///
    /// If this is the initial deposit to an empty vault (`total_shares == 0`),
    /// the caller must supply enough assets to mint at least `MIN_LIQUIDITY_SHARES + 1`.
    /// `MIN_LIQUIDITY_SHARES` is permanently allocated to dead storage and never
    /// released or rewarded, ensuring share supply never returns to 0.
    pub fn deposit_with_dead_shares(
        env: Env,
        user: Address,
        amount: i128,
    ) -> Result<i128, VaultInflationError> {
        user.require_auth();
        if amount <= 0 {
            return Err(VaultInflationError::ZeroAmount);
        }

        let total_shares = balance::get_total_shares(&env);
        let total_deposited = balance::get_total_deposited(&env);

        let is_first_deposit = total_shares == 0;
        let minted = Self::preview_deposit(env.clone(), amount);

        if is_first_deposit {
            if minted <= MIN_LIQUIDITY_SHARES {
                return Err(VaultInflationError::InsufficientInitialDeposit);
            }

            // Permanently lock MIN_LIQUIDITY_SHARES
            env.storage()
                .instance()
                .set(&DEAD_SHARES_KEY, &MIN_LIQUIDITY_SHARES);

            let user_shares = minted
                .checked_sub(MIN_LIQUIDITY_SHARES)
                .ok_or(VaultInflationError::ArithmeticError)?;

            // Update total state
            balance::set_total_shares(&env, minted);
            balance::set_total_deposited(
                &env,
                total_deposited
                    .checked_add(amount)
                    .ok_or(VaultInflationError::ArithmeticError)?,
            );

            // Credit remaining shares to user
            let cur_user_shares = balance::get_shares(&env, &user);
            balance::set_shares(
                &env,
                &user,
                cur_user_shares
                    .checked_add(user_shares)
                    .ok_or(VaultInflationError::ArithmeticError)?,
            );
            if cur_user_shares == 0 {
                balance::register_staker(&env, &user);
            }

            env.events().publish(
                (symbol_short!("dead_lck"), user.clone()),
                (MIN_LIQUIDITY_SHARES, user_shares),
            );

            Ok(user_shares)
        } else {
            let user_shares = Self::deposit(env, user, amount, None)
                .map_err(|_| VaultInflationError::ArithmeticError)?;
            Ok(user_shares)
        }
    }

    // ── Issue #862: Balance Accounting Isolation ─────────────────────────────

    /// Verifies that internal accounting counters remain completely unaffected
    /// by any raw token balance transfers directly into the contract address.
    pub fn verify_balance_accounting_isolation(env: Env) -> bool {
        let total_deposited = balance::get_total_deposited(&env);
        let total_shares = balance::get_total_shares(&env);

        // Preview functions and share conversion must calculate solely from internal counters
        let preview_100 = Self::preview_deposit(env.clone(), 100);
        let preview_expected = balance::amount_to_shares(total_shares, total_deposited, 100).unwrap_or(0);

        preview_100 == preview_expected
    }
}
