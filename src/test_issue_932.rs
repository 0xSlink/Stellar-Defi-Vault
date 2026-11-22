#![cfg(test)]
//! Tests for node operator model, delegated staking, and commission (issue #932).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, BytesN, Env,
};

use crate::vault::{
    node_operator::{OperatorError, MAX_OPERATOR_COMMISSION_BPS},
    VaultContract, VaultContractClient,
};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    token_client: token::Client<'a>,
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
        let token_client = token::Client::new(&env, &token_address);
        let token_admin = token::StellarAssetClient::new(&env, &token_address);
        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_address, &0, &None, &None);
        Fixture {
            env,
            vault,
            admin,
            token_client,
            token_admin,
        }
    }

    fn set_seq(&self, s: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = s);
    }
}

#[test]
fn register_operator_enforces_commission_ceiling() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[7u8; 32]);

    assert_eq!(
        f.vault
            .try_register_operator(&op, &(MAX_OPERATOR_COMMISSION_BPS + 1), &meta),
        Err(Ok(OperatorError::CommissionTooHigh))
    );

    f.vault
        .register_operator(&op, &MAX_OPERATOR_COMMISSION_BPS, &meta)
        .unwrap();
    let info = f.vault.get_operator(&op).unwrap();
    assert_eq!(info.commission_bps, MAX_OPERATOR_COMMISSION_BPS);
}

#[test]
fn commission_math_deducts_from_delegator_rewards_and_pays_operator() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    let payer = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[1u8; 32]);

    // 10% commission (1,000 bps)
    f.vault.register_operator(&op, &1_000, &meta).unwrap();

    f.token_admin.mint(&alice, &300_000);
    f.token_admin.mint(&bob, &100_000);
    f.token_admin.mint(&payer, &100_000);

    // Alice delegates 300k (75%), Bob delegates 100k (25%).
    f.vault.delegate_to_operator(&alice, &op, &300_000).unwrap();
    f.vault.delegate_to_operator(&bob, &op, &100_000).unwrap();

    // Distribute 40,000 rewards -> 4,000 commission (10%), 36,000 net to delegators.
    let (comm, net) = f
        .vault
        .distribute_operator_rewards(&payer, &op, &40_000)
        .unwrap();
    assert_eq!(comm, 4_000);
    assert_eq!(net, 36_000);

    // Alice gets 75% of 36,000 = 27,000; Bob gets 25% of 36,000 = 9,000.
    let alice_claimed = f.vault.claim_delegator_rewards(&alice, &op).unwrap();
    let bob_claimed = f.vault.claim_delegator_rewards(&bob, &op).unwrap();
    let op_claimed = f.vault.claim_operator_commission(&op).unwrap();

    assert_eq!(alice_claimed, 27_000);
    assert_eq!(bob_claimed, 9_000);
    assert_eq!(op_claimed, 4_000);
    assert_eq!(f.token_client.balance(&op), 4_000);
}

#[test]
fn commission_increase_requires_notice_period_decrease_is_immediate() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[2u8; 32]);

    f.vault.set_commission_notice_period(&f.admin, &50).unwrap();
    f.vault.register_operator(&op, &1_000, &meta).unwrap();

    // Decrease applies immediately.
    f.vault.propose_commission_change(&op, &500).unwrap();
    assert_eq!(f.vault.get_operator(&op).unwrap().commission_bps, 500);
    assert_eq!(f.vault.get_pending_commission_change(&op), None);

    // Increase requires notice period (seq 100 + 50 = 150).
    f.vault.propose_commission_change(&op, &1_500).unwrap();
    assert_eq!(f.vault.get_operator(&op).unwrap().commission_bps, 500);

    f.set_seq(149);
    assert_eq!(
        f.vault.try_apply_commission_change(&op),
        Err(Ok(OperatorError::NoticePeriodNotElapsed))
    );

    f.set_seq(150);
    f.vault.apply_commission_change(&op).unwrap();
    assert_eq!(f.vault.get_operator(&op).unwrap().commission_bps, 1_500);
}

#[test]
fn undelegation_enforces_cooldown() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let alice = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[3u8; 32]);

    f.vault.set_undelegation_cooldown(&f.admin, &40).unwrap();
    f.vault.register_operator(&op, &500, &meta).unwrap();

    f.token_admin.mint(&alice, &100_000);
    f.vault.delegate_to_operator(&alice, &op, &100_000).unwrap();
    assert_eq!(f.token_client.balance(&alice), 0);

    // Undelegate 60,000 at seq 100 -> unlocks at 140.
    f.vault
        .undelegate_from_operator(&alice, &op, &60_000)
        .unwrap();
    assert_eq!(f.vault.get_delegation(&alice, &op).0, 40_000);

    f.set_seq(139);
    assert_eq!(
        f.vault.try_withdraw_undelegated(&alice),
        Err(Ok(OperatorError::CooldownNotElapsed))
    );

    f.set_seq(140);
    let withdrawn = f.vault.withdraw_undelegated(&alice).unwrap();
    assert_eq!(withdrawn, 60_000);
    assert_eq!(f.token_client.balance(&alice), 60_000);
}
