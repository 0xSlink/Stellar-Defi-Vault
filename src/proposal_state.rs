//! Explicit proposal lifecycle enum and `get_proposal_state` query (issue #843).
//!
//! Clients have historically had to combine several fields
//! (`enacted`, `ends_at`, cancellation, veto, execution delay) to infer where
//! a proposal stands.  This module provides a single authoritative
//! `ProposalState` value derived entirely from storage.
//!
//! # State machine
//!
//! ```text
//!                     create_proposal
//!                           │
//!                           ▼
//!                        Active ──(cancel_proposal)──▶ Cancelled
//!                           │
//!             now > ends_at │
//!                           ▼
//!            ┌──────────────┴──────────────┐
//!      (passes vote?)                (fails vote?)
//!            │                             │
//!            ▼                             ▼
//!         Queued                        Expired
//!    (in execution delay)      (can never be enacted)
//!            │
//!   (delay elapsed)──(guardian veto)──▶ Vetoed
//!            │
//!            ▼
//!        Succeeded
//!            │
//!   (enact_proposal)
//!            │
//!            ▼
//!         Executed
//!   (parameter applied)
//!
//! Defeated is Succeeded where enact_proposal ran but quorum/threshold failed.
//!
//! Note: this contract has no pre-vote delay, so Pending == Active in
//! practice.  Pending is included in the enum for forward compatibility
//! should a voting-start offset be added later.
//! ```
//!
//! # Priority order
//!
//! States are tested in this order so that terminal states (Cancelled, Vetoed,
//! Executed, Defeated) shadow any conditions that could otherwise match a later
//! test:
//!
//! 1. Cancelled
//! 2. Executed (enacted && passes)
//! 3. Defeated (enacted && fails)
//! 4. Vetoed (not enacted, vetoed)
//! 5. Queued (voting ended, would pass, still in execution delay)
//! 6. Succeeded (voting ended, would pass, delay elapsed)
//! 7. Expired (voting ended, would fail)
//! 8. Active (voting still open)
//! 9. Pending (created with a future voting start — reserved)

use soroban_sdk::{contractimpl, contracttype, Env};

use crate::balance;
use crate::governance_params;
use crate::guardian_veto;
use crate::proposal_cancel;
use crate::vault::{VaultContract, VaultContractClient};

/// Authoritative lifecycle state of a governance proposal.
///
/// Variants are ordered from terminal/highest-priority to earliest.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalState {
    /// The proposer cancelled the proposal before any votes were cast.
    Cancelled,
    /// The proposal was enacted and the parameter change was applied.
    Executed,
    /// The proposal was enacted but quorum or pass-threshold was not met;
    /// no parameter change occurred.
    Defeated,
    /// The proposal passed the vote but was vetoed by the guardian before
    /// the execution delay elapsed; it can never be enacted.
    Vetoed,
    /// The proposal passed the vote and is waiting out the execution delay
    /// before it can be enacted.
    Queued,
    /// The proposal passed the vote, the execution delay has elapsed, and
    /// it has not yet been enacted.  Call `enact_proposal` to apply it.
    Succeeded,
    /// The proposal's voting period ended but it did not meet quorum or
    /// pass-threshold.  It can never be enacted.
    Expired,
    /// Voting is currently open.
    Active,
    /// Created but voting has not opened yet (reserved for future use when
    /// a pre-vote delay is introduced; currently identical to Active).
    Pending,
}

/// Derive the current state of proposal `id` from stored data and the
/// current ledger sequence.  Returns `None` when no proposal with that id
/// exists.
pub fn proposal_state(env: &Env, id: u32) -> Option<ProposalState> {
    let proposal = balance::get_proposal(env, id)?;
    let now = env.ledger().sequence();

    // 1. Cancelled — terminal, checked before anything else.
    if proposal_cancel::is_cancelled(env, id) {
        return Some(ProposalState::Cancelled);
    }

    // 2 & 3. Enacted (executed or defeated).
    if proposal.enacted {
        // Re-derive whether it passed using the snapshotted parameters so
        // that this read-only call agrees with what enact_proposal did.
        let passed = governance_params::passes(
            env,
            id,
            proposal.votes_for,
            proposal.votes_against,
        )
        .unwrap_or(proposal.votes_for > proposal.votes_against);
        return Some(if passed {
            ProposalState::Executed
        } else {
            ProposalState::Defeated
        });
    }

    // 4. Vetoed but not yet enacted (guardian stepped in).
    if guardian_veto::is_vetoed(env, id) {
        return Some(ProposalState::Vetoed);
    }

    // Voting is over — determine whether it would pass or not.
    if now > proposal.ends_at {
        let would_pass = governance_params::passes(
            env,
            id,
            proposal.votes_for,
            proposal.votes_against,
        )
        .unwrap_or(proposal.votes_for > proposal.votes_against);

        if !would_pass {
            // 7. Failed the vote — permanently unexecutable.
            return Some(ProposalState::Expired);
        }

        // Passed the vote; check the execution delay.
        let delay = guardian_veto::execution_delay_ledgers(env);
        let executable_at = proposal.ends_at.saturating_add(delay);

        if now < executable_at {
            // 5. Waiting out the execution delay.
            return Some(ProposalState::Queued);
        }
        if let Some(window) = crate::proposal_window::execution_window_opt(env) {
            if now > executable_at.saturating_add(window) {
                // 7b. Execution window elapsed without enactment.
                return Some(ProposalState::Expired);
            }
        }
        // 6. Ready to enact.
        Some(ProposalState::Succeeded)
    }

    // 8. Voting is still open.
    Some(ProposalState::Active)
}

#[contractimpl]
impl VaultContract {
    /// Return the authoritative lifecycle state of proposal `id`.
    ///
    /// Derived purely from on-chain storage and the current ledger; never
    /// reverts — returns `None` for an unknown id.
    pub fn get_proposal_state(env: Env, id: u32) -> Option<ProposalState> {
        proposal_state(&env, id)
    }
}
