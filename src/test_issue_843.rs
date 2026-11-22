#![cfg(test)]
//! Tests for `get_proposal_state` / `ProposalState` lifecycle enum (issue #843).
//!
//! Every reachable state is tested, and boundary ledgers are verified where
//! states transition.

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{
    proposal_state::ProposalState,
    storage::ProposableParam,
    vault::{VaultContract, VaultContractClient},
};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    token_admin: token::StellarAssetClient<'a>,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| {
            li.sequence_number = 100;
            li.min_temp_entry_ttl = 10_000_000;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
        });
        let admin = Address::generate(&env);
        let token_address = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_address);
        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_address, &0, &None, &None);
        Fixture { env, vault, admin, token_admin }
    }

    fn seq(&self) -> u32 {
        self.env.ledger().sequence()
    }

    fn set_seq(&self, s: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = s);
    }

    /// Deposit `amount` for `user` and return shares minted.
    fn fund(&self, user: &Address, amount: i128) -> i128 {
        self.token_admin.mint(user, &amount);
        self.vault.deposit(user, &amount, &None)
    }

    /// Create a proposal with `duration_ledgers` voting window, return id.
    fn make_proposal(&self, proposer: &Address, duration_ledgers: u32) -> u32 {
        self.vault.create_proposal(
            proposer,
            &ProposableParam::RewardRate,
            &500,
            &duration_ledgers,
        )
        .unwrap()
    }
}

// ── Unknown id ────────────────────────────────────────────────────────────────

#[test]
fn unknown_proposal_returns_none() {
    let f = Fixture::new();
    assert_eq!(f.vault.get_proposal_state(&9999), None);
}

// ── Active (voting open) ──────────────────────────────────────────────────────

#[test]
fn active_while_voting_window_is_open() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    let start = f.seq(); // 100
    let id = f.make_proposal(&alice, 50); // ends_at = 150

    // Mid-window: Active
    f.set_seq(start + 25);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Active));

    // One ledger before end: still Active
    f.set_seq(149);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Active));

    // At the boundary ledger ends_at: Active (vote() uses `>`, so ends_at itself still allows voting)
    f.set_seq(150);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Active));
}

// ── Expired (voting ended, failed) ───────────────────────────────────────────

#[test]
fn expired_when_no_votes_cast_and_voting_ended() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    // No votes cast → votes_for == votes_against == 0 → fails simple-majority check
    f.set_seq(151);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Expired));
}

#[test]
fn expired_when_votes_against_exceed_for() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    f.fund(&alice, 500_000);
    f.fund(&bob, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);
    f.vault.vote(&bob, &id, &false); // bob has more weight

    f.set_seq(151);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Expired));
}

// ── Queued (passed, in execution delay) ──────────────────────────────────────

#[test]
fn queued_during_execution_delay() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    // Set a 20-ledger execution delay.
    f.vault.set_execution_delay(&f.admin, &20);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);

    // One ledger after ends_at: Queued (delay = 20, executable_at = 170)
    f.set_seq(151);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Queued));

    // Still Queued one ledger before executable_at
    f.set_seq(169);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Queued));
}

// ── Succeeded (passed, delay elapsed, not yet enacted) ───────────────────────

#[test]
fn succeeded_when_delay_elapsed_and_not_enacted() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &20);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);

    // At executable_at (150 + 20 = 170): Succeeded
    f.set_seq(170);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Succeeded));

    // Well past delay, still not enacted: Succeeded
    f.set_seq(300);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Succeeded));
}

#[test]
fn succeeded_with_no_execution_delay() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    // Default delay is 0: ends_at + 0 == ends_at, so Succeeded immediately after voting ends.
    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);

    f.set_seq(151);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Succeeded));
}

// ── Executed (enacted, parameter applied) ────────────────────────────────────

#[test]
fn executed_after_enact_proposal_passes() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);

    f.set_seq(151);
    f.vault.enact_proposal(&id).unwrap();

    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Executed));
}

// ── Defeated (enacted, quorum/threshold not met) ─────────────────────────────

#[test]
fn defeated_after_enact_proposal_fails_majority() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    f.fund(&alice, 500_000);
    f.fund(&bob, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);
    f.vault.vote(&bob, &id, &false); // bob outweighs alice

    f.set_seq(151);
    f.vault.enact_proposal(&id).unwrap();

    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Defeated));
}

// ── Cancelled ────────────────────────────────────────────────────────────────

#[test]
fn cancelled_after_cancel_proposal() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    // Cancel while still open and no votes cast.
    f.vault.cancel_proposal(&alice, &id);

    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Cancelled));

    // Cancelled state persists past the voting window.
    f.set_seq(200);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Cancelled));
}

// ── Vetoed ────────────────────────────────────────────────────────────────────

#[test]
fn vetoed_after_guardian_veto() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &20);

    let id = f.make_proposal(&alice, 50); // ends_at = 150
    f.vault.vote(&alice, &id, &true);

    // Veto inside the execution delay window (ledger 151..170).
    f.set_seq(155);
    f.vault.guardian_veto_proposal(&f.admin, &id);

    // State is Vetoed regardless of where in time we query.
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Vetoed));

    // Still Vetoed after what would have been the executable ledger.
    f.set_seq(200);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Vetoed));
}

// ── Boundary: Active → Expired at ends_at + 1 (no delay, fails) ──────────────

#[test]
fn boundary_active_to_expired_one_ledger_after_ends_at() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    let id = f.make_proposal(&alice, 50); // ends_at = 150; no votes
    f.set_seq(150);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Active));
    f.set_seq(151);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Expired));
}

// ── Boundary: Queued → Succeeded at executable_at ────────────────────────────

#[test]
fn boundary_queued_to_succeeded_at_executable_ledger() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &10);

    let id = f.make_proposal(&alice, 50); // ends_at = 150; executable_at = 160
    f.vault.vote(&alice, &id, &true);

    f.set_seq(159);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Queued));
    f.set_seq(160);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Succeeded));
}
