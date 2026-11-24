//! Merkle-root allowlist for large permissioned lists (issue #872).
//!
//! Storing thousands of allowlisted addresses on-chain is expensive. Instead an
//! operator computes the addresses into a single Merkle root off-chain and
//! stores just that root; depositors prove membership with a proof, so the
//! contract keeps one 32-byte value no matter how long the list is.
//!
//! # Behaviour
//!
//! - `set_allowlist_root(admin, root)` stores the root. An **all-zero root
//!   disables** the allowlist, so a fresh vault (and any vault that has never
//!   set a root) behaves exactly as before: `deposit_with_proof` skips the
//!   check entirely.
//! - `deposit_with_proof(user, amount, proof)` verifies the proof against the
//!   stored root and, when it passes, deposits exactly as `deposit` does.
//! - `get_allowlist_root()` reports the configured root (or `None`).
//!
//! Integrators that rely on the allowlist must route deposits through
//! `deposit_with_proof`; a root only gates that entrypoint.
//!
//! # Domain-separated leaf hashing
//!
//! A leaf is `SHA-256(LEAF_DOMAIN || address_strkey)`, where `LEAF_DOMAIN` is
//! the ASCII string `"stellar-defi-vault:allowlist:v1:"`. Prefixing a domain
//! tag means this tree's leaves can never collide with another tree built over
//! the same addresses (for example the bonus distributor in issue #873, which
//! uses a different tag and additionally commits to the amount). The proof
//! generator in `scripts/build-merkle.ts` builds the identical leaf, and pairs
//! are sorted before hashing (see `compute_merkle_root`), so proofs are
//! order-independent and match the on-chain fold.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the root uses a raw
//! `Symbol`-keyed instance entry, as `balance.rs` does.

use soroban_sdk::{contractimpl, symbol_short, Address, Bytes, BytesN, Env, Symbol, Vec};

use crate::admin;
use crate::errors::VaultSafetyError;
use crate::vault::VaultContract;

/// Instance key holding the current allowlist Merkle root.
const ROOT_KEY: Symbol = symbol_short!("awl_root");

/// Largest proof `deposit_with_proof` accepts. A 32-deep tree covers up to
/// `2^32` leaves, far beyond any realistic allowlist; a longer proof is
/// rejected rather than walked, bounding the work an attacker can force.
pub const MAX_ALLOWLIST_PROOF_LEN: u32 = 32;

/// Domain-separation prefix mixed into every allowlist leaf. See the module
/// docs for why this must never be reused by another tree.
const LEAF_DOMAIN: &[u8] = b"stellar-defi-vault:allowlist:v1:";

/// The stored allowlist root, or `None` when no root has ever been set.
fn stored_root(env: &Env) -> Option<BytesN<32>> {
    env.storage().instance().get(&ROOT_KEY)
}

/// Whether `root` should be treated as "no allowlist". An all-zero root is the
/// sentinel for disabled, matching how an operator clears the list.
fn is_disabled(root: &BytesN<32>) -> bool {
    root.to_array() == [0u8; 32]
}

/// The allowlist leaf for `user`: `SHA-256(LEAF_DOMAIN || address_strkey)`.
pub fn allowlist_leaf(env: &Env, user: &Address) -> BytesN<32> {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(LEAF_DOMAIN);
    let addr = user.to_string();
    let len = addr.len();
    let mut raw = [0u8; 64];
    let slice = &mut raw[..len as usize];
    addr.copy_into_slice(slice);
    buf.extend_from_slice(slice);
    env.crypto().sha256(&buf).into()
}

#[contractimpl]
impl VaultContract {
    /// Admin: set the allowlist Merkle root. Deposits through
    /// `deposit_with_proof` must then prove membership. Passing an all-zero
    /// root disables the allowlist.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `NotInitialized`: no admin is stored.
    /// - `Unauthorized`: `admin` is not the stored admin.
    ///
    /// # Events
    /// Topic `("awl_root", admin)` with data `(root,)`.
    pub fn set_allowlist_root(
        env: Env,
        admin: Address,
        root: BytesN<32>,
    ) -> Result<(), VaultSafetyError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().set(&ROOT_KEY, &root);
        env.events()
            .publish((symbol_short!("awl_root"), admin), (root,));
        Ok(())
    }

    /// Deposit `amount` for `user`, proving membership when an allowlist is
    /// configured. With no root set (or an all-zero root) this behaves exactly
    /// like `deposit`.
    ///
    /// # Authorization
    /// `user` must sign (enforced by the underlying deposit path).
    ///
    /// # Errors
    /// - `MerkleProofTooLong`: `proof` exceeds [`MAX_ALLOWLIST_PROOF_LEN`].
    /// - `AllowlistProofRequired`: an allowlist is active and no proof was
    ///   supplied.
    /// - `InvalidMerkleProof`: the proof does not resolve to the stored root.
    /// - Plus every error `deposit` can return.
    ///
    /// # Events
    /// The same events `deposit` emits on success.
    pub fn deposit_with_proof(
        env: Env,
        user: Address,
        amount: i128,
        proof: Vec<BytesN<32>>,
    ) -> Result<i128, VaultSafetyError> {
        if let Some(root) = stored_root(&env) {
            if !is_disabled(&root) {
                if proof.len() > MAX_ALLOWLIST_PROOF_LEN {
                    return Err(VaultSafetyError::MerkleProofTooLong);
                }
                let leaf = allowlist_leaf(&env, &user);
                let empty = proof.is_empty();
                let computed = Self::compute_merkle_root(&env, leaf, proof);
                if computed != root {
                    return Err(if empty {
                        VaultSafetyError::AllowlistProofRequired
                    } else {
                        VaultSafetyError::InvalidMerkleProof
                    });
                }
            }
        }
        Self::deposit(env, user, amount, None).map_err(Into::into)
    }

    /// Read-only: the configured allowlist Merkle root, or `None` when the
    /// allowlist has never been set.
    pub fn get_allowlist_root(env: Env) -> Option<BytesN<32>> {
        stored_root(&env)
    }
}
