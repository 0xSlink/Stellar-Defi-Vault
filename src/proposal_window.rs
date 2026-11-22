//! Proposal execution window / deadline (issue #938).
//!
//! Passed governance proposals must not sit executable forever. Once a
//! proposal's voting period ends (`now > ends_at`), its execution delay
//! elapses (`executable_at = ends_at + execution_delay`), and an execution
//! window is configured via `set_execution_window(admin, ledgers)`, the
//! proposal is only enactable through `executable_at + window` inclusive.
//! After that ledger, `enact_proposal` reverts with `ProposalExpired` and
//! `get_proposal_state` reports `ProposalState::Expired`.
//!
//! # Storage
//! Uses raw `Symbol`-keyed instance storage because `DataKey` and
//! `VaultError` are at Soroban's 50-variant cap.

use soroban_sdk::{contracterror, contractimpl, symbol_short, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::guardian_veto;
use crate::vault::{VaultContract, VaultContractClient};

/// Instance key storing the configured execution window in ledgers.
/// Absent (`None`) means no deadline is enforced (preserving legacy behavior).
const WINDOW_KEY: Symbol = symbol_short!("pw_win");

/// Errors for proposal execution window configuration and expiry.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ProposalWindowError {
    /// Contract has not been initialized.
    NotInitialized = 1,
    /// Caller is not the stored admin.
    Unauthorized = 2,
    /// The proposal's execution window has elapsed (`now > executable_at + window`).
    ProposalExpired = 3,
}

/// Returns the configured execution window in ledgers, if one has been set.
pub fn execution_window_opt(env: &Env) -> Option<u32> {
    env.storage().instance().get(&WINDOW_KEY)
}

/// Returns the configured execution window in ledgers (`0` when unset).
pub fn execution_window_ledgers(env: &Env) -> u32 {
    execution_window_opt(env).unwrap_or(0)
}

/// Computes `(executable_at, deadline)` for a proposal if it passed and a
/// window is configured.
pub fn execution_bounds(env: &Env, proposal_id: u32) -> Option<(u32, Option<u32>)> {
    let proposal = balance::get_proposal(env, proposal_id)?;
    let delay = guardian_veto::execution_delay_ledgers(env);
    let executable_at = proposal.ends_at.saturating_add(delay);
    let deadline = execution_window_opt(env).map(|w| executable_at.saturating_add(w));
    Some((executable_at, deadline))
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the execution window (in ledgers) after `executable_at`
    /// during which a passed proposal may still be enacted.
    pub fn set_execution_window(
        env: Env,
        admin_addr: Address,
        ledgers: u32,
    ) -> Result<(), ProposalWindowError> {
        admin_addr.require_auth();
        if admin_addr != admin::get_admin(&env).map_err(|_| ProposalWindowError::NotInitialized)? {
            return Err(ProposalWindowError::Unauthorized);
        }
        env.storage().instance().set(&WINDOW_KEY, &ledgers);
        Ok(())
    }

    /// Read-only: the configured execution window in ledgers (`0` if unset).
    pub fn get_execution_window(env: Env) -> u32 {
        execution_window_ledgers(&env)
    }

    /// Read-only: `(executable_at, deadline)` for `proposal_id`, where
    /// `deadline` is `Some(executable_at + window)` if an execution window is
    /// configured, or `None` if no window has been set.
    pub fn get_proposal_execution_window(
        env: Env,
        proposal_id: u32,
    ) -> Option<(u32, Option<u32>)> {
        execution_bounds(&env, proposal_id)
    }
}
