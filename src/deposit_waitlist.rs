//! Issue #834: deposit waitlist for demand that exceeds the deposit cap.
//!
//! With a deposit cap, demand above the cap has nowhere to go and is lost. The
//! waitlist lets a user queue the excess: [`VaultContract::join_waitlist`]
//! escrows the tokens, and [`VaultContract::process_waitlist`] fills queued
//! entries in FIFO order as capacity frees up.
//!
//! "Capacity" here is the remaining headroom under the admin-configured
//! per-user deposit cap ([`crate::max_deposit_cap`]) — the only configured cap
//! on deposits in this pool. When the cap is disabled (`0`) there is unlimited
//! capacity, so a queue drains in full on the next processing call. A user may
//! `join_waitlist` even when some capacity is free: the free portion is
//! credited immediately and only the excess is escrowed, which is exactly the
//! "demand above the cap" the feature is for.
//!
//! Escrowed tokens sit in the contract's balance but are **not** credited to
//! `total_deposited` until an entry is filled, so escrow never counts toward
//! TVL (and never dilutes existing stakers).
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the FIFO queue and the per-user
//! escrow records use raw `Symbol`-keyed storage, matching the other feature
//! modules.

use soroban_sdk::{
    contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, Symbol, Vec,
};

use crate::balance;
use crate::storage::DataKey;
use crate::transfer_safety;
use crate::vault::VaultContract;
use crate::vault::VaultContractClient;

/// Maximum number of entries the waitlist holds at once.
pub const MAX_WAITLIST_SIZE: u32 = 100;

/// Instance key for the FIFO queue of waiting users.
const QUEUE_KEY: Symbol = symbol_short!("wl_q");
/// Persistent key prefix for a user's escrow record: `(ESCROW_KEY, user)`.
const ESCROW_KEY: Symbol = symbol_short!("wl_esc");

/// Errors for the deposit waitlist.
///
/// `VaultError` is at Soroban's 50-variant cap, so these get their own enum;
/// `From<VaultError>` keeps `?` working at call sites.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WaitlistError {
    /// Caller is not permitted for this action.
    Unauthorized = 1,
    /// The pool has not been initialized.
    NotInitialized = 2,
    /// `amount` was not strictly positive.
    ZeroAmount = 3,
    /// Arithmetic overflow/underflow while recording the entry.
    ArithmeticError = 4,
    /// The waitlist is at [`MAX_WAITLIST_SIZE`].
    WaitlistFull = 5,
    /// The user has no escrowed waitlist entry.
    NotInWaitlist = 6,
    /// Deposits are paused, so the waitlist cannot be joined or processed.
    VaultPaused = 7,
}

impl From<crate::errors::VaultError> for WaitlistError {
    fn from(err: crate::errors::VaultError) -> Self {
        use crate::errors::VaultError;
        match err {
            VaultError::NotInitialized => WaitlistError::NotInitialized,
            VaultError::ZeroAmount => WaitlistError::ZeroAmount,
            VaultError::ArithmeticError => WaitlistError::ArithmeticError,
            VaultError::VaultPaused => WaitlistError::VaultPaused,
            _ => WaitlistError::Unauthorized,
        }
    }
}

/// A user's escrowed waitlist position.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct WaitlistEntry {
    /// The waiting user.
    pub user: Address,
    /// Amount currently escrowed and awaiting capacity.
    pub escrowed_amount: i128,
    /// Ledger the user first joined.
    pub joined_at: u32,
}

/// Internal escrow record kept per user.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
struct WaitlistRecord {
    escrowed: i128,
    joined_at: u32,
}

fn record_key(user: &Address) -> (Symbol, Address) {
    (ESCROW_KEY, user.clone())
}

fn get_record(env: &Env, user: &Address) -> Option<WaitlistRecord> {
    env.storage().persistent().get(&record_key(user))
}

fn set_record(env: &Env, user: &Address, record: &WaitlistRecord) {
    env.storage().persistent().set(&record_key(user), record);
}

fn remove_record(env: &Env, user: &Address) {
    env.storage().persistent().remove(&record_key(user));
}

fn get_queue(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&QUEUE_KEY)
        .unwrap_or(Vec::new(env))
}

fn set_queue(env: &Env, queue: &Vec<Address>) {
    env.storage().instance().set(&QUEUE_KEY, queue);
}

fn is_queued(env: &Env, user: &Address) -> bool {
    let queue = get_queue(env);
    let mut i = 0;
    while i < queue.len() {
        if let Some(addr) = queue.get(i) {
            if &addr == user {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn remove_from_queue(env: &Env, user: &Address) {
    let queue = get_queue(env);
    let mut rebuilt: Vec<Address> = Vec::new(env);
    let mut i = 0;
    while i < queue.len() {
        if let Some(addr) = queue.get(i) {
            if &addr != user {
                rebuilt.push_back(addr);
            }
        }
        i += 1;
    }
    set_queue(env, &rebuilt);
}

fn token_address(env: &Env) -> Result<Address, WaitlistError> {
    env.storage()
        .instance()
        .get(&DataKey::Token)
        .ok_or(WaitlistError::NotInitialized)
}

/// Remaining room under the per-user deposit cap. `i128::MAX` when the cap is
/// disabled.
fn cap_headroom(env: &Env, user: &Address) -> i128 {
    let cap = crate::max_deposit_cap::get_cap(env);
    if cap == 0 {
        return i128::MAX;
    }
    let shares = balance::get_shares(env, user);
    let existing = if shares == 0 {
        0
    } else {
        balance::shares_to_amount(
            balance::get_total_shares(env),
            balance::get_total_deposited(env),
            shares,
        )
        .unwrap_or(0)
    };
    cap.saturating_sub(existing).max(0)
}

/// Credits `amount` of already-held tokens to `user` as a deposit, minting
/// shares at the current price. Mirrors the accounting inside `stake` but
/// without the token pull (the tokens are already escrowed) or an auth check
/// (the user authorised the escrow when they joined).
fn credit_deposit(env: &Env, user: &Address, amount: i128) -> Result<i128, WaitlistError> {
    let total_shares = balance::get_total_shares(env);
    let total_deposited = balance::get_total_deposited(env);
    let shares = balance::amount_to_shares_with_decimals(
        total_shares,
        total_deposited,
        amount,
        balance::get_stake_decimals(env),
    )
    .ok_or(WaitlistError::ArithmeticError)?;

    let current = balance::get_shares(env, user);
    balance::set_shares(
        env,
        user,
        current.checked_add(shares).ok_or(WaitlistError::ArithmeticError)?,
    );
    balance::set_total_shares(
        env,
        total_shares
            .checked_add(shares)
            .ok_or(WaitlistError::ArithmeticError)?,
    );
    balance::set_total_deposited(
        env,
        total_deposited
            .checked_add(amount)
            .ok_or(WaitlistError::ArithmeticError)?,
    );
    if current == 0 {
        balance::register_staker(env, user);
    }
    Ok(shares)
}

#[contractimpl]
impl VaultContract {
    /// Join the deposit waitlist, escrowing `amount` of the stake token.
    ///
    /// Any portion that fits under the per-user deposit cap right now is
    /// credited immediately; the remainder is escrowed and queued for FIFO
    /// processing. Escrow does not count toward TVL.
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Errors
    /// - `ZeroAmount`: `amount <= 0` or the token delivered nothing.
    /// - `NotInitialized`: no deposit token is configured.
    /// - `VaultPaused`: deposits are paused.
    /// - `WaitlistFull`: the queue is at [`MAX_WAITLIST_SIZE`] and the deposit
    ///   cannot be credited immediately.
    ///
    /// # Events
    /// Emits `wl_join` with `(received, credited, escrowed, ledger)`.
    pub fn join_waitlist(env: Env, user: Address, amount: i128) -> Result<(), WaitlistError> {
        user.require_auth();
        crate::pause_mask::require_not_paused(&env, crate::pause_mask::PAUSE_DEPOSIT)?;
        if amount <= 0 {
            return Err(WaitlistError::ZeroAmount);
        }
        let token_addr = token_address(&env)?;
        let received = transfer_safety::pull_tokens(&env, &token_addr, &user, amount)?;

        let headroom = cap_headroom(&env, &user);
        let credited = received.min(headroom);
        if credited > 0 {
            credit_deposit(&env, &user, credited)?;
        }
        let remainder = received - credited;
        let now = env.ledger().sequence();

        if remainder > 0 {
            if !is_queued(&env, &user) {
                let mut queue = get_queue(&env);
                if queue.len() >= MAX_WAITLIST_SIZE {
                    return Err(WaitlistError::WaitlistFull);
                }
                queue.push_back(user.clone());
                set_queue(&env, &queue);
            }
            let escrowed = match get_record(&env, &user) {
                Some(record) => record
                    .escrowed
                    .checked_add(remainder)
                    .ok_or(WaitlistError::ArithmeticError)?,
                None => remainder,
            };
            let joined_at = match get_record(&env, &user) {
                Some(record) => record.joined_at,
                None => now,
            };
            set_record(&env, &user, &WaitlistRecord { escrowed, joined_at });
        }

        env.events().publish(
            (symbol_short!("wl_join"), user),
            (received, credited, remainder, now),
        );
        Ok(())
    }

    /// Keeper: fill up to `max_entries` queued entries, in FIFO order, as
    /// per-user capacity allows. Partially filled entries stay in the queue
    /// with their remainder escrowed; the walk stops at the first blocked
    /// (no-capacity) entry so ordering is preserved.
    ///
    /// # Authorization
    /// `keeper` must sign. Any caller may act as a keeper.
    ///
    /// # Errors
    /// - `VaultPaused`: deposits are paused.
    ///
    /// # Events
    /// Emits `wl_fill` per filled entry with `(credited, remaining, ledger)`.
    pub fn process_waitlist(
        env: Env,
        keeper: Address,
        max_entries: u32,
    ) -> Result<u32, WaitlistError> {
        keeper.require_auth();
        crate::pause_mask::require_not_paused(&env, crate::pause_mask::PAUSE_DEPOSIT)?;
        if max_entries == 0 {
            return Ok(0);
        }

        let queue = get_queue(&env);
        let mut rebuilt: Vec<Address> = Vec::new(&env);
        let mut processed: u32 = 0;
        let mut blocked = false;
        let now = env.ledger().sequence();

        let mut i = 0;
        while i < queue.len() {
            let user = match queue.get(i) {
                Some(user) => user,
                None => break,
            };
            i += 1;

            if blocked || processed >= max_entries {
                rebuilt.push_back(user);
                continue;
            }

            let record = match get_record(&env, &user) {
                Some(record) => record,
                None => continue, // stale queue entry
            };
            if record.escrowed <= 0 {
                remove_record(&env, &user);
                continue;
            }

            let headroom = cap_headroom(&env, &user);
            if headroom <= 0 {
                blocked = true;
                rebuilt.push_back(user);
                continue;
            }

            let fill = record.escrowed.min(headroom);
            credit_deposit(&env, &user, fill)?;
            let remaining = record.escrowed - fill;
            processed += 1;

            if remaining > 0 {
                set_record(
                    &env,
                    &user,
                    &WaitlistRecord {
                        escrowed: remaining,
                        joined_at: record.joined_at,
                    },
                );
                rebuilt.push_back(user.clone());
            } else {
                remove_record(&env, &user);
            }

            env.events().publish(
                (symbol_short!("wl_fill"), user),
                (fill, remaining, now),
            );
        }

        set_queue(&env, &rebuilt);
        Ok(processed)
    }

    /// Leave the waitlist, refunding the caller's full escrow.
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Errors
    /// - `NotInWaitlist`: the caller has no escrowed entry.
    ///
    /// # Events
    /// Emits `wl_leave` with `(refunded, ledger)`.
    pub fn leave_waitlist(env: Env, user: Address) -> Result<i128, WaitlistError> {
        user.require_auth();
        let record = get_record(&env, &user).ok_or(WaitlistError::NotInWaitlist)?;
        if record.escrowed <= 0 {
            return Err(WaitlistError::NotInWaitlist);
        }
        let token_addr = token_address(&env)?;
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &user,
            &record.escrowed,
        );
        remove_record(&env, &user);
        remove_from_queue(&env, &user);
        env.events().publish(
            (symbol_short!("wl_leave"), user),
            (record.escrowed, env.ledger().sequence()),
        );
        Ok(record.escrowed)
    }

    /// Read-only: `user`'s position in the FIFO queue, or `None` when not
    /// waiting.
    pub fn get_waitlist_position(env: Env, user: Address) -> Option<u32> {
        let queue = get_queue(&env);
        let mut i = 0;
        while i < queue.len() {
            if let Some(addr) = queue.get(i) {
                if &addr == &user {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    }

    /// Read-only: the number of users currently waiting.
    pub fn get_waitlist_len(env: Env) -> u32 {
        get_queue(&env).len()
    }

    /// Read-only: the full FIFO waitlist, oldest first.
    pub fn get_waitlist(env: Env) -> Vec<WaitlistEntry> {
        let queue = get_queue(&env);
        let mut entries: Vec<WaitlistEntry> = Vec::new(&env);
        let mut i = 0;
        while i < queue.len() {
            if let Some(user) = queue.get(i) {
                let (escrowed_amount, joined_at) = match get_record(&env, &user) {
                    Some(record) => (record.escrowed, record.joined_at),
                    None => (0, 0),
                };
                entries.push_back(WaitlistEntry {
                    user,
                    escrowed_amount,
                    joined_at,
                });
            }
            i += 1;
        }
        entries
    }
}
