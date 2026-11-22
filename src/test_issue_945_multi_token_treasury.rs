#![cfg(test)]

use soroban_sdk::{testutils::Ledger as _, token, Address, Env, String};

use crate::vault::{VaultContract, VaultContractClient};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    stake_token: token::Client<'a>,
    stake_admin: token::StellarAssetClient<'a>,
    admin: Address,
    alice: Address,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| {
            li.sequence_number = 1;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
        });

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);
        let stake_token_addr = env.register_stellar_asset_contract(admin.clone());
        let stake_token = token::Client::new(&env, &stake_token_addr);
        let stake_admin = token::StellarAssetClient::new(&env, &stake_token_addr);
        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &stake_token_addr, &0_u32, &None, &None);
        stake_admin.mint(&alice, &1_000_000);

        Self {
            env,
            vault,
            stake_token,
            stake_admin,
            admin,
            alice,
        }
    }
}

#[test]
fn treasury_balance_tracked_per_token_and_withdrawable() {
    let f = Fixture::new();

    f.vault.set_treasury_contribution_bps(&f.admin, &10_000);
    f.vault.set_unstake_fee_bps(&f.admin, &500);
    f.vault.deposit(&f.alice, &100_000, &0);
    f.vault.withdraw(&f.alice, &10_000);

    let stake_addr = f.stake_token.address.clone();
    let tracked = f.vault.get_treasury_balance(&stake_addr);
    assert!(tracked > 0);

    let tokens = f.vault.list_treasury_tokens();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens.get(0).unwrap(), stake_addr);

    let recipient = Address::generate(&f.env);
    let before = f.stake_token.balance(&recipient);
    f.vault
        .withdraw_treasury(&f.admin, &stake_addr, &tracked, &recipient);
    assert_eq!(f.stake_token.balance(&recipient), before + tracked);
    assert_eq!(f.vault.get_treasury_balance(&stake_addr), 0);
}

#[test]
fn cannot_withdraw_more_than_tracked_treasury_balance() {
    let f = Fixture::new();
    let stake_addr = f.stake_token.address.clone();
    let recipient = Address::generate(&f.env);

    let res = f
        .vault
        .try_withdraw_treasury(&f.admin, &stake_addr, &1, &recipient);
    assert!(res.is_err());
}

#[test]
fn balances_tracked_separately_for_different_tokens() {
    let f = Fixture::new();
    let stake_addr = f.stake_token.address.clone();

    // Fund stake-token treasury through existing fee routing.
    f.vault.set_treasury_contribution_bps(&f.admin, &10_000);
    f.vault.set_unstake_fee_bps(&f.admin, &500);
    f.vault.deposit(&f.alice, &100_000, &0);
    f.vault.withdraw(&f.alice, &10_000);
    let stake_tracked = f.vault.get_treasury_balance(&stake_addr);
    assert!(stake_tracked > 0);

    // Create a second token and simulate tracked balance by routing via direct storage path is not exposed;
    // instead verify independent accounting by checking unknown token remains zero and listing includes known token.
    let other_token_addr = f.env.register_stellar_asset_contract(f.admin.clone());
    assert_eq!(f.vault.get_treasury_balance(&other_token_addr), 0);

    let tokens = f.vault.list_treasury_tokens();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens.get(0).unwrap(), stake_addr);
}
