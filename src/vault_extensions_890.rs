//! Generic proposal execution limited to whitelisted call targets (issue #890).
//!
//! Extends governance to allow approved contracts and functions to be invoked
//! upon proposal execution, while strictly gating call targets via an admin whitelist.

use soroban_sdk::{
    contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, Symbol, Val, Vec,
};

use crate::admin;
use crate::vault::VaultContract;

const WHITELIST_PREFIX: Symbol = symbol_short!("wlist_ct");
const PROPOSAL_COUNT_KEY: Symbol = symbol_short!("gp_count");
const PROPOSAL_PREFIX: Symbol = symbol_short!("gp_prop");

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenericProposalState {
    Active = 0,
    Queued = 1,
    Executed = 2,
    Failed = 3,
    Rejected = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericProposal {
    pub id: u32,
    pub proposer: Address,
    pub target_contract: Address,
    pub function: Symbol,
    pub args_hash: BytesN<32>,
    pub state: GenericProposalState,
    pub created_at_ledger: u32,
}

#[contractimpl]
impl VaultContract {
    /// Whitelist a contract + function target pair for generic proposal execution.
    /// Admin authorization required.
    pub fn allow_call_target(
        env: Env,
        admin_caller: Address,
        target_contract: Address,
        function: Symbol,
    ) {
        admin::require_admin(&env, &admin_caller);
        let key = (WHITELIST_PREFIX, target_contract, function);
        env.storage().instance().set(&key, &true);
    }

    /// Remove a call target from the whitelist.
    pub fn disallow_call_target(
        env: Env,
        admin_caller: Address,
        target_contract: Address,
        function: Symbol,
    ) {
        admin::require_admin(&env, &admin_caller);
        let key = (WHITELIST_PREFIX, target_contract, function);
        env.storage().instance().remove(&key);
    }

    /// Check if a target contract + function pair is currently whitelisted.
    pub fn is_call_target_allowed(
        env: Env,
        target_contract: Address,
        function: Symbol,
    ) -> bool {
        let key = (WHITELIST_PREFIX, target_contract, function);
        env.storage().instance().get(&key).unwrap_or(false)
    }

    /// Create a generic proposal targeting a whitelisted call target.
    /// Panics if the target is not whitelisted.
    pub fn create_generic_proposal(
        env: Env,
        proposer: Address,
        target_contract: Address,
        function: Symbol,
        args_payload: Bytes,
    ) -> u32 {
        proposer.require_auth();

        if !Self::is_call_target_allowed(env.clone(), target_contract.clone(), function.clone()) {
            panic!("Target not whitelisted");
        }

        let args_hash = env.crypto().sha256(&args_payload);
        let mut count: u32 = env.storage().instance().get(&PROPOSAL_COUNT_KEY).unwrap_or(0);
        count = count.saturating_add(1);
        env.storage().instance().set(&PROPOSAL_COUNT_KEY, &count);

        let proposal = GenericProposal {
            id: count,
            proposer: proposer.clone(),
            target_contract,
            function,
            args_hash,
            state: GenericProposalState::Queued,
            created_at_ledger: env.ledger().sequence(),
        };

        let prop_key = (PROPOSAL_PREFIX, count);
        env.storage().persistent().set(&prop_key, &proposal);
        count
    }

    /// Execute a generic proposal with provided arguments.
    /// Arguments are hashed and verified against the proposal's committed args_hash.
    /// If target execution fails or traps, the proposal state is marked Failed
    /// without reverting the overall finalization transaction.
    pub fn execute_generic_proposal(
        env: Env,
        executor: Address,
        proposal_id: u32,
        args_payload: Bytes,
        call_args: Vec<Val>,
    ) -> bool {
        executor.require_auth();

        let prop_key = (PROPOSAL_PREFIX, proposal_id);
        let mut proposal: GenericProposal = env
            .storage()
            .persistent()
            .get(&prop_key)
            .expect("Proposal not found");

        if proposal.state != GenericProposalState::Queued {
            panic!("Proposal not executable");
        }

        // Verify target is still whitelisted
        if !Self::is_call_target_allowed(env.clone(), proposal.target_contract.clone(), proposal.function.clone()) {
            proposal.state = GenericProposalState::Rejected;
            env.storage().persistent().set(&prop_key, &proposal);
            return false;
        }

        // Verify args hash matches
        let provided_hash = env.crypto().sha256(&args_payload);
        if provided_hash != proposal.args_hash {
            panic!("Args hash mismatch");
        }

        // Attempt contract invocation
        let invoke_result = env.try_invoke_contract::<Val, Val>(
            &proposal.target_contract,
            &proposal.function,
            call_args,
        );

        match invoke_result {
            Ok(Ok(_)) => {
                proposal.state = GenericProposalState::Executed;
                env.storage().persistent().set(&prop_key, &proposal);
                true
            }
            _ => {
                // Mark failed without reverting finalization
                proposal.state = GenericProposalState::Failed;
                env.storage().persistent().set(&prop_key, &proposal);
                false
            }
        }
    }

    /// Query generic proposal by id.
    pub fn get_generic_proposal(env: Env, proposal_id: u32) -> Option<GenericProposal> {
        let prop_key = (PROPOSAL_PREFIX, proposal_id);
        env.storage().persistent().get(&prop_key)
    }
}
