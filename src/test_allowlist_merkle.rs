#![cfg(test)]
//! Tests for the Merkle-root allowlist (issue #872).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, vec, Address, Bytes, BytesN, Env, Vec,
};

use crate::errors::VaultSafetyError;
use crate::vault::{VaultContract, VaultContractClient};

const STAKE: i128 = 100_000;
const DOMAIN: &[u8] = b"stellar-defi-vault:allowlist:v1:";

/// Must mirror `allowlist_merkle::allowlist_leaf`.
fn leaf(env: &Env, addr: &Address) -> BytesN<32> {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(DOMAIN);
    let s = addr.to_string();
    let mut raw = [0u8; 64];
    let slice = &mut raw[..s.len() as usize];
    s.copy_into_slice(slice);
    buf.extend_from_slice(slice);
    env.crypto().sha256(&buf).into()
}

/// Must mirror `compute_merkle_root`'s sorted-pair fold.
fn pair(env: &Env, a: &BytesN<32>, b: &BytesN<32>) -> BytesN<32> {
    let aa = a.to_array();
    let bb = b.to_array();
    let mut c = Bytes::new(env);
    if aa[0] <= bb[0] {
        c.extend_from_slice(&aa);
        c.extend_from_slice(&bb);
    } else {
        c.extend_from_slice(&bb);
        c.extend_from_slice(&aa);
    }
    env.crypto().sha256(&c).into()
}

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    alice: Address,
    bob: Address,
    carol: Address,
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
        let bob = Address::generate(&env);
        let carol = Address::generate(&env);

        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);
        for who in [&alice, &bob, &carol] {
            token_admin.mint(who, &1_000_000);
        }

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &0_u32, &None, &None);

        Fixture { env, vault, admin, alice, bob, carol }
    }

    fn empty_proof(&self) -> Vec<BytesN<32>> {
        Vec::new(&self.env)
    }
}

#[test]
fn empty_root_disables_the_check() {
    let f = Fixture::new();
    // No root configured: any address may deposit.
    f.vault
        .deposit_with_proof(&f.alice, &STAKE, &f.empty_proof());
    assert_eq!(f.vault.get_depositor_count(), 1);
}

#[test]
fn valid_proof_is_accepted() {
    let f = Fixture::new();
    let root = pair(&f.env, &leaf(&f.env, &f.alice), &leaf(&f.env, &f.bob));
    f.vault.set_allowlist_root(&f.admin, &root);

    let proof = vec![&f.env, leaf(&f.env, &f.bob)];
    let shares = f.vault.deposit_with_proof(&f.alice, &STAKE, &proof);
    assert!(shares > 0);
    assert_eq!(f.vault.get_depositor_count(), 1);
}

#[test]
fn single_entry_list_accepts_empty_proof() {
    let f = Fixture::new();
    let root = leaf(&f.env, &f.alice);
    f.vault.set_allowlist_root(&f.admin, &root);

    // A one-leaf tree's root is the leaf itself, so the proof is empty.
    f.vault
        .deposit_with_proof(&f.alice, &STAKE, &f.empty_proof());
    assert_eq!(f.vault.get_depositor_count(), 1);
}

#[test]
fn wrong_proof_is_rejected() {
    let f = Fixture::new();
    let root = pair(&f.env, &leaf(&f.env, &f.alice), &leaf(&f.env, &f.bob));
    f.vault.set_allowlist_root(&f.admin, &root);

    let bogus = vec![&f.env, BytesN::from_array(&f.env, &[7u8; 32])];
    assert_eq!(
        f.vault.try_deposit_with_proof(&f.alice, &STAKE, &bogus),
        Err(Ok(VaultSafetyError::InvalidMerkleProof))
    );
}

#[test]
fn tampered_leaf_is_rejected() {
    let f = Fixture::new();
    let root = pair(&f.env, &leaf(&f.env, &f.alice), &leaf(&f.env, &f.bob));
    f.vault.set_allowlist_root(&f.admin, &root);

    // Carol is not in the tree, but presents Bob's sibling.
    let proof = vec![&f.env, leaf(&f.env, &f.bob)];
    assert_eq!(
        f.vault.try_deposit_with_proof(&f.carol, &STAKE, &proof),
        Err(Ok(VaultSafetyError::InvalidMerkleProof))
    );
}

#[test]
fn missing_proof_is_rejected_when_allowlist_active() {
    let f = Fixture::new();
    let root = pair(&f.env, &leaf(&f.env, &f.alice), &leaf(&f.env, &f.bob));
    f.vault.set_allowlist_root(&f.admin, &root);

    assert_eq!(
        f.vault
            .try_deposit_with_proof(&f.alice, &STAKE, &f.empty_proof()),
        Err(Ok(VaultSafetyError::AllowlistProofRequired))
    );
}

#[test]
fn over_long_proof_is_rejected() {
    let f = Fixture::new();
    let root = pair(&f.env, &leaf(&f.env, &f.alice), &leaf(&f.env, &f.bob));
    f.vault.set_allowlist_root(&f.admin, &root);

    let mut proof = Vec::new(&f.env);
    for _ in 0..33 {
        proof.push_back(BytesN::from_array(&f.env, &[1u8; 32]));
    }
    assert_eq!(
        f.vault.try_deposit_with_proof(&f.alice, &STAKE, &proof),
        Err(Ok(VaultSafetyError::MerkleProofTooLong))
    );
}

#[test]
fn zero_root_disables_the_check() {
    let f = Fixture::new();
    // An explicit all-zero root is the "disabled" sentinel.
    f.vault
        .set_allowlist_root(&f.admin, &BytesN::from_array(&f.env, &[0u8; 32]));
    f.vault
        .deposit_with_proof(&f.alice, &STAKE, &f.empty_proof());
    assert_eq!(f.vault.get_depositor_count(), 1);
}
