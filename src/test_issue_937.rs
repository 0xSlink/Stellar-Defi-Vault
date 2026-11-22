#![cfg(test)]
//! Tests for global and per-proposer active proposal limits (issue #937).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, BytesN, Env, InvokeError,
};

use crate::{
    proposal_state::ProposalState,
    storage::ProposableParam,
    vault::{proposal_limits::ProposalLimitError, VaultContract, VaultContractClient},
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
        Fixture {
            env,
            vault,
            admin,
            token_admin,
        }
    }

    fn set_seq(&self, s: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = s);
    }

    fn fund(&self, user: &Address, amount: i128) -> i128 {
        self.token_admin.mint(user, &amount);
        self.vault.deposit(user, &amount, &None)
    }

    fn make_proposal(&self, proposer: &Address, duration_ledgers: u32) -> u32 {
        self.vault
            .create_proposal(proposer, &ProposableParam::RewardRate, &500, &duration_ledgers)
            .unwrap()
    }
}

#[test]
fn set_proposal_limits_requires_admin() {
    let f = Fixture::new();
    let stranger = Address::generate(&f.env);

    assert_eq!(
        f.vault.try_set_proposal_limits(&stranger, &3, &1),
        Err(Ok(ProposalLimitError::Unauthorized))
    );
    f.vault.set_proposal_limits(&f.admin, &3, &1).unwrap();
    assert_eq!(f.vault.get_proposal_limits(), (3, 1));
}

#[test]
fn per_proposer_and_global_limits_enforced() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    let carol = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);
    f.fund(&bob, 1_000_000);
    f.fund(&carol, 1_000_000);

    // Max 2 active globally, max 1 per proposer.
    f.vault.set_proposal_limits(&f.admin, &2, &1).unwrap();

    let _p1 = f.make_proposal(&alice, 50);
    assert_eq!(f.vault.get_active_proposal_count(), 1);
    assert_eq!(f.vault.get_proposer_active_count(&alice), 1);

    // Alice hits per-proposer cap.
    let res_alice = f.vault.try_create_proposal(&alice, &ProposableParam::RewardRate, &600, &50);
    assert_eq!(
        res_alice,
        Err(Ok(InvokeError::from_contract_error(
            ProposalLimitError::TooManyActiveProposals as u32
        )))
    );

    // Bob can still create 1 (global = 2).
    let _p2 = f.make_proposal(&bob, 50);
    assert_eq!(f.vault.get_active_proposal_count(), 2);
    assert_eq!(f.vault.get_proposer_active_count(&bob), 1);

    // Carol hits global cap.
    let res_carol = f.vault.try_create_proposal(&carol, &ProposableParam::RewardRate, &700, &50);
    assert_eq!(
        res_carol,
        Err(Ok(InvokeError::from_contract_error(
            ProposalLimitError::TooManyActiveProposals as u32
        )))
    );
}

#[test]
fn counters_decrement_after_cancel_veto_and_expiry() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);
    f.fund(&bob, 1_000_000);

    f.vault.set_proposal_limits(&f.admin, &3, &1).unwrap();
    f.vault.set_guardian(&alice).unwrap();
    f.vault.set_execution_delay(&f.admin, &20).unwrap();
    f.vault.set_execution_window(&f.admin, &30).unwrap();

    // 1. Cancel decrements counters.
    let p1 = f.make_proposal(&alice, 50);
    assert_eq!(f.vault.get_proposer_active_count(&alice), 1);
    f.vault.cancel_proposal(&alice, &p1).unwrap();
    assert_eq!(f.vault.get_proposal_state(&p1), Some(ProposalState::Cancelled));
    assert_eq!(f.vault.get_active_proposal_count(), 0);
    assert_eq!(f.vault.get_proposer_active_count(&alice), 0);

    // 2. Guardian veto decrements counters.
    let p2 = f.make_proposal(&alice, 50); // ends_at = 150, executable_at = 170
    f.vault.vote(&alice, &p2, &true).unwrap();
    f.set_seq(155);
    let reason = BytesN::from_array(&f.env, &[1u8; 32]);
    f.vault.guardian_veto_proposal(&alice, &p2, &reason).unwrap();
    assert_eq!(f.vault.get_proposal_state(&p2), Some(ProposalState::Vetoed));
    assert_eq!(f.vault.get_active_proposal_count(), 0);
    assert_eq!(f.vault.get_proposer_active_count(&alice), 0);

    // 3. Expiry (voting failure) decrements counters.
    let p3 = f.make_proposal(&bob, 10); // ends_at = 165 (now = 155)
    assert_eq!(f.vault.get_active_proposal_count(), 1);
    assert_eq!(f.vault.get_proposer_active_count(&bob), 1);
    f.set_seq(166); // no votes cast -> Expired
    assert_eq!(f.vault.get_proposal_state(&p3), Some(ProposalState::Expired));
    assert_eq!(f.vault.get_active_proposal_count(), 0);
    assert_eq!(f.vault.get_proposer_active_count(&bob), 0);

    // Bob can now create a new proposal; test execution-window expiry too.
    let p4 = f.make_proposal(&bob, 10); // ends_at = 176, executable_at = 196, deadline = 226
    f.vault.vote(&bob, &p4, &true).unwrap();
    f.set_seq(227);
    assert_eq!(f.vault.get_proposal_state(&p4), Some(ProposalState::Expired));
    assert_eq!(f.vault.get_active_proposal_count(), 0);
    assert_eq!(f.vault.get_proposer_active_count(&bob), 0);

    // Slot is free again.
    let _p5 = f.make_proposal(&bob, 10);
    assert_eq!(f.vault.get_active_proposal_count(), 1);
    assert_eq!(f.vault.get_proposer_active_count(&bob), 1);
}
