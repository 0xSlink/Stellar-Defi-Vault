//! Maintained depositor index with paginated listing (issue #748).
//!
//! Several features need to enumerate the pool's current depositors without an
//! unbounded scan. The index tracks every address whose share balance is
//! non-zero: it is updated from `balance::set_shares`, the single choke point
//! all stake, unstake and share-moving paths write through, so an address
//! enters the index on its first deposit and leaves it on its last exit.
//!
//! # O(1) removal
//!
//! The index is a dense `Vec<Address>` plus a per-user slot number. Adding an
//! address appends it (a fresh slot). Removing one moves the last entry into
//! the vacated slot and shrinks the vector (swap-remove), so exits stay O(1)
//! no matter how many depositors remain. The count is simply the list length,
//! so `get_depositor_count()` and `list_depositors()` can never disagree.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, matching `balance.rs`: the dense list in instance storage and each
//! user's slot in persistent storage.

use soroban_sdk::{contractimpl, symbol_short, Address, Env, Symbol, Vec};

use crate::vault::VaultContract;

/// Instance key for the dense list of indexed depositors.
const LIST_KEY: Symbol = symbol_short!("dep_list");
/// Persistent key prefix mapping a depositor to its slot: `(SLOT_KEY, user)`.
const SLOT_KEY: Symbol = symbol_short!("dep_slot");

/// Largest page `list_depositors()` will return; a larger `limit` is clamped.
pub const MAX_DEPOSITOR_PAGE: u32 = 50;

fn list(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&LIST_KEY)
        .unwrap_or(Vec::new(env))
}

fn store_list(env: &Env, list: &Vec<Address>) {
    env.storage().instance().set(&LIST_KEY, list);
}

fn slot_of(env: &Env, user: &Address) -> Option<u32> {
    env.storage()
        .persistent()
        .get(&(SLOT_KEY, user.clone()))
}

fn set_slot(env: &Env, user: &Address, slot: u32) {
    env.storage()
        .persistent()
        .set(&(SLOT_KEY, user.clone()), &slot);
}

fn clear_slot(env: &Env, user: &Address) {
    env.storage()
        .persistent()
        .remove(&(SLOT_KEY, user.clone()));
}

/// The number of currently indexed depositors (share balance greater than
/// zero). The source of truth is the dense list, so this always matches
/// [`VaultContract::list_depositors`].
pub fn count(env: &Env) -> u32 {
    list(env).len()
}

/// Whether `user` is currently indexed.
pub fn contains(env: &Env, user: &Address) -> bool {
    slot_of(env, user).is_some()
}

/// Index `user`, appending a fresh slot. Idempotent, so callers can invoke it
/// on every deposit.
fn add(env: &Env, user: &Address) {
    if slot_of(env, user).is_some() {
        return;
    }
    let mut list = list(env);
    let slot = list.len();
    list.push_back(user.clone());
    store_list(env, &list);
    set_slot(env, user, slot);
}

/// Remove `user` with an O(1) swap-remove. No-op if `user` is not indexed.
fn remove(env: &Env, user: &Address) {
    let Some(slot) = slot_of(env, user) else {
        return;
    };
    let mut list = list(env);
    let len = list.len();
    if len == 0 {
        // Defensive: a slot without a list entry means the index is corrupt.
        // Clear the dangling slot rather than underflow.
        clear_slot(env, user);
        return;
    }
    let last = len - 1;
    if slot != last {
        let moved = match list.get(last) {
            Some(address) => address,
            None => {
                clear_slot(env, user);
                return;
            }
        };
        list.set(slot, moved.clone());
        set_slot(env, &moved, slot);
    }
    list.pop_back();
    store_list(env, &list);
    clear_slot(env, user);
}

/// Reconcile the index with a share-balance change from `old` to `new`.
/// Called from `balance::set_shares`; only the zero/non-zero boundary matters,
/// so top-ups and partial withdrawals leave the index untouched.
pub fn track(env: &Env, user: &Address, old: i128, new: i128) {
    if old == 0 && new > 0 {
        add(env, user);
    } else if old > 0 && new == 0 {
        remove(env, user);
    }
}

/// Read-only: up to `limit` depositors, starting `offset` entries into the
/// index. `limit` is clamped to [`MAX_DEPOSITOR_PAGE`]. Returns an empty
/// vector once `offset` is past the end.
#[cfg_attr(not(feature = "testutils"), contractimpl)]
impl VaultContract {
    /// Read-only: a page of the current depositor index.
    ///
    /// # Arguments
    /// - `offset`: number of entries to skip (0-based).
    /// - `limit`: maximum entries to return, clamped to
    ///   [`MAX_DEPOSITOR_PAGE`].
    pub fn list_depositors(env: Env, offset: u32, limit: u32) -> Vec<Address> {
        let all = list(&env);
        let len = all.len();
        if limit == 0 || offset >= len {
            return Vec::new(&env);
        }
        let end = offset
            .saturating_add(limit.min(MAX_DEPOSITOR_PAGE))
            .min(len);
        let mut page = Vec::new(&env);
        let mut i = offset;
        while i < end {
            if let Some(address) = all.get(i) {
                page.push_back(address);
            }
            i += 1;
        }
        page
    }

    /// Read-only: the number of currently indexed depositors.
    pub fn get_depositor_count(env: Env) -> u32 {
        count(&env)
    }
}
