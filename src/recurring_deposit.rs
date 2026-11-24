//! Recurring ("keeper-executed") deposits (issue #832).
//!
//! A staker can schedule a fixed-size deposit to repeat every
//! `interval_ledgers` ledgers up to `max_executions` times. The staker
//! pre-approves the vault as a token spender (the standard SEP-41 allowance),
//! and any keeper can then call [`VaultContract::execute_recurring_deposit`]
//! once the deposit is due. The keeper does not need the staker's signature:
//! the funds move through the pre-approved allowance.
//!
//! # Failure never blocks the keeper
//!
//! If the allowance or the staker's token balance has dropped below the
//! scheduled amount, the run is *skipped* — the contract emits `rec_skip` and
//! returns success rather than reverting. The schedule's due time is left
//! untouched so it is retried once the staker tops the allowance or balance
//! back up. An execution only advances the due time once it actually lands,
//! which is what makes the entrypoint safe to call from an unattended keeper.
//!
//! A successful run credits shares through the same
//! [`VaultContract::deposit_from`] path used for allowance-funded deposits, so
//! every downstream effect (depositor cap, activation, hooks, events) is
//! identical to a manual deposit.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! persistent storage, matching `balance.rs` and `auto_claim_on_withdraw.rs`.

use soroban_sdk::{contractimpl, contracttype, symbol_short, token, Address, Env, Symbol};

use crate::errors::VaultSafetyError;
use crate::storage::DataKey;
use crate::vault::VaultContract;

/// Persistent key prefix for a staker's recurring-deposit schedule.
const REC_KEY: Symbol = symbol_short!("rec_dep");

/// A staker's recurring-deposit schedule (issue #832).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct RecurringDeposit {
    /// Amount pulled (via allowance) on every execution.
    pub amount: i128,
    /// Ledgers between executions.
    pub interval_ledgers: u32,
    /// Total number of successful executions allowed.
    pub max_executions: u32,
    /// Successful executions so far.
    pub executions: u32,
    /// Ledger at which the next execution becomes due.
    pub next_due_ledger: u32,
    /// Whether the schedule is still active.
    pub active: bool,
}

fn rec_key(user: &Address) -> (Symbol, Address) {
    (REC_KEY, user.clone())
}

/// The staker's recurring-deposit schedule, if one has been set.
pub fn get_recurring(env: &Env, user: &Address) -> Option<RecurringDeposit> {
    env.storage().persistent().get(&rec_key(user))
}

/// Whether the staker has an active recurring-deposit schedule.
pub fn is_active(env: &Env, user: &Address) -> bool {
    get_recurring(env, user).map(|r| r.active).unwrap_or(false)
}

fn store(env: &Env, user: &Address, rec: &RecurringDeposit) {
    env.storage().persistent().set(&rec_key(user), rec);
}

#[contractimpl]
impl VaultContract {
    /// Create (or replace) the caller's recurring-deposit schedule (issue
    /// #832). The caller must separately approve the vault as a token spender
    /// for at least `amount` before a keeper can execute it.
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Errors
    /// - `ZeroAmount`: `amount` is not positive.
    /// - `InvalidRecurringInterval`: `interval_ledgers` is zero.
    /// - `InvalidRecurringExecutions`: `max_executions` is zero.
    ///
    /// # Events
    /// Emits `rec_set` with `(amount, interval_ledgers, max_executions, next_due_ledger)`.
    pub fn set_recurring_deposit(
        env: Env,
        user: Address,
        amount: i128,
        interval_ledgers: u32,
        max_executions: u32,
    ) -> Result<(), VaultSafetyError> {
        user.require_auth();
        if amount <= 0 {
            return Err(VaultSafetyError::ZeroAmount);
        }
        if interval_ledgers == 0 {
            return Err(VaultSafetyError::InvalidRecurringInterval);
        }
        if max_executions == 0 {
            return Err(VaultSafetyError::InvalidRecurringExecutions);
        }

        let now = env.ledger().sequence();
        let next_due_ledger = now.saturating_add(interval_ledgers);
        let rec = RecurringDeposit {
            amount,
            interval_ledgers,
            max_executions,
            executions: 0,
            next_due_ledger,
            active: true,
        };
        store(&env, &user, &rec);

        env.events().publish(
            (symbol_short!("rec_set"), user),
            (amount, interval_ledgers, max_executions, next_due_ledger),
        );
        Ok(())
    }

    /// Cancel the caller's recurring-deposit schedule (issue #832).
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Events
    /// Emits `rec_cncl` with the ledger.
    pub fn cancel_recurring_deposit(env: Env, user: Address) -> Result<(), VaultSafetyError> {
        user.require_auth();
        env.storage().persistent().remove(&rec_key(&user));
        env.events()
            .publish((symbol_short!("rec_cncl"), user), env.ledger().sequence());
        Ok(())
    }

    /// Execute a due recurring deposit for `user` (issue #832). Permissionless:
    /// any keeper may call it, and the funds are pulled through the staker's
    /// pre-approved allowance, so `user` does not sign.
    ///
    /// When the allowance or balance is insufficient the run is skipped with
    /// `rec_skip` (no revert, schedule unchanged); otherwise the deposit is
    /// credited and `rec_exec` is emitted.
    ///
    /// # Authorization
    /// `keeper` must sign.
    ///
    /// # Errors
    /// - `RecurringDepositNotFound`: `user` has no active schedule.
    /// - `RecurringDepositNotDue`: the next due ledger has not been reached.
    /// - `NotInitialized`: the vault has no token configured.
    ///
    /// # Events
    /// Emits `rec_exec` with `(amount, shares, executions)` or `rec_skip` with
    /// `(amount, next_due_ledger)`.
    pub fn execute_recurring_deposit(
        env: Env,
        keeper: Address,
        user: Address,
    ) -> Result<(), VaultSafetyError> {
        keeper.require_auth();

        let mut rec = match get_recurring(&env, &user) {
            Some(rec) if rec.active => rec,
            _ => return Err(VaultSafetyError::RecurringDepositNotFound),
        };

        let now = env.ledger().sequence();
        if now < rec.next_due_ledger {
            return Err(VaultSafetyError::RecurringDepositNotDue);
        }

        let token_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(VaultSafetyError::NotInitialized)?;
        let vault = env.current_contract_address();

        // Skip instead of reverting when the schedule cannot be funded. The
        // due time is left untouched so the keeper can retry later.
        let client = token::Client::new(&env, &token_addr);
        let allowance = client.allowance(&user, &vault);
        let balance = client.balance(&user);
        if allowance < rec.amount || balance < rec.amount {
            env.events().publish(
                (symbol_short!("rec_skip"), keeper),
                (rec.amount, rec.next_due_ledger),
            );
            return Ok(());
        }

        // Credit via the allowance-funded deposit path so all downstream
        // effects match a manual deposit. The vault is the spender (the staker
        // approved it) and `deposit_from` is invoked internally.
        match VaultContract::deposit_from(
            env.clone(),
            vault,
            user.clone(),
            rec.amount,
            user.clone(),
        ) {
            Ok(shares) => {
                rec.executions = rec.executions.saturating_add(1);
                rec.next_due_ledger = now.saturating_add(rec.interval_ledgers);
                if rec.executions >= rec.max_executions {
                    rec.active = false;
                }
                store(&env, &user, &rec);
                env.events().publish(
                    (symbol_short!("rec_exec"), user),
                    (rec.amount, shares, rec.executions),
                );
            }
            Err(_) => {
                env.events().publish(
                    (symbol_short!("rec_skip"), keeper),
                    (rec.amount, rec.next_due_ledger),
                );
            }
        }
        Ok(())
    }

    /// Read-only: the staker's recurring-deposit schedule, if any (issue #832).
    pub fn get_recurring_deposit(env: Env, user: Address) -> Option<RecurringDeposit> {
        get_recurring(&env, &user)
    }
}
