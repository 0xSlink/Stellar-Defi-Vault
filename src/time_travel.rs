//! Ledger time-travel helpers for the test suite (issue #892).
//!
//! Provides standardized helpers that synchronize ledger sequence numbers,
//! block timestamps (5 seconds per block), and entry TTLs so time-dependent
//! tests remain readable and maintain consistent state.

use soroban_sdk::Env;

/// Default block time on Stellar/Soroban (5 seconds).
pub const SECONDS_PER_LEDGER: u64 = 5;
/// Approximate ledgers per calendar day.
pub const LEDGERS_PER_DAY: u32 = 17_280;

/// Set the ledger sequence to an exact sequence number, keeping timestamp and TTL in sync.
pub fn set_ledger(env: &Env, sequence: u32) {
    env.ledger().with_mut(|li| {
        let delta = sequence.saturating_sub(li.sequence_number);
        li.sequence_number = sequence;
        li.timestamp = li.timestamp.saturating_add((delta as u64) * SECONDS_PER_LEDGER);
        li.min_temp_entry_ttl = li.min_temp_entry_ttl.saturating_add(delta);
        li.min_persistent_entry_ttl = li.min_persistent_entry_ttl.saturating_add(delta);
        li.max_entry_ttl = li.max_entry_ttl.saturating_add(delta);
    });
}

/// Advance the ledger sequence by `n` ledgers, advancing timestamp and TTL accordingly.
pub fn advance_ledgers(env: &Env, n: u32) {
    env.ledger().with_mut(|li| {
        li.sequence_number = li.sequence_number.saturating_add(n);
        li.timestamp = li.timestamp.saturating_add((n as u64) * SECONDS_PER_LEDGER);
        li.min_temp_entry_ttl = li.min_temp_entry_ttl.saturating_add(n);
        li.min_persistent_entry_ttl = li.min_persistent_entry_ttl.saturating_add(n);
        li.max_entry_ttl = li.max_entry_ttl.saturating_add(n);
    });
}

/// Advance the ledger sequence by `days` calendar days (17,280 ledgers per day).
pub fn advance_days(env: &Env, days: u32) {
    advance_ledgers(env, days.saturating_mul(LEDGERS_PER_DAY));
}
