#![cfg(test)]
//! Tests for the rage-quit window opened by announced adverse changes.

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env, String,
};

use crate::{
    errors::VaultError,
    vault::{VaultContract, VaultContractClient},
};

const STAKE: i128 = 100_000;
const START_BALANCE: i128 = 1_000_000;

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    vault_id: Address,
    token: token::Client<'a>,
    admin: Address,
    alice: Address,
    bob: Address,
}

fn set_ledger(env: &Env, sequence: u32) {
    env.ledger().with_mut(|ledger| ledger.sequence_number = sequence);
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let token_address = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_address);
        token_admin.mint(&alice, &START_BALANCE);
        token_admin.mint(&bob, &START_BALANCE);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_address, &0_u32, &None, &None);

        let token = token::Client::new(&env, &token_address);
        Fixture { env, vault, vault_id, token, admin, alice, bob }
    }

    /// Run `f` inside the vault's contract context (to set internal config).
    fn with_contract<R>(&self, f: impl FnOnce() -> R) -> R {
        self.env.as_contract(&self.vault_id, f)
    }

    /// Stake for alice and bob, set a live 1% unstake fee, then queue a fee
    /// increase with a 500-ledger notice at ledger 1000 — opening a window
    /// that closes at ledger 1500.
    fn with_announced_fee_hike(self) -> Self {
        set_ledger(&self.env, 1_000);
        self.vault.stake(&self.alice, &STAKE, &0);
        self.vault.stake(&self.bob, &STAKE, &0);
        self.vault.set_unstake_fee_bps(&self.admin, &100);
        self.vault.set_fee_notice_ledgers(&self.admin, &500);
        self.vault.set_unstake_fee_bps(&self.admin, &200);
        self
    }
}

#[test]
fn no_window_by_default() {
    let f = Fixture::new();
    assert_eq!(f.vault.get_rage_quit_window(), None);
}

#[test]
fn queued_fee_increase_opens_window_until_effective_ledger() {
    let f = Fixture::new().with_announced_fee_hike();
    assert_eq!(f.vault.get_rage_quit_window(), Some(1_500));
}

#[test]
fn fee_decrease_does_not_open_window() {
    let f = Fixture::new();
    set_ledger(&f.env, 1_000);
    f.vault.set_unstake_fee_bps(&f.admin, &300);
    f.vault.set_fee_notice_ledgers(&f.admin, &500);
    f.vault.set_unstake_fee_bps(&f.admin, &100);
    assert_eq!(f.vault.get_rage_quit_window(), None);
}

#[test]
fn config_change_announcement_opens_window() {
    let f = Fixture::new();
    set_ledger(&f.env, 2_000);
    f.vault.announce_config_change(
        &f.admin,
        &String::from_str(&f.env, "reward_rate"),
        &500,
        &300,
    );
    assert_eq!(f.vault.get_rage_quit_window(), Some(2_300));
}

#[test]
fn window_closes_at_the_effective_ledger() {
    let f = Fixture::new().with_announced_fee_hike();
    set_ledger(&f.env, 1_499);
    assert_eq!(f.vault.get_rage_quit_window(), Some(1_500));
    set_ledger(&f.env, 1_500);
    assert_eq!(f.vault.get_rage_quit_window(), None);
}

#[test]
fn exit_is_fee_free_during_window_and_pays_fee_after_it_closes() {
    let f = Fixture::new().with_announced_fee_hike();

    // Inside the window: alice exits in full with no fee.
    set_ledger(&f.env, 1_200);
    let alice_shares = f.vault.shares_of(&f.alice);
    f.vault.unstake(&f.alice, &alice_shares);
    assert_eq!(f.token.balance(&f.alice), START_BALANCE);

    // After the window: bob pays the normal fee.
    set_ledger(&f.env, 1_500);
    let bob_shares = f.vault.shares_of(&f.bob);
    f.vault.unstake(&f.bob, &bob_shares);
    assert!(f.token.balance(&f.bob) < START_BALANCE);
}

#[test]
fn cooldown_is_skipped_during_window_and_enforced_after_it() {
    let f = Fixture::new().with_announced_fee_hike();
    f.with_contract(|| crate::balance::set_cooldown_period(&f.env, 50));

    // Inside the window the cooldown does not apply.
    set_ledger(&f.env, 1_200);
    let alice_shares = f.vault.shares_of(&f.alice);
    f.vault.unstake(&f.alice, &alice_shares);
    assert_eq!(f.token.balance(&f.alice), START_BALANCE);

    // After the window the direct path is gated by the cooldown again.
    set_ledger(&f.env, 1_500);
    let bob_shares = f.vault.shares_of(&f.bob);
    assert_eq!(
        f.vault.try_unstake(&f.bob, &bob_shares),
        Err(Ok(VaultError::UseCooldownFlow))
    );
}

#[test]
fn locked_positions_still_pay_the_early_exit_penalty_in_the_window() {
    let f = Fixture::new();
    set_ledger(&f.env, 1_000);
    f.with_contract(|| {
        crate::balance::set_lock_period(&f.env, 10_000);
        crate::balance::set_early_exit_penalty_bps(&f.env, 500);
    });
    f.vault.stake(&f.alice, &STAKE, &0);
    f.vault.set_fee_notice_ledgers(&f.admin, &500);
    f.vault.set_unstake_fee_bps(&f.admin, &200);
    assert_eq!(f.vault.get_rage_quit_window(), Some(1_500));

    // The fee is waived, but the lock is not: the 5% penalty still applies.
    set_ledger(&f.env, 1_200);
    let shares = f.vault.shares_of(&f.alice);
    f.vault.unstake(&f.alice, &shares);
    assert_eq!(f.token.balance(&f.alice), START_BALANCE - STAKE * 5 / 100);
}
