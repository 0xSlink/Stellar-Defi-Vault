#![cfg(test)]
//! Tests for the Merkle distributor (issue #873).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Bytes, BytesN, Env, Vec,
};

use crate::errors::VaultSafetyError;
use crate::vault::merkle_distributor::DISTRIBUTION_WINDOW_LEDGERS;
use crate::vault::{VaultContract, VaultContractClient};

const DOMAIN: &[u8] = b"stellar-defi-vault:distribution:v1:";
const TOTAL: i128 = 500_000;
const BONUS: i128 = 40_000;

/// Must mirror `merkle_distributor::distribution_leaf`.
fn leaf(env: &Env, addr: &Address, amount: i128) -> BytesN<32> {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(DOMAIN);
    let s = addr.to_string();
    let mut raw = [0u8; 64];
    let slice = &mut raw[..s.len() as usize];
    s.copy_into_slice(slice);
    buf.extend_from_slice(slice);
    buf.extend_from_slice(&amount.to_be_bytes());
    env.crypto().sha256(&buf).into()
}

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    vault_id: Address,
    token: token::Client<'a>,
    token_addr: Address,
    admin: Address,
    alice: Address,
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
        let alice = Address::generate(&env);
        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);

        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token = token::Client::new(&env, &token_addr);
        token::StellarAssetClient::new(&env, &token_addr).mint(&admin, &10_000_000);
        vault.initialize(&admin, &token_addr, &0_u32, &None, &None);

        Fixture { env, vault, vault_id, token, token_addr, admin, alice }
    }

    /// Publish a one-recipient distribution for Alice and return its id.
    fn publish_single(&self) -> u32 {
        let root = leaf(&self.env, &self.alice, BONUS);
        self.vault
            .publish_distribution(&self.admin, &root, &self.token_addr, &TOTAL)
    }

    fn advance(&self, ledgers: u32) {
        self.env.ledger().with_mut(|li| {
            li.sequence_number += ledgers;
        });
    }
}

#[test]
fn publish_escrows_the_tokens() {
    let f = Fixture::new();
    let before = f.token.balance(&f.admin);
    let id = f.publish_single();
    assert_eq!(id, 0);
    // Escrow left the admin and landed in the contract.
    assert_eq!(f.token.balance(&f.admin), before - TOTAL);
    assert_eq!(f.token.balance(&f.vault_id), TOTAL);
    let dist = f.vault.get_distribution(&id).unwrap();
    assert_eq!(dist.total, TOTAL);
    assert_eq!(dist.remaining, TOTAL);
}

#[test]
fn claim_pays_once() {
    let f = Fixture::new();
    let id = f.publish_single();
    let before = f.token.balance(&f.alice);

    f.vault
        .claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env));

    assert_eq!(f.token.balance(&f.alice), before + BONUS);
    assert_eq!(f.vault.get_distribution(&id).unwrap().remaining, TOTAL - BONUS);
    assert!(f.vault.is_distribution_claimed(&id, &f.alice));
}

#[test]
fn double_claim_reverts() {
    let f = Fixture::new();
    let id = f.publish_single();
    f.vault
        .claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env));

    assert_eq!(
        f.vault
            .try_claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env)),
        Err(Ok(VaultSafetyError::DistributionAlreadyClaimed))
    );
}

#[test]
fn wrong_amount_reverts() {
    let f = Fixture::new();
    let id = f.publish_single();

    // The amount is bound into the leaf, so a different value cannot prove.
    assert_eq!(
        f.vault
            .try_claim_distribution(&f.alice, &id, &(BONUS + 1), &Vec::new(&f.env)),
        Err(Ok(VaultSafetyError::InvalidMerkleProof))
    );
}

#[test]
fn unknown_distribution_reverts() {
    let f = Fixture::new();
    assert_eq!(
        f.vault
            .try_claim_distribution(&f.alice, &7, &BONUS, &Vec::new(&f.env)),
        Err(Ok(VaultSafetyError::DistributionNotFound))
    );
}

#[test]
fn recover_requires_expiry() {
    let f = Fixture::new();
    let id = f.publish_single();
    assert_eq!(
        f.vault.try_recover_unclaimed(&f.admin, &id),
        Err(Ok(VaultSafetyError::DistributionNotExpired))
    );
}

#[test]
fn recover_after_expiry_returns_the_remainder() {
    let f = Fixture::new();
    let id = f.publish_single();
    // Alice claims part, leaving a remainder.
    f.vault
        .claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env));

    f.advance(DISTRIBUTION_WINDOW_LEDGERS + 1);
    let admin_before = f.token.balance(&f.admin);
    f.vault.recover_unclaimed(&f.admin, &id);
    assert_eq!(f.token.balance(&f.admin), admin_before + (TOTAL - BONUS));
    assert_eq!(f.token.balance(&f.vault_id), 0);

    // Nothing is claimable afterwards.
    assert_eq!(
        f.vault
            .try_claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env)),
        Err(Ok(VaultSafetyError::DistributionAlreadyRecovered))
    );
    // And recovery cannot run twice.
    assert_eq!(
        f.vault.try_recover_unclaimed(&f.admin, &id),
        Err(Ok(VaultSafetyError::DistributionAlreadyRecovered))
    );
}

#[test]
fn claim_after_expiry_reverts() {
    let f = Fixture::new();
    let id = f.publish_single();
    f.advance(DISTRIBUTION_WINDOW_LEDGERS + 1);
    assert_eq!(
        f.vault
            .try_claim_distribution(&f.alice, &id, &BONUS, &Vec::new(&f.env)),
        Err(Ok(VaultSafetyError::DistributionExpired))
    );
}

#[test]
fn non_positive_total_is_rejected() {
    let f = Fixture::new();
    let root = leaf(&f.env, &f.alice, BONUS);
    assert_eq!(
        f.vault
            .try_publish_distribution(&f.admin, &root, &f.token_addr, &0),
        Err(Ok(VaultSafetyError::InvalidDistribution))
    );
}
