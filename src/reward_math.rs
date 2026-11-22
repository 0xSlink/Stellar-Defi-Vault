//! Pure reward-accrual math (issue #661).
//!
//! This is the single source of truth for the reward formula that
//! `VaultContract::reward_for_ledgers` uses during accrual. It is free of any
//! `Env`/storage access so it can be exercised directly by unit tests,
//! property tests (`proptest`) and `cargo-fuzz` targets without a contract
//! instance.
//!
//! Multipliers stack multiplicatively (issue #276): the tier multiplier, then
//! the campaign multiplier (#48), then the season multiplier:
//!
//! ```text
//! effective_rate = rate_bps * tier_mult / 10_000
//!                  * campaign_mult / 10_000
//!                  * season_mult  / 10_000
//! reward         = amount * effective_rate * elapsed_ledgers
//!                  / 10_000 / STELLAR_LEDGERS_PER_YEAR
//! ```
//!
//! Every intermediate uses checked arithmetic and returns
//! [`VaultError::ArithmeticError`] instead of panicking, so a fuzz target can
//! assert "no panic; error only via the documented variant".

use crate::errors::VaultError;
use crate::vault::{BOOST_BPS_BASE, STELLAR_LEDGERS_PER_YEAR};

/// Reward owed on `amount` over `elapsed_ledgers`, with all three multipliers
/// stacked and the result annualised to a per-ledger rate. Returns `Ok(0)` for
/// a zero amount or zero elapsed ledgers.
pub fn reward_for_ledgers(
    amount: i128,
    rate_bps: u32,
    multiplier_bps: u32,
    campaign_multiplier_bps: u32,
    season_multiplier_bps: u32,
    elapsed_ledgers: u32,
) -> Result<i128, VaultError> {
    if elapsed_ledgers == 0 || amount == 0 {
        return Ok(0);
    }

    // Issue #784: each stacked multiplier below is a `mul_div_floor` now
    // rather than `checked_mul` + `checked_div`, so a large `amount` or
    // `rate_bps` can no longer make an *intermediate* step overflow i128
    // when the step's own final result would have fit. Rounding direction
    // (floor, i.e. truncating) is unchanged from the previous
    // `checked_div`-based implementation.
    let to_arith_err = |_: crate::math::MathError| VaultError::ArithmeticError;

    // Apply tier multiplier: effective_rate = rate_bps * tier_mult / 10000
    let effective_rate_bps = crate::math::mul_div_floor(
        rate_bps as i128,
        multiplier_bps as i128,
        BOOST_BPS_BASE as i128,
    )
    .map_err(to_arith_err)?;

    // Stack campaign multiplier: boosted_rate = effective_rate * campaign_mult / 10000
    let boosted_rate_bps = crate::math::mul_div_floor(
        effective_rate_bps,
        campaign_multiplier_bps as i128,
        BOOST_BPS_BASE as i128,
    )
    .map_err(to_arith_err)?;

    // Issue #276: stack season multiplier on top of tier + campaign.
    let seasoned_rate_bps = crate::math::mul_div_floor(
        boosted_rate_bps,
        season_multiplier_bps as i128,
        BOOST_BPS_BASE as i128,
    )
    .map_err(to_arith_err)?;

    // The original implementation computed `amount * seasoned_rate_bps *
    // elapsed_ledgers`, then divided by `BOOST_BPS_BASE` and
    // `STELLAR_LEDGERS_PER_YEAR` *sequentially*. Dividing by two factors in
    // sequence gives the same floor result as dividing once by their
    // product (a standard identity for non-negative integer floor
    // division), so combining the denominators first and doing a single
    // `mul_div_floor` reproduces that exact original result — rather than
    // flooring once after the first division and again after the second,
    // which would compound rounding error the original never had.
    //
    // `seasoned_rate_bps * elapsed_ledgers` uses a plain `checked_mul`
    // rather than the widening helper: both operands are bps-scaled rates
    // and ledger counts (bounded far below i128::MAX even at extreme
    // values), unlike `amount`, which is the one operand that can
    // realistically approach i128::MAX and is exactly what `mul_div_floor`
    // below protects.
    let numerator_factor = seasoned_rate_bps
        .checked_mul(elapsed_ledgers as i128)
        .ok_or(VaultError::ArithmeticError)?;
    let combined_denominator = (BOOST_BPS_BASE as i128)
        .checked_mul(STELLAR_LEDGERS_PER_YEAR as i128)
        .ok_or(VaultError::ArithmeticError)?;

    crate::math::mul_div_floor(amount, numerator_factor, combined_denominator)
        .map_err(to_arith_err)
}
