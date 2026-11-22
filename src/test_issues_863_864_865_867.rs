#![cfg(test)]
//! Tests for Issues #863, #864, #865, and #867.
//!
//! - #863: Principal and reward pool balance separation
//! - #864: Stellar Asset Contract tokens with revocable auth and clawback
//! - #865: Native XLM integration through SAC
//! - #867: Nonce and replay protection module for signed actions

extern crate std;

use soroban_sdk::{
    testutils::{Address as _},
    token, Address, Env,
};

use crate::{
    vault_extensions_863_864_865_867::{TokenRiskFlags},
    vault::{VaultContract, VaultContractClient},
};

fn setup_vault<'a>(env: &Env) -> (VaultContractClient<'a>, Address, Address, token::Client<'a>) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(token_admin);
    let token_client = token::Client::new(env, &token_id);
    let vault_id = env.register(VaultContract, ());
    let client = VaultContractClient::new(env, &vault_id);
    client.initialize(&admin, &token_id);
    (client, admin, vault_id, token_client)
}

// ── Tests for Issue #863: Separate Principal and Reward Accounting ───────────

#[test]
fn test_principal_and_reward_pool_isolation() {
    let env = Env::default();
    let (client, _, _, token_client) = setup_vault(&env);
    let funder = Address::generate(&env);
    let user = Address::generate(&env);

    // Fund the reward pool with 5,000 tokens
    token_client.mint(&funder, &10_000);
    client.fund_reward_pool_explicit(&funder, &5_000);
    assert_eq!(client.get_reward_pool_balance(), 5_000);

    // User claims 2,000 tokens successfully
    client.claim_with_principal_protection(&user, &2_000);
    assert_eq!(client.get_reward_pool_balance(), 3_000);
    assert_eq!(token_client.balance(&user), 2_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_claims_revert_when_exceeding_reward_pool() {
    let env = Env::default();
    let (client, _, _, token_client) = setup_vault(&env);
    let funder = Address::generate(&env);
    let user = Address::generate(&env);

    // Fund with only 1,000 tokens
    token_client.mint(&funder, &5_000);
    client.fund_reward_pool_explicit(&funder, &1_000);

    // Attempting to claim 2,000 reverts with InsufficientRewardPool (Code 14)
    // rather than dipping into depositor principal
    client.claim_with_principal_protection(&user, &2_000);
}

// ── Tests for Issue #864: Stellar Asset Contract Risk Flags ──────────────────

#[test]
fn test_token_risk_flags_configuration() {
    let env = Env::default();
    let (client, admin, _, _) = setup_vault(&env);

    assert_eq!(client.get_token_risk_flags(), None);

    let flags = TokenRiskFlags {
        auth_required: true,
        auth_revocable: true,
        auth_clawback_enabled: true,
        risk_acknowledged: true,
    };

    client.set_token_risk_flags(&admin, &flags);
    let stored = client.get_token_risk_flags().expect("flags should be stored");
    assert_eq!(stored, flags);
}

// ── Tests for Issue #865: Native XLM Integration via SAC ─────────────────────

#[test]
fn test_native_xlm_sac_integration() {
    let env = Env::default();
    let (client, _, _, token_client) = setup_vault(&env);
    let user = Address::generate(&env);

    // Native XLM has 7 decimals
    assert_eq!(token_client.decimals(), 7);
    assert!(client.is_native_xlm_sac(&token_client.address));

    // Deposit 10 XLM (100,000,000 stroops)
    let ten_xlm = 100_000_000;
    token_client.mint(&user, &ten_xlm);
    let shares = client.deposit(&user, &ten_xlm, &None);
    assert!(shares > 0);
}

// ── Tests for Issue #867: Nonce & Replay Protection Module ───────────────────

#[test]
fn test_user_nonce_sequential_consumption() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);
    let user = Address::generate(&env);

    assert_eq!(client.get_user_nonce(&user), 0);

    // Consume nonce 0 -> advances to 1
    client.consume_nonce(&user, &0);
    assert_eq!(client.get_user_nonce(&user), 1);

    // Consume nonce 1 -> advances to 2
    client.consume_nonce(&user, &1);
    assert_eq!(client.get_user_nonce(&user), 2);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_user_nonce_replay_reverts() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);
    let user = Address::generate(&env);

    client.consume_nonce(&user, &0);
    // Replaying nonce 0 must revert with InvalidNonce (Code 2)
    client.consume_nonce(&user, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_user_nonce_skipped_reverts() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);
    let user = Address::generate(&env);

    // Out-of-order nonce 5 when expected is 0 must revert
    client.consume_nonce(&user, &5);
}
