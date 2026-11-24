//! Merkle distributor for off-chain computed bonus rewards (issue #873).
//!
//! Campaigns (points, partner rewards, retroactive bonuses) are computed
//! off-chain. Rather than write one storage entry per recipient, the operator
//! publishes a single Merkle root and escrows the tokens; each recipient then
//! claims their entitlement with a proof, and the admin can reclaim whatever is
//! left once the distribution expires.
//!
//! # Lifecycle
//!
//! 1. `publish_distribution(admin, root, token, total)` escrows `total` tokens
//!    from `admin` into the contract and returns the new distribution id. The
//!    distribution expires [`DISTRIBUTION_WINDOW_LEDGERS`] ledgers later.
//! 2. `claim_distribution(user, id, amount, proof)` pays `amount` to `user`
//!    once. Each entitlement is tracked with a per-leaf claimed flag, so a
//!    second claim (or a claim for the wrong amount, which cannot prove against
//!    the root) reverts.
//! 3. `recover_unclaimed(admin, id)` returns the unclaimed remainder to the
//!    admin after the expiry ledger has passed.
//!
//! # Domain-separated leaf hashing
//!
//! A leaf is `SHA-256(LEAF_DOMAIN || address_strkey || amount_be_i128)`, where
//! `LEAF_DOMAIN` is `"stellar-defi-vault:distribution:v1:"`. The domain tag
//! keeps these leaves distinct from the allowlist tree in issue #872, and
//! committing to the amount ties each proof to one exact entitlement, so a
//! claimant cannot prove a different value. `scripts/build-merkle.ts` builds
//! the identical leaf, and `compute_merkle_root` sorts each pair before
//! hashing, so the off-chain proof and the on-chain fold agree.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! storage, as `balance.rs` does.

use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::{contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, Symbol, Vec};

use crate::admin;
use crate::constants::LEDGERS_PER_DAY;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Instance key holding the next distribution id to hand out.
const NEXT_ID_KEY: Symbol = symbol_short!("dist_nxt");
/// Persistent prefix mapping a distribution id to its `Distribution`.
const DIST_KEY: Symbol = symbol_short!("dist");
/// Persistent prefix marking `(id, user)` as already claimed.
const CLAIM_KEY: Symbol = symbol_short!("dist_clm");

/// Largest proof `claim_distribution` accepts; a 32-deep tree covers up to
/// `2^32` leaves, bounding the work a claimant can force.
pub const MAX_DISTRIBUTION_PROOF_LEN: u32 = 32;

/// How long, in ledgers, a distribution stays claimable after publication
/// (30 days at 5s/ledger) before the admin may recover the remainder.
pub const DISTRIBUTION_WINDOW_LEDGERS: u32 = LEDGERS_PER_DAY * 30;

/// Domain-separation prefix mixed into every distribution leaf. See the module
/// docs for why this must never be reused by another tree.
const LEAF_DOMAIN: &[u8] = b"stellar-defi-vault:distribution:v1:";

/// An escrowed bonus distribution.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Distribution {
    /// Merkle root over `(address, amount)` entitlements.
    pub root: BytesN<32>,
    /// Token that is escrowed and paid out.
    pub token: Address,
    /// Total amount escrowed at publication.
    pub total: i128,
    /// Amount still claimable (decreases as claims land).
    pub remaining: i128,
    /// First ledger at which the admin may recover the remainder.
    pub expiry_ledger: u32,
    /// Ledger the distribution was published at.
    pub published_at: u32,
    /// Whether the remainder has been recovered by the admin.
    pub recovered: bool,
}

fn next_id(env: &Env) -> u32 {
    env.storage().instance().get(&NEXT_ID_KEY).unwrap_or(0)
}

fn set_next_id(env: &Env, id: u32) {
    env.storage().instance().set(&NEXT_ID_KEY, &id);
}

fn stored(env: &Env, id: u32) -> Option<Distribution> {
    env.storage().persistent().get(&(DIST_KEY, id))
}

fn store(env: &Env, id: u32, dist: &Distribution) {
    env.storage().persistent().set(&(DIST_KEY, id), dist);
}

fn is_claimed(env: &Env, id: u32, user: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&(CLAIM_KEY, id, user.clone()))
        .unwrap_or(false)
}

fn set_claimed(env: &Env, id: u32, user: &Address) {
    env.storage()
        .persistent()
        .set(&(CLAIM_KEY, id, user.clone()), &true);
}

/// The distributor leaf for `(user, amount)`:
/// `SHA-256(LEAF_DOMAIN || address_strkey || amount_be_i128)`.
pub fn distribution_leaf(env: &Env, user: &Address, amount: i128) -> BytesN<32> {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(LEAF_DOMAIN);
    let addr = user.to_string();
    let len = addr.len();
    let mut raw = [0u8; 64];
    let slice = &mut raw[..len as usize];
    addr.copy_into_slice(slice);
    buf.extend_from_slice(slice);
    buf.extend_from_slice(&amount.to_be_bytes());
    env.crypto().sha256(&buf).into()
}

#[contractimpl]
impl VaultContract {
    /// Admin: publish a bonus distribution, escrowing `total` of `token` from
    /// `admin` into the contract, and return its id. Recipients claim with
    /// `claim_distribution`; unclaimed funds are recoverable after the expiry
    /// window.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign. The escrow transfer also
    /// requires `admin`'s token authorization.
    ///
    /// # Errors
    /// - `NotInitialized`: no admin is stored.
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `InvalidDistribution`: `total <= 0`.
    /// - Token errors when the escrow transfer fails.
    ///
    /// # Events
    /// Topic `("d_pub", admin)` with data `(id, token, total, expiry_ledger)`.
    pub fn publish_distribution(
        env: Env,
        admin: Address,
        root: BytesN<32>,
        token: Address,
        total: i128,
    ) -> Result<u32, VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        if total <= 0 {
            return Err(VaultSafetyError::InvalidDistribution);
        }
        TokenClient::new(&env, &token).transfer(
            &admin,
            &env.current_contract_address(),
            &total,
        );

        let id = next_id(&env);
        set_next_id(&env, id.checked_add(1).ok_or(VaultSafetyError::ArithmeticError)?);

        let published_at = env.ledger().sequence();
        let expiry_ledger = published_at
            .checked_add(DISTRIBUTION_WINDOW_LEDGERS)
            .ok_or(VaultSafetyError::ArithmeticError)?;
        let dist = Distribution {
            root,
            token: token.clone(),
            total,
            remaining: total,
            expiry_ledger,
            published_at,
            recovered: false,
        };
        store(&env, id, &dist);
        env.events().publish(
            (symbol_short!("d_pub"), admin),
            (id, token, total, expiry_ledger),
        );
        Ok(id)
    }

    /// Claim the `amount` entitled to `user` under distribution `id`. Each
    /// entitlement can be claimed once; the amount is bound to the leaf, so a
    /// claim for any other value cannot prove against the root.
    ///
    /// # Authorization
    /// `user` must sign.
    ///
    /// # Errors
    /// - `DistributionNotFound`: no distribution with that id.
    /// - `DistributionAlreadyRecovered`: the remainder was already recovered.
    /// - `DistributionExpired`: the expiry ledger has passed.
    /// - `DistributionAlreadyClaimed`: `user` already claimed from `id`.
    /// - `InsufficientDistributionBalance`: `amount <= 0` or exceeds what is
    ///   left in the escrow.
    /// - `MerkleProofTooLong`: `proof` exceeds [`MAX_DISTRIBUTION_PROOF_LEN`].
    /// - `InvalidMerkleProof`: the proof does not resolve to the stored root.
    ///
    /// # Events
    /// Topic `("d_claim", user)` with data `(id, amount)`.
    pub fn claim_distribution(
        env: Env,
        user: Address,
        id: u32,
        amount: i128,
        proof: Vec<BytesN<32>>,
    ) -> Result<(), VaultSafetyError> {
        user.require_auth();
        let mut dist = stored(&env, id).ok_or(VaultSafetyError::DistributionNotFound)?;
        if dist.recovered {
            return Err(VaultSafetyError::DistributionAlreadyRecovered);
        }
        if env.ledger().sequence() > dist.expiry_ledger {
            return Err(VaultSafetyError::DistributionExpired);
        }
        if is_claimed(&env, id, &user) {
            return Err(VaultSafetyError::DistributionAlreadyClaimed);
        }
        if amount <= 0 || amount > dist.remaining {
            return Err(VaultSafetyError::InsufficientDistributionBalance);
        }
        if proof.len() > MAX_DISTRIBUTION_PROOF_LEN {
            return Err(VaultSafetyError::MerkleProofTooLong);
        }

        let leaf = distribution_leaf(&env, &user, amount);
        let computed = Self::compute_merkle_root(&env, leaf, proof);
        if computed != dist.root {
            return Err(VaultSafetyError::InvalidMerkleProof);
        }

        set_claimed(&env, id, &user);
        dist.remaining -= amount;
        store(&env, id, &dist);
        TokenClient::new(&env, &dist.token).transfer(
            &env.current_contract_address(),
            &user,
            &amount,
        );
        env.events()
            .publish((symbol_short!("d_claim"), user), (id, amount));
        Ok(())
    }

    /// Admin: after the expiry ledger has passed, recover the unclaimed
    /// remainder of distribution `id` back to `admin`. Can only be done once.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `NotInitialized`: no admin is stored.
    /// - `Unauthorized`: `admin` is not the stored admin.
    /// - `DistributionNotFound`: no distribution with that id.
    /// - `DistributionNotExpired`: the expiry ledger has not yet passed.
    /// - `DistributionAlreadyRecovered`: the remainder was already recovered.
    ///
    /// # Events
    /// Topic `("d_recov", admin)` with data `(id, amount)`.
    pub fn recover_unclaimed(
        env: Env,
        admin: Address,
        id: u32,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        let mut dist = stored(&env, id).ok_or(VaultSafetyError::DistributionNotFound)?;
        if env.ledger().sequence() <= dist.expiry_ledger {
            return Err(VaultSafetyError::DistributionNotExpired);
        }
        if dist.recovered {
            return Err(VaultSafetyError::DistributionAlreadyRecovered);
        }

        let amount = dist.remaining;
        dist.remaining = 0;
        dist.recovered = true;
        store(&env, id, &dist);
        if amount > 0 {
            TokenClient::new(&env, &dist.token).transfer(
                &env.current_contract_address(),
                &admin,
                &amount,
            );
        }
        env.events()
            .publish((symbol_short!("d_recov"), admin), (id, amount));
        Ok(())
    }

    /// Read-only: the distribution with `id`, or `None` if there is none.
    pub fn get_distribution(env: Env, id: u32) -> Option<Distribution> {
        stored(&env, id)
    }

    /// Read-only: whether `user` has already claimed from distribution `id`.
    pub fn is_distribution_claimed(env: Env, id: u32, user: Address) -> bool {
        is_claimed(&env, id, &user)
    }
}
