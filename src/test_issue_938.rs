#![cfg(test)]
//! Tests for proposal execution window / deadline (issue #938).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env, InvokeError,
};

use crate::{
    proposal_state::ProposalState,
    storage::ProposableParam,
    vault::{proposal_window::ProposalWindowError, VaultContract, VaultContractClient},
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

    fn make_passed_proposal(&self, proposer: &Address, duration_ledgers: u32) -> u32 {
        let id = self
            .vault
            .create_proposal(proposer, &ProposableParam::RewardRate, &500, &duration_ledgers)
            .unwrap();
        self.vault.vote(proposer, &id, &true).unwrap();
        id
    }
}

#[test]
fn set_execution_window_requires_admin() {
    let f = Fixture::new();
    let stranger = Address::generate(&f.env);

    assert_eq!(
        f.vault.try_set_execution_window(&stranger, &50),
        Err(Ok(ProposalWindowError::Unauthorized))
    );
    f.vault.set_execution_window(&f.admin, &50).unwrap();
    assert_eq!(f.vault.get_execution_window(), 50);
}

#[test]
fn execute_at_first_valid_ledger_succeeds() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &20).unwrap();
    f.vault.set_execution_window(&f.admin, &30).unwrap();

    let id = f.make_passed_proposal(&alice, 50); // ends_at = 150, executable_at = 170, deadline = 200
    assert_eq!(
        f.vault.get_proposal_execution_window(&id),
        Some((170, Some(200)))
    );

    // One ledger before executable_at: Queued
    f.set_seq(169);
    assert_eq!(f.vault.get_proposal_state(&id), Some(ProposalState::Queued));

    // First valid ledger: executable_at = 170
    f.set_seq(170);
    assert_eq!(
        f.vault.get_proposal_state(&id),
        Some(ProposalState::Succeeded)
    );
    f.vault.enact_proposal(&id).unwrap();
    assert_eq!(
        f.vault.get_proposal_state(&id),
        Some(ProposalState::Executed)
    );
}

#[test]
fn execute_at_last_valid_ledger_succeeds() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &20).unwrap();
    f.vault.set_execution_window(&f.admin, &30).unwrap();

    let id = f.make_passed_proposal(&alice, 50); // ends_at = 150, executable_at = 170, deadline = 200

    // Last valid ledger: executable_at + window = 200
    f.set_seq(200);
    assert_eq!(
        f.vault.get_proposal_state(&id),
        Some(ProposalState::Succeeded)
    );
    f.vault.enact_proposal(&id).unwrap();
    assert_eq!(
        f.vault.get_proposal_state(&id),
        Some(ProposalState::Executed)
    );
}

#[test]
fn execute_one_ledger_past_window_reverts_and_state_is_expired() {
    let f = Fixture::new();
    let alice = Address::generate(&f.env);
    f.fund(&alice, 1_000_000);

    f.vault.set_execution_delay(&f.admin, &20).unwrap();
    f.vault.set_execution_window(&f.admin, &30).unwrap();

    let id = f.make_passed_proposal(&alice, 50); // ends_at = 150, executable_at = 170, deadline = 200

    // One ledger past deadline: 201
    f.set_seq(201);
    assert_eq!(
        f.vault.get_proposal_state(&id),
        Some(ProposalState::Expired)
    );

    let res = f.vault.try_enact_proposal(&id);
    assert_eq!(
        res,
        Err(Ok(InvokeError::from_contract_error(
            ProposalWindowError::ProposalExpired as u32
        )))
    );
}
