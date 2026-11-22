//! Global and per-proposer active proposal limits (issue #937).
//!
//! Prevents governance spam by capping how many proposals may be open at once
//! (globally and per proposer) via `set_proposal_limits(admin, max_active,
//! max_per_proposer)`. Counters automatically decrement when proposals reach
//! any terminal state (`Cancelled`, `Executed`, `Defeated`, `Vetoed`, or
//! `Expired`).
//!
//! # Storage
//! Uses raw `Symbol`-keyed storage because `DataKey` and `VaultError` are at
//! Soroban's 50-variant cap.

use soroban_sdk::{
    contracterror, contractimpl, panic_with_error, symbol_short, Address, Env, Symbol, Vec,
};

use crate::admin;
use crate::balance;
use crate::proposal_state::{proposal_state, ProposalState};
use crate::vault::{VaultContract, VaultContractClient};

const MAX_ACTIVE_KEY: Symbol = symbol_short!("pl_max_a");
const MAX_PER_PROPOSER_KEY: Symbol = symbol_short!("pl_max_p");
const ACTIVE_IDS_KEY: Symbol = symbol_short!("pl_ids");
const PROPOSER_KEY: Symbol = symbol_short!("pl_prop");

/// Errors for proposal limit configuration and enforcement.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ProposalLimitError {
    /// Contract has not been initialized.
    NotInitialized = 1,
    /// Caller is not the stored admin.
    Unauthorized = 2,
    /// Global or per-proposer active proposal limit has been reached.
    TooManyActiveProposals = 3,
}

/// Returns `true` if `state` is a terminal proposal lifecycle state.
pub fn is_terminal_state(state: &ProposalState) -> bool {
    matches!(
        state,
        ProposalState::Cancelled
            | ProposalState::Executed
            | ProposalState::Defeated
            | ProposalState::Vetoed
            | ProposalState::Expired
    )
}

/// Configured global active proposal limit (`0` = fall back to `MAX_OPEN_PROPOSALS`).
pub fn max_active_limit(env: &Env) -> u32 {
    env.storage().instance().get(&MAX_ACTIVE_KEY).unwrap_or(0)
}

/// Configured per-proposer active proposal limit (`0` = unlimited).
pub fn max_per_proposer_limit(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&MAX_PER_PROPOSER_KEY)
        .unwrap_or(0)
}

fn get_active_ids(env: &Env) -> Vec<u32> {
    env.storage()
        .instance()
        .get(&ACTIVE_IDS_KEY)
        .unwrap_or_else(|| Vec::new(env))
}

fn set_active_ids(env: &Env, ids: &Vec<u32>) {
    env.storage().instance().set(&ACTIVE_IDS_KEY, ids);
}

fn get_proposer(env: &Env, proposal_id: u32) -> Option<Address> {
    env.storage()
        .persistent()
        .get(&(PROPOSER_KEY, proposal_id))
}

/// Prunes any tracked proposal IDs that have reached a terminal state (such as
/// passive expiry) and keeps `balance::get_open_proposal_count` synchronized.
pub fn reconcile_active_proposals(env: &Env) -> Vec<u32> {
    let ids = get_active_ids(env);
    let mut live = Vec::new(env);
    let mut pruned: u32 = 0;

    for id in ids.iter() {
        let is_live = proposal_state(env, id)
            .map(|s| !is_terminal_state(&s))
            .unwrap_or(false);
        if is_live {
            live.push_back(id);
        } else {
            pruned = pruned.saturating_add(1);
        }
    }

    if pruned > 0 {
        set_active_ids(env, &live);
        let open_count = balance::get_open_proposal_count(env);
        balance::set_open_proposal_count(env, open_count.saturating_sub(pruned));
    }

    live
}

/// Removes `proposal_id` from active tracking when it transitions to a
/// terminal state via an explicit entrypoint (`cancel_proposal`,
/// `enact_proposal`, `veto_proposal`, `guardian_veto_proposal`).
pub fn mark_proposal_terminal(env: &Env, proposal_id: u32) {
    let ids = get_active_ids(env);
    let mut next = Vec::new(env);
    let mut removed = false;
    for id in ids.iter() {
        if id == proposal_id {
            removed = true;
        } else {
            next.push_back(id);
        }
    }
    if removed {
        set_active_ids(env, &next);
    }
}

/// Enforces global and per-proposer active proposal limits before creating a
/// new proposal. Reverts with `ProposalLimitError::TooManyActiveProposals` if
/// either configured limit is reached.
pub fn check_proposal_limits(env: &Env, proposer: &Address) {
    let live = reconcile_active_proposals(env);
    let max_active = max_active_limit(env);
    let max_per = max_per_proposer_limit(env);

    if max_active > 0 && live.len() >= max_active {
        panic_with_error!(env, ProposalLimitError::TooManyActiveProposals);
    }

    if max_per > 0 {
        let mut proposer_count: u32 = 0;
        for id in live.iter() {
            if get_proposer(env, id).as_ref() == Some(proposer) {
                proposer_count = proposer_count.saturating_add(1);
            }
        }
        if proposer_count >= max_per {
            panic_with_error!(env, ProposalLimitError::TooManyActiveProposals);
        }
    }
}

/// Records a newly created active proposal and its proposer.
pub fn record_proposal_active(env: &Env, proposal_id: u32, proposer: &Address) {
    env.storage()
        .persistent()
        .set(&(PROPOSER_KEY, proposal_id), proposer);
    let mut ids = get_active_ids(env);
    ids.push_back(proposal_id);
    set_active_ids(env, &ids);
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the maximum number of simultaneously active proposals
    /// globally (`max_active`) and per proposer (`max_per_proposer`).
    /// `0` disables the respective cap.
    pub fn set_proposal_limits(
        env: Env,
        admin_addr: Address,
        max_active: u32,
        max_per_proposer: u32,
    ) -> Result<(), ProposalLimitError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| ProposalLimitError::NotInitialized)? {
            return Err(ProposalLimitError::Unauthorized);
        }
        env.storage().instance().set(&MAX_ACTIVE_KEY, &max_active);
        env.storage()
            .instance()
            .set(&MAX_PER_PROPOSER_KEY, &max_per_proposer);
        Ok(())
    }

    /// Read-only: `(max_active, max_per_proposer)`. `0` means unconfigured/unlimited.
    pub fn get_proposal_limits(env: Env) -> (u32, u32) {
        (max_active_limit(&env), max_per_proposer_limit(&env))
    }

    /// Read-only: number of currently non-terminal proposals globally.
    pub fn get_active_proposal_count(env: Env) -> u32 {
        let ids = get_active_ids(&env);
        let mut count: u32 = 0;
        for id in ids.iter() {
            let is_live = proposal_state(&env, id)
                .map(|s| !is_terminal_state(&s))
                .unwrap_or(false);
            if is_live {
                count = count.saturating_add(1);
            }
        }
        count
    }

    /// Read-only: number of currently non-terminal proposals for `proposer`.
    pub fn get_proposer_active_count(env: Env, proposer: Address) -> u32 {
        let ids = get_active_ids(&env);
        let mut count: u32 = 0;
        for id in ids.iter() {
            if get_proposer(&env, id).as_ref() == Some(&proposer) {
                let is_live = proposal_state(&env, id)
                    .map(|s| !is_terminal_state(&s))
                    .unwrap_or(false);
                if is_live {
                    count = count.saturating_add(1);
                }
            }
        }
        count
    }
}
