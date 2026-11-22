#![cfg(test)]
//! Tests for issue #928: zero-elapsed-ledger behavior across all accrual paths.

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::vault::{VaultContract, VaultContractClient};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    token: token::Client<'a>,
    admin: Address,
    alice: Address,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();
        env.ledger().with_mut(|li| {
            li.sequence_number = 1_000;
            li.min_temp_entry_ttl = 10_000_000;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
        });

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);
        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token = token::Client::new(&env, &token_addr);
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &2_000_u32, &None, &None);

        token_admin.mint(&admin, &100_000_000_000);
        token_admin.mint(&alice, &100_000_000_000);
        vault.fund_reward_pool(&admin, &50_000_000_000);

        Fixture {
            env,
            vault,
            token,
            admin,
            alice,
        }
    }

    fn set_ledger(&self, seq: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = seq);
    }
}

#[test]
fn same_ledger_deposit_then_claim_yields_zero_and_no_double_count() {
    let f = Fixture::new();

    // Deposit and immediately claim in the exact same ledger (elapsed == 0).
    f.vault.deposit(&f.alice, &10_000_000_000);
    let first_claim = f.vault.claim(&f.alice);
    assert_eq!(first_claim, 0);
    assert_eq!(f.vault.pending_reward(&f.alice), 0);

    // Repeated claim in the same ledger also yields 0.
    let second_claim = f.vault.claim(&f.alice);
    assert_eq!(second_claim, 0);
    assert_eq!(f.vault.pending_reward(&f.alice), 0);

    // Advance 100_000 ledgers and claim once, then claim again in the same ledger.
    f.set_ledger(101_000);
    let accrued_claim = f.vault.claim(&f.alice);
    assert!(accrued_claim > 0);
    let repeat_same_ledger = f.vault.claim(&f.alice);
    assert_eq!(repeat_same_ledger, 0);
}

#[test]
fn same_ledger_deposit_then_withdraw_cannot_extract_rewards() {
    let f = Fixture::new();
    let balance_before = f.token.balance(&f.alice);

    let shares = f.vault.deposit(&f.alice, &10_000_000_000);
    let returned = f.vault.withdraw(&f.alice, &shares);

    assert_eq!(returned, 10_000_000_000);
    assert_eq!(f.token.balance(&f.alice), balance_before);
    assert_eq!(f.vault.pending_reward(&f.alice), 0);
    assert_eq!(f.vault.claim(&f.alice), 0);
}

#[test]
fn same_ledger_deposit_claim_compound_withdraw_and_rate_change() {
    let f = Fixture::new();
    f.vault.set_auto_compound(&f.alice, &true);

    let balance_before = f.token.balance(&f.alice);

    // All five operations in ledger 1_000 (zero elapsed ledgers).
    let shares = f.vault.deposit(&f.alice, &5_000_000_000);
    f.vault.set_reward_rate_bps(&5_000);
    let claimed = f.vault.claim(&f.alice);
    assert_eq!(claimed, 0);

    f.vault.compound(&f.alice);
    assert_eq!(f.vault.shares_of(&f.alice), shares);
    assert_eq!(f.vault.pending_reward(&f.alice), 0);

    // Second rate change + claim + compound in the same ledger.
    f.vault.set_reward_rate_bps(&1_000);
    assert_eq!(f.vault.claim(&f.alice), 0);
    f.vault.compound(&f.alice);

    // Withdraw everything in the same ledger: no rewards extracted, no division by zero.
    let withdrawn = f.vault.withdraw(&f.alice, &shares);
    assert_eq!(withdrawn, 5_000_000_000);
    assert_eq!(f.token.balance(&f.alice), balance_before);
    assert_eq!(f.vault.shares_of(&f.alice), 0);
    assert_eq!(f.vault.pending_reward(&f.alice), 0);

    let _ = f.admin;
}
