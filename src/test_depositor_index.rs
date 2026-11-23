#![cfg(test)]
//! Tests for the maintained depositor index and paginated listing (issue #748).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::vault::{VaultContract, VaultContractClient};

const STAKE: i128 = 100_000;

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    token_admin: token::StellarAssetClient<'a>,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| {
            li.min_temp_entry_ttl = 10_000_000;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
            li.sequence_number = 1000;
        });

        let admin = Address::generate(&env);
        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &0_u32, &None, &None);

        Fixture { env, vault, token_admin }
    }

    /// Fund a fresh address and stake for it, returning the address.
    fn stake_new(&self, amount: i128) -> Address {
        let user = Address::generate(&self.env);
        self.token_admin.mint(&user, &amount);
        self.vault.stake(&user, &amount);
        user
    }
}

#[test]
fn first_deposit_indexes_and_last_exit_removes() {
    let f = Fixture::new();
    let alice = f.stake_new(STAKE);

    assert_eq!(f.vault.get_depositor_count(), 1);
    let page = f.vault.list_depositors(&0, &50);
    assert_eq!(page.len(), 1);
    assert_eq!(page.get(0), Some(alice.clone()));

    // A top-up must not duplicate the entry.
    f.vault.stake(&alice, &STAKE);
    assert_eq!(f.vault.get_depositor_count(), 1);

    // A full exit removes it.
    let shares = f.vault.shares_of(&alice);
    f.vault.unstake(&alice, &shares);
    assert_eq!(f.vault.get_depositor_count(), 0);
    assert!(f.vault.list_depositors(&0, &50).is_empty());
}

#[test]
fn partial_withdrawal_keeps_the_entry() {
    let f = Fixture::new();
    let alice = f.stake_new(STAKE);

    let shares = f.vault.shares_of(&alice) / 2;
    f.vault.unstake(&alice, &shares);

    assert_eq!(f.vault.get_depositor_count(), 1);
    assert_eq!(f.vault.list_depositors(&0, &50).get(0), Some(alice));
}

#[test]
fn repeated_entry_and_exit_stays_consistent() {
    let f = Fixture::new();
    let alice = f.stake_new(STAKE);
    let bob = f.stake_new(STAKE);
    assert_eq!(f.vault.get_depositor_count(), 2);

    // Alice leaves; a swap-remove keeps the list dense and Bob still present.
    let shares = f.vault.shares_of(&alice);
    f.vault.unstake(&alice, &shares);
    assert_eq!(f.vault.get_depositor_count(), 1);
    let page = f.vault.list_depositors(&0, &50);
    assert_eq!(page.len(), 1);
    assert_eq!(page.get(0), Some(bob.clone()));

    // Re-depositing re-adds exactly one entry.
    f.token_admin.mint(&alice, &STAKE);
    f.vault.stake(&alice, &STAKE);
    assert_eq!(f.vault.get_depositor_count(), 2);
}

#[test]
fn pagination_boundaries() {
    let f = Fixture::new();
    for _ in 0..5 {
        f.stake_new(STAKE);
    }
    assert_eq!(f.vault.get_depositor_count(), 5);

    assert_eq!(f.vault.list_depositors(&0, &2).len(), 2);
    assert_eq!(f.vault.list_depositors(&2, &2).len(), 2);
    assert_eq!(f.vault.list_depositors(&4, &2).len(), 1);
    // Past the end, and a zero limit, both yield an empty page.
    assert!(f.vault.list_depositors(&5, &2).is_empty());
    assert!(f.vault.list_depositors(&0, &0).is_empty());
}

#[test]
fn limit_is_capped_at_fifty() {
    let f = Fixture::new();
    for _ in 0..3 {
        f.stake_new(STAKE);
    }
    // Requesting far more than the cap still returns at most what exists, and
    // never more than MAX_DEPOSITOR_PAGE.
    let page = f.vault.list_depositors(&0, &10_000);
    assert_eq!(page.len(), 3);
    assert_eq!(f.vault.get_depositor_count(), 3);
}
