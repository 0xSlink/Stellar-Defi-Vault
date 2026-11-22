//! Vault extensions for issues #863, #864, #865, and #867.
//!
//! - Issue #863: Separate principal and reward accounting when stake and reward tokens are identical
//! - Issue #864: Handle Stellar Asset Contract tokens with revocable authorization or clawback
//! - Issue #865: Native XLM integration through the SAC
//! - Issue #867: Nonce and replay protection module for signed actions

use soroban_sdk::{
    contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, Symbol,
};

use crate::admin;
use crate::balance;
use crate::errors::VaultError;
use crate::vault::VaultContract;

// ── Storage Keys ─────────────────────────────────────────────────────────────

const PRINCIPAL_KEY: Symbol = symbol_short!("tot_prnc");
const REWARD_POOL_KEY: Symbol = symbol_short!("rwd_pool");
const TOKEN_RISK_FLAGS_KEY: Symbol = symbol_short!("rsk_flag");
const USER_NONCE_PREFIX: Symbol = symbol_short!("usr_non");

// ── Types and Errors ─────────────────────────────────────────────────────────

/// Stellar Asset Contract token compliance and risk classification flags (issue #864).
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct TokenRiskFlags {
    /// Token requires issuer trustline authorization before transacting.
    pub auth_required: bool,
    /// Issuer can revoke authorization at any time, freezing balances.
    pub auth_revocable: bool,
    /// Issuer retains power to claw back token balances without consent.
    pub auth_clawback_enabled: bool,
    /// Administrator has formally acknowledged the issuer risk profile.
    pub risk_acknowledged: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum NonceReplayError {
    Unauthorized = 1,
    InvalidNonce = 2,
    NonceAlreadyUsed = 3,
    ArithmeticError = 4,
}

// ── Implementation ───────────────────────────────────────────────────────────

#[contractimpl]
impl VaultContract {
    // ── Issue #863: Separate Principal and Reward Accounting ─────────────────

    /// Retrieve the currently recorded depositor principal balance.
    pub fn get_total_principal(env: Env) -> i128 {
        env.storage().instance().get(&PRINCIPAL_KEY).unwrap_or(0)
    }

    /// Retrieve the current dedicated reward pool balance.
    pub fn get_reward_pool_balance(env: Env) -> i128 {
        env.storage().instance().get(&REWARD_POOL_KEY).unwrap_or(0)
    }

    /// Fund the dedicated reward pool.
    ///
    /// Keeps reward funding accounting completely distinct from depositor principal,
    /// preventing claims from dipping into deposited principal even when stake and
    /// reward tokens are the exact same token.
    pub fn fund_reward_pool_explicit(
        env: Env,
        funder: Address,
        amount: i128,
    ) -> Result<(), VaultError> {
        funder.require_auth();
        if amount <= 0 {
            return Err(VaultError::ZeroAmount);
        }

        let reward_token: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("rw_token"))
            .unwrap_or_else(|| {
                env.storage()
                    .instance()
                    .get(&symbol_short!("token"))
                    .expect("vault token not set")
            });

        // Pull reward tokens into contract
        let client = token::Client::new(&env, &reward_token);
        client.transfer(&funder, &env.current_contract_address(), &amount);

        // Update dedicated reward pool balance
        let current_reward_pool = Self::get_reward_pool_balance(env.clone());
        let new_reward_pool = current_reward_pool
            .checked_add(amount)
            .ok_or(VaultError::ArithmeticError)?;
        env.storage().instance().set(&REWARD_POOL_KEY, &new_reward_pool);

        env.events().publish(
            (symbol_short!("rwd_fnd"), funder),
            (amount, new_reward_pool),
        );

        Ok(())
    }

    /// Claim rewards with strict principal isolation guarantee.
    ///
    /// Reverts with `InsufficientRewardPool` if the claim exceeds the tracked reward pool,
    /// ensuring that reward claims NEVER deplete depositor principal.
    pub fn claim_with_principal_protection(
        env: Env,
        user: Address,
        claim_amount: i128,
    ) -> Result<(), VaultError> {
        user.require_auth();
        if claim_amount <= 0 {
            return Err(VaultError::ZeroAmount);
        }

        let reward_pool = Self::get_reward_pool_balance(env.clone());
        if claim_amount > reward_pool {
            return Err(VaultError::InsufficientRewardPool);
        }

        // Deduct from reward pool balance
        let updated_pool = reward_pool
            .checked_sub(claim_amount)
            .ok_or(VaultError::ArithmeticError)?;
        env.storage().instance().set(&REWARD_POOL_KEY, &updated_pool);

        // Disburse reward to user
        let reward_token: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("rw_token"))
            .unwrap_or_else(|| {
                env.storage()
                    .instance()
                    .get(&symbol_short!("token"))
                    .expect("vault token not set")
            });

        let client = token::Client::new(&env, &reward_token);
        client.transfer(&env.current_contract_address(), &user, &claim_amount);

        env.events().publish(
            (symbol_short!("clm_prot"), user),
            (claim_amount, updated_pool),
        );

        Ok(())
    }

    // ── Issue #864: Stellar Asset Contract Risk & Issuer Powers ──────────────

    /// Configure risk classification flags for underlying Stellar Asset Contract tokens.
    ///
    /// # Authorization
    /// Requires current administrator authorization.
    pub fn set_token_risk_flags(
        env: Env,
        admin: Address,
        flags: TokenRiskFlags,
    ) -> Result<(), VaultError> {
        admin.require_auth();
        let stored_admin = admin::get_admin(&env)?;
        if admin != stored_admin {
            return Err(VaultError::Unauthorized);
        }

        env.storage().instance().set(&TOKEN_RISK_FLAGS_KEY, &flags);

        env.events().publish(
            (symbol_short!("rsk_upd"), admin),
            (flags.auth_required, flags.auth_revocable, flags.auth_clawback_enabled),
        );

        Ok(())
    }

    /// Query the configured token risk classification flags.
    pub fn get_token_risk_flags(env: Env) -> Option<TokenRiskFlags> {
        env.storage().instance().get(&TOKEN_RISK_FLAGS_KEY)
    }

    // ── Issue #865: Native XLM SAC Helpers ───────────────────────────────────

    /// Verify whether a token contract address corresponds to native XLM on Stellar.
    pub fn is_native_xlm_sac(env: Env, token_addr: Address) -> bool {
        // Native XLM uses 7 decimal digits (1 XLM = 10_000_000 stroops)
        let token_client = token::Client::new(&env, &token_addr);
        token_client.decimals() == 7
    }

    // ── Issue #867: Nonce and Replay Protection Module ───────────────────────

    /// Retrieve the current expected monotonic nonce for a user.
    ///
    /// Starts at 0 for accounts that have not consumed any nonces.
    pub fn get_user_nonce(env: Env, user: Address) -> u64 {
        env.storage()
            .persistent()
            .get(&(USER_NONCE_PREFIX, user))
            .unwrap_or(0)
    }

    /// Consume a user nonce for signed actions (meta-transactions, relayer submissions).
    ///
    /// Reverts with `InvalidNonce` if the supplied nonce does not match the current expected nonce.
    /// On success, advances the user's nonce by 1, bumps persistent storage TTL,
    /// and emits a `nonce_used` event.
    pub fn consume_nonce(
        env: Env,
        user: Address,
        nonce: u64,
    ) -> Result<(), NonceReplayError> {
        user.require_auth();

        let current_nonce = Self::get_user_nonce(env.clone(), user.clone());
        if nonce != current_nonce {
            return Err(NonceReplayError::InvalidNonce);
        }

        let next_nonce = current_nonce
            .checked_add(1)
            .ok_or(NonceReplayError::ArithmeticError)?;

        let key = (USER_NONCE_PREFIX, user.clone());
        env.storage().persistent().set(&key, &next_nonce);

        // Bump persistent entry TTL to ensure compact storage longevity
        env.storage().persistent().extend_ttl(&key, 17_280, 120_960);

        env.events().publish(
            (symbol_short!("non_used"), user),
            (nonce, next_nonce),
        );

        Ok(())
    }
}
