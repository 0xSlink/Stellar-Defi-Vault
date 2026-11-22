#![cfg(test)]
//! Tests for issue #934: protocol take rate on claimed rewards (`reward_fee_bps`).

extern crate std;

use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger as _},
    token, Address, Env, Symbol, TryFromVal, Vec,
};

use crate::{
    balance,
    errors::VaultError,
    reward_fee::MAX_REWARD_FEE_BPS,
    vault::{VaultContract, VaultContractClient},
};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    vault_id: Address,
    token: token::Client<'a>,
    token_admin: token::StellarAssetClient<'a>,
    admin: Address,
    alice: Address,
    treasury_a: Address,
    treasury_b: Address,
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
        let treasury_a = Address::generate(&env);
        let treasury_b = Address::generate(&env);

        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token = token::Client::new(&env, &token_addr);
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &0_u32, &None, &None);

        token_admin.mint(&admin, &100_000_000);
        token_admin.mint(&alice, &100_000_000);
        vault.fund_reward_pool(&admin, &50_000_000);

        let mut recipients = Vec::new(&env);
        recipients.push_back(treasury_a.clone());
        recipients.push_back(treasury_b.clone());
        let mut shares = Vec::new(&env);
        shares.push_back(7_000_u32);
        shares.push_back(3_000_u32);
        vault.set_treasury_split(&admin, &recipients, &shares);

        Fixture {
            env,
            vault,
            vault_id,
            token,
            token_admin,
            admin,
            alice,
            treasury_a,
            treasury_b,
        }
    }

    fn seed_accrued(&self, user: &Address, amount: i128) {
        self.env.as_contract(&self.vault_id, || {
            balance::set_accrued_reward(&self.env, user, amount);
        });
    }

    fn set_ledger(&self, seq: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = seq);
    }

    fn last_receipt_fee(&self, user: &Address) -> Option<i128> {
        let events = self.env.events().all();
        let target_topic = symbol_short!("clm_rcpt");
        let mut last_fee = None;
        for (_, topics, data) in events.iter() {
            if topics.len() >= 2 {
                if let (Ok(sym), Ok(addr)) = (
                    Symbol::try_from_val(&self.env, &topics.get_unchecked(0)),
                    Address::try_from_val(&self.env, &topics.get_unchecked(1)),
                ) {
                    if sym == target_topic && &addr == user {
                        if let Ok(tuple) =
                            <(u32, i128, Address, u32, Address, i128)>::try_from_val(
                                &self.env,
                                &data,
                            )
                        {
                            last_fee = Some(tuple.5);
                        }
                    }
                }
            }
        }
        last_fee
    }
}

#[test]
fn reward_fee_ceiling_and_notice_period_enforced() {
    let f = Fixture::new();

    // Exceeding MAX_REWARD_FEE_BPS fails.
    let res = f
        .vault
        .try_set_reward_fee_bps(&f.admin, &(MAX_REWARD_FEE_BPS + 1));
    assert_eq!(res, Err(Ok(VaultError::UnstakeFeeTooHigh)));

    // Unauthorized caller fails.
    let stranger = Address::generate(&f.env);
    let res = f.vault.try_set_reward_fee_bps(&stranger, &500);
    assert_eq!(res, Err(Ok(VaultError::Unauthorized)));

    // Configure 100-ledger notice period.
    f.vault.set_reward_fee_notice_ledgers(&f.admin, &100);
    f.vault.set_reward_fee_bps(&f.admin, &1_000);

    // Increase is queued, not yet active at ledger 1_050.
    assert_eq!(f.vault.get_reward_fee_bps(), 0);
    assert!(f.vault.get_pending_reward_fee_change().is_some());

    f.set_ledger(1_050);
    assert_eq!(f.vault.get_reward_fee_bps(), 0);

    // At ledger 1_100, pending increase becomes effective.
    f.set_ledger(1_100);
    assert_eq!(f.vault.get_reward_fee_bps(), 1_000);

    // Decrease applies immediately without notice delay.
    f.vault.set_reward_fee_bps(&f.admin, &400);
    assert_eq!(f.vault.get_reward_fee_bps(), 400);
    assert!(f.vault.get_pending_reward_fee_change().is_none());
}

#[test]
fn reward_fee_applied_on_claim_and_routed_to_treasury_split() {
    let f = Fixture::new();
    f.vault.set_reward_fee_bps(&f.admin, &1_000); // 10%
    f.seed_accrued(&f.alice, 10_000);

    let alice_before = f.token.balance(&f.alice);
    let ta_before = f.token.balance(&f.treasury_a);
    let tb_before = f.token.balance(&f.treasury_b);

    f.vault.claim(&f.alice);

    // 10% of 10_000 = 1_000 fee -> 700 to treasury_a, 300 to treasury_b; 9_000 to alice.
    assert_eq!(f.token.balance(&f.alice) - alice_before, 9_000);
    assert_eq!(f.token.balance(&f.treasury_a) - ta_before, 700);
    assert_eq!(f.token.balance(&f.treasury_b) - tb_before, 300);
    assert_eq!(f.last_receipt_fee(&f.alice), Some(1_000));
}

#[test]
fn reward_fee_applied_on_claim_partial() {
    let f = Fixture::new();
    f.vault.set_reward_fee_bps(&f.admin, &500); // 5%
    f.seed_accrued(&f.alice, 10_000);

    let alice_before = f.token.balance(&f.alice);
    let ta_before = f.token.balance(&f.treasury_a);
    let tb_before = f.token.balance(&f.treasury_b);

    let paid = f.vault.claim_partial(&f.alice, &4_000);

    // 5% of 4_000 = 200 fee -> 140 to treasury_a, 60 to treasury_b; 3_800 to alice.
    assert_eq!(paid, 3_800);
    assert_eq!(f.token.balance(&f.alice) - alice_before, 3_800);
    assert_eq!(f.token.balance(&f.treasury_a) - ta_before, 140);
    assert_eq!(f.token.balance(&f.treasury_b) - tb_before, 60);
    assert_eq!(f.last_receipt_fee(&f.alice), Some(200));
}

#[test]
fn reward_fee_applied_on_compound() {
    let f = Fixture::new();
    f.vault.deposit(&f.alice, &10_000);
    f.vault.set_auto_compound(&f.alice, &true);
    f.vault.set_reward_fee_bps(&f.admin, &1_000); // 10%
    f.seed_accrued(&f.alice, 2_000);

    let ta_before = f.token.balance(&f.treasury_a);
    let tb_before = f.token.balance(&f.treasury_b);
    let shares_before = f.vault.shares_of(&f.alice);

    f.vault.compound(&f.alice);

    // 10% of 2_000 = 200 fee -> 140 to treasury_a, 60 to treasury_b; 1_800 compounded.
    assert_eq!(f.token.balance(&f.treasury_a) - ta_before, 140);
    assert_eq!(f.token.balance(&f.treasury_b) - tb_before, 60);
    assert_eq!(f.vault.shares_of(&f.alice) - shares_before, 1_800);
    assert_eq!(f.last_receipt_fee(&f.alice), Some(200));
}

#[test]
fn zero_reward_fee_leaves_payouts_and_treasury_unchanged() {
    let f = Fixture::new();
    f.vault.set_reward_fee_bps(&f.admin, &0);
    f.seed_accrued(&f.alice, 5_000);

    let alice_before = f.token.balance(&f.alice);
    let ta_before = f.token.balance(&f.treasury_a);
    let tb_before = f.token.balance(&f.treasury_b);

    f.vault.claim(&f.alice);

    assert_eq!(f.token.balance(&f.alice) - alice_before, 5_000);
    assert_eq!(f.token.balance(&f.treasury_a), ta_before);
    assert_eq!(f.token.balance(&f.treasury_b), tb_before);
    assert_eq!(f.last_receipt_fee(&f.alice), Some(0));
}

#[test]
fn reward_fee_interacts_with_vesting_period() {
    let f = Fixture::new();
    f.vault.set_vesting_period(&500);
    f.vault.set_reward_fee_bps(&f.admin, &1_000); // 10%
    f.seed_accrued(&f.alice, 10_000);

    let alice_before = f.token.balance(&f.alice);
    let ta_before = f.token.balance(&f.treasury_a);
    let tb_before = f.token.balance(&f.treasury_b);

    f.vault.claim(&f.alice);

    // Fee is routed at claim time; user payout is vested (not transferred yet).
    assert_eq!(f.token.balance(&f.alice), alice_before);
    assert_eq!(f.token.balance(&f.treasury_a) - ta_before, 700);
    assert_eq!(f.token.balance(&f.treasury_b) - tb_before, 300);
    assert_eq!(f.last_receipt_fee(&f.alice), Some(1_000));

    // After vesting period elapses, releasing vested entries pays the net 9_000.
    f.set_ledger(1_500);
    let released = f.vault.claim_vested(&f.alice);
    assert_eq!(released, 9_000);
    assert_eq!(f.token.balance(&f.alice) - alice_before, 9_000);
}
