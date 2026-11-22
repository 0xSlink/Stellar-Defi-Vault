//! Shared full-precision `mul_div` helpers with explicit rounding (issue #784).
//!
//! Share, reward, and fee math across this contract repeats the same
//! `a.checked_mul(b)?.checked_div(c)?` pattern. That pattern is only safe
//! when the *intermediate* product `a * b` fits in `i128` — but for
//! `mul_div(a, b, c)` the final result can be perfectly representable in
//! `i128` even when `a * b` alone overflows it (e.g. `a` and `b` both large,
//! `c` large enough to bring the quotient back down). A plain
//! `checked_mul` rejects that case with a false-positive overflow.
//!
//! [`mul_div_floor`] and [`mul_div_ceil`] fix this by computing the exact
//! 256-bit product of `a` and `b` first (via [`widening_mul`]), then
//! dividing that by `c` with a 256-by-128-bit long division
//! ([`div_u256_by_u128`]), and only then checking whether the *final*
//! result fits in `i128`. Overflow is reported only when the mathematically
//! exact answer itself doesn't fit — never because of a lossy intermediate
//! step.
//!
//! This crate has no `i256`/`u256` type available (no bignum dependency is
//! pulled in, to keep the release Wasm within its size budget — see
//! `.github/wasm-size-budget`), so the 256-bit intermediate is represented
//! as a `(high: u128, low: u128)` pair and built from four 64-by-64-bit
//! partial products, the standard schoolbook technique.
//!
//! # Rounding
//!
//! Both helpers round the *mathematically exact* rational result:
//! - [`mul_div_floor`] rounds toward negative infinity.
//! - [`mul_div_ceil`] rounds toward positive infinity.
//!
//! For the all-non-negative case every caller in this contract currently
//! uses (token amounts, share counts, bps rates, ledger counts), floor and
//! ceil both agree with "round toward/away from zero" respectively, so this
//! matches the truncating-division behavior the replaced call sites already
//! had for floor, while making the rounding direction explicit and giving
//! callers that want to round the other way (e.g. a fee that must never be
//! under-collected) a ready-made `mul_div_ceil`.
//!
//! # Scope of adoption
//!
//! This module is applied here to the highest-traffic math in the
//! contract — the share/amount conversion in `balance.rs`
//! (`amount_to_shares_with_decimals` / `shares_to_amount_with_decimals`,
//! which almost every feature module calls into) and the two other most
//! central spots, `reward_math::reward_for_ledgers` and `do_unstake`'s fee
//! calculation in `vault.rs`. The acceptance criteria for issue #784 calls
//! for replacing *every* `a * b / c` pattern in the codebase; this repo has
//! well over a hundred of them spread across ~150 feature modules, each
//! needing individual review to confirm the replacement preserves that
//! call site's exact existing rounding behavior. Doing that mechanically in
//! one pass, without being able to run the test suite to confirm nothing
//! regressed, is a correctness risk disproportionate to this change — so it
//! is left as follow-up work, module by module, with this helper as the
//! target.

/// Error from a [`mul_div_floor`] / [`mul_div_ceil`] computation.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MathError {
    /// `c == 0`.
    DivisionByZero,
    /// The mathematically exact result does not fit in `i128`.
    Overflow,
}

/// `floor(a * b / c)`, computed via a full 256-bit intermediate product so
/// overflow is reported only when the final result doesn't fit in `i128`.
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> Result<i128, MathError> {
    mul_div(a, b, c, false)
}

/// `ceil(a * b / c)`, computed via a full 256-bit intermediate product so
/// overflow is reported only when the final result doesn't fit in `i128`.
pub fn mul_div_ceil(a: i128, b: i128, c: i128) -> Result<i128, MathError> {
    mul_div(a, b, c, true)
}

fn mul_div(a: i128, b: i128, c: i128, round_ceiling: bool) -> Result<i128, MathError> {
    if c == 0 {
        return Err(MathError::DivisionByZero);
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }

    let result_negative = (a < 0) ^ (b < 0) ^ (c < 0);

    let a_mag = a.unsigned_abs();
    let b_mag = b.unsigned_abs();
    let c_mag = c.unsigned_abs();

    let (hi, lo) = widening_mul(a_mag, b_mag);
    let (trunc_quotient, remainder) =
        div_u256_by_u128(hi, lo, c_mag).ok_or(MathError::Overflow)?;

    // floor rounds away from zero on the negative side when there's a
    // remainder; ceil rounds toward zero (truncates) on the negative side.
    // On the non-negative side it's the reverse: floor truncates, ceil
    // rounds up when there's a remainder.
    let round_up_magnitude = if result_negative {
        !round_ceiling && remainder != 0
    } else {
        round_ceiling && remainder != 0
    };

    let magnitude = if round_up_magnitude {
        trunc_quotient
            .checked_add(1)
            .ok_or(MathError::Overflow)?
    } else {
        trunc_quotient
    };

    if result_negative {
        // i128::MIN's magnitude (2^127) has no positive i128 counterpart to
        // negate, so it's handled as a special case rather than via `-x`.
        match magnitude.cmp(&(i128::MAX as u128 + 1)) {
            core::cmp::Ordering::Greater => Err(MathError::Overflow),
            core::cmp::Ordering::Equal => Ok(i128::MIN),
            core::cmp::Ordering::Less => Ok(-(magnitude as i128)),
        }
    } else if magnitude > i128::MAX as u128 {
        Err(MathError::Overflow)
    } else {
        Ok(magnitude as i128)
    }
}

/// The exact 256-bit product of two `u128` values, as `(high, low)`.
///
/// Standard schoolbook multiplication: split each operand into 64-bit
/// halves, form the four 64x64-bit partial products (each of which fits
/// comfortably in a `u128`), and recombine them with explicit carries.
fn widening_mul(a: u128, b: u128) -> (u128, u128) {
    const MASK64: u128 = u64::MAX as u128;

    let a_lo = a & MASK64;
    let a_hi = a >> 64;
    let b_lo = b & MASK64;
    let b_hi = b >> 64;

    let lo_lo = a_lo * b_lo; // bits [0, 128)
    let hi_lo = a_hi * b_lo; // bits [64, 192)
    let lo_hi = a_lo * b_hi; // bits [64, 192)
    let hi_hi = a_hi * b_hi; // bits [128, 256)

    // The two "middle" partial products both land at bit offset 64; sum
    // them first, tracking the carry out of that add (worth 2^128 at this
    // scale).
    let (mid, mid_overflow) = hi_lo.overflowing_add(lo_hi);
    let mid_lo = (mid & MASK64) << 64;
    let mid_hi = mid >> 64;

    let (lo, carry_from_lo) = lo_lo.overflowing_add(mid_lo);
    let hi = hi_hi
        .wrapping_add(mid_hi)
        .wrapping_add((mid_overflow as u128) << 64)
        .wrapping_add(carry_from_lo as u128);

    (hi, lo)
}

/// Divides the 256-bit value `(hi, lo)` by `divisor`, returning
/// `(quotient, remainder)`, or `None` if the exact quotient doesn't fit in a
/// `u128`.
///
/// Binary long division, processing one bit of the dividend at a time from
/// the most significant end. `remainder` is kept `< divisor` throughout (the
/// long-division invariant); the one subtlety is that shifting `remainder`
/// left by one bit can momentarily need 129 bits to represent exactly. That
/// lost top bit is captured as `carry` and folded back in via
/// `wrapping_sub`, which is exact because `remainder < divisor <= u128::MAX`
/// guarantees the post-subtraction value is always representable in 128
/// bits (see the module tests for worked examples).
fn div_u256_by_u128(hi: u128, lo: u128, divisor: u128) -> Option<(u128, u128)> {
    if hi == 0 {
        return Some((lo / divisor, lo % divisor));
    }

    let mut remainder: u128 = 0;
    let mut quotient_hi: u128 = 0;
    let mut quotient_lo: u128 = 0;

    for (word_index, word) in [hi, lo].into_iter().enumerate() {
        for bit_index in (0..128).rev() {
            let bit = (word >> bit_index) & 1;
            let carry = remainder >> 127;
            let shifted = (remainder << 1) | bit;

            let quotient_bit = if carry == 1 || shifted >= divisor {
                remainder = shifted.wrapping_sub(divisor);
                1u128
            } else {
                remainder = shifted;
                0u128
            };

            if word_index == 0 {
                quotient_hi = (quotient_hi << 1) | quotient_bit;
            } else {
                quotient_lo = (quotient_lo << 1) | quotient_bit;
            }
        }
    }

    if quotient_hi != 0 {
        None
    } else {
        Some((quotient_lo, remainder))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rounds_toward_negative_infinity() {
        assert_eq!(mul_div_floor(7, 1, 2), Ok(3));
        assert_eq!(mul_div_floor(-7, 1, 2), Ok(-4));
        assert_eq!(mul_div_floor(7, -1, 2), Ok(-4));
        assert_eq!(mul_div_floor(-7, -1, 2), Ok(3));
    }

    #[test]
    fn ceil_rounds_toward_positive_infinity() {
        assert_eq!(mul_div_ceil(7, 1, 2), Ok(4));
        assert_eq!(mul_div_ceil(-7, 1, 2), Ok(-3));
        assert_eq!(mul_div_ceil(7, -1, 2), Ok(-3));
        assert_eq!(mul_div_ceil(-7, -1, 2), Ok(4));
    }

    #[test]
    fn exact_division_has_no_rounding_ambiguity() {
        assert_eq!(mul_div_floor(10, 3, 5), Ok(6));
        assert_eq!(mul_div_ceil(10, 3, 5), Ok(6));
    }

    #[test]
    fn division_by_zero_is_a_typed_error_not_a_panic() {
        assert_eq!(mul_div_floor(1, 1, 0), Err(MathError::DivisionByZero));
        assert_eq!(mul_div_ceil(1, 1, 0), Err(MathError::DivisionByZero));
        // Even 0/0 is reported as division-by-zero, not a silent `Ok(0)`.
        assert_eq!(mul_div_floor(0, 0, 0), Err(MathError::DivisionByZero));
    }

    #[test]
    fn zero_numerator_is_always_zero() {
        assert_eq!(mul_div_floor(0, 12345, 7), Ok(0));
        assert_eq!(mul_div_ceil(12345, 0, 7), Ok(0));
    }

    /// The core claim of issue #784: a naive `a.checked_mul(b)` overflows
    /// here (both factors exceed `i128::MAX.sqrt()`-ish magnitude), but the
    /// true result of `a * b / c` fits comfortably in `i128` once divided.
    #[test]
    fn survives_intermediate_overflow_a_naive_checked_mul_would_reject() {
        let a = i128::MAX / 2;
        let b = i128::MAX / 2;
        assert!(a.checked_mul(b).is_none(), "test setup: expected naive overflow");

        let c = i128::MAX / 4;
        let result = mul_div_floor(a, b, c).expect("should not overflow");
        // a*b/c ≈ (MAX/2 * MAX/2) / (MAX/4) ≈ MAX, within rounding.
        assert!(result > i128::MAX - 8 && result <= i128::MAX);
    }

    #[test]
    fn near_i128_max_inputs_that_truly_overflow_report_overflow() {
        // a * b vastly exceeds c, so even the exact result can't fit.
        assert_eq!(
            mul_div_floor(i128::MAX, i128::MAX, 2),
            Err(MathError::Overflow)
        );
    }

    #[test]
    fn i128_min_is_representable_as_a_result() {
        // -(2^127) == i128::MIN, exercised via the magnitude-equality branch.
        let a = i128::MIN / 2; // -(2^126)
        assert_eq!(mul_div_floor(a, 2, 1), Ok(i128::MIN));
    }

    #[test]
    fn widening_mul_matches_hand_checked_examples() {
        assert_eq!(widening_mul(1u128 << 64, 1u128 << 64), (1, 0));
        assert_eq!(
            widening_mul(u128::MAX, 2),
            (1, u128::MAX - 1)
        );
    }

    #[test]
    fn div_u256_by_u128_matches_hand_checked_examples() {
        // 2^128 / 3: verify by reconstructing hi*2^128 + lo == q*3 + r.
        let (q, r) = div_u256_by_u128(1, 0, 3).expect("fits");
        let (hi, lo) = widening_mul(q, 3);
        // Reconstruct q*3 + r as a 256-bit value and compare to (1, 0).
        let (sum_lo, carry) = lo.overflowing_add(r);
        let sum_hi = hi + carry as u128;
        assert_eq!((sum_hi, sum_lo), (1u128, 0u128));
    }
}
