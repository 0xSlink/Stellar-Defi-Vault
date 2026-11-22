//! `deposit_and_lock()`: deposit and lock the resulting position atomically
//! (issue #747).
//!
//! Without this, a user who wants their deposit locked immediately must
//! call `deposit()` then `lock_position()` as two separate transactions,
//! leaving a window between them where the position is deposited but
//! unlocked (and so exposed to whatever an unlocked position isn't
//! protected from — e.g. a competing withdrawal race, or simply forgetting
//! the second call). `deposit_and_lock()` makes both calls within one
//! contract invocation instead of reimplementing their logic, so the
//! validations are exactly `deposit()`'s and `lock_position()`'s own —
//! never a second, drifting copy of them.
//!
//! # Atomicity
//!
//! This is a single Soroban contract invocation: if `lock_position()`
//! fails after `deposit()` already succeeded, returning that error from
//! `deposit_and_lock()` rolls back the whole invocation, including the
//! deposit — Soroban commits or discards a contract call's state changes as
//! one unit, there's nothing to manually undo here.
//!
//! # Boost timing
//!
//! `lock_position()` stamps `unlocks_at` from `env.ledger().sequence()` at
//! the moment it runs. Since both calls happen in the same invocation (and
//! so the same ledger), that's the same ledger the deposit lands on — the
//! lock boost applies from the deposit ledger with no extra bookkeeping
//! needed here.

use soroban_sdk::{contractimpl, Address, Env};

use crate::errors::VaultLockError;
use crate::vault::VaultContract;

#[contractimpl]
impl VaultContract {
    /// Deposit `amount` and immediately lock the caller's resulting
    /// position for `lock_duration_ledgers`, as one atomic call.
    ///
    /// Equivalent to calling `deposit(user, amount, referrer)` followed by
    /// `lock_position(user, lock_duration_ledgers)` — every validation
    /// either of those two perform (minimum stake, pool cap, whitelist,
    /// withdraw-only mode, max lock duration, an already-active lock, and so
    /// on) still applies, unchanged. If the deposit succeeds but the lock
    /// would fail, the whole call reverts and nothing is deposited.
    ///
    /// Note that `lock_position()` locks the caller's *entire* position
    /// (this contract has no per-amount partial lock), so if the caller
    /// already held an unlocked position, this locks that too, not just
    /// the new deposit — matching exactly what calling `lock_position()`
    /// on its own would do after this deposit.
    ///
    /// Returns the number of shares minted by the deposit.
    pub fn deposit_and_lock(
        env: Env,
        user: Address,
        amount: i128,
        lock_duration_ledgers: u32,
        referrer: Option<Address>,
    ) -> Result<i128, VaultLockError> {
        let shares_minted =
            VaultContract::deposit(env.clone(), user.clone(), amount, referrer)?;
        VaultContract::lock_position(env, user, lock_duration_ledgers)?;
        Ok(shares_minted)
    }
}
