//! One-call compounding for pools whose reward and stake tokens differ.
//!
//! `zap_compound(user, min_out)` claims the caller's rewards, swaps them to the
//! stake token through the admin-registered router, and deposits the result
//! back into the caller's position, all inside one invocation.
//!
//! # Flow
//!
//! 1. `claim` runs exactly as a standalone claim, so the claim fee, minimum,
//!    cooldown and vesting rules all apply to what gets compounded.
//! 2. The claimed reward is pulled back from the caller and swapped to the
//!    stake token by the router configured with `set_swap_router`; the router
//!    delivers the output to the caller.
//! 3. The amount actually received (measured as the caller's stake-token
//!    balance change, never the router's self-reported figure) is deposited
//!    with `stake`, so every deposit rule applies.
//!
//! When the reward and stake tokens are the same, no router is needed: the
//! claimed reward is deposited directly.
//!
//! # Slippage and atomicity
//!
//! The call reverts with `SlippageExceeded` if the stake-token amount received
//! is below `min_out`. A Soroban invocation is atomic, so a failure at any step
//! (claim, swap, slippage check or deposit) reverts the whole zap, including
//! the claim.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so the router lives under a raw
//! `Symbol`-keyed instance entry, matching `balance.rs`.

use soroban_sdk::{contracterror, contractimpl, symbol_short, token, Address, Env, Symbol};

use crate::admin;
use crate::balance;
use crate::errors::VaultError;
use crate::vault::{DexRouterClient, VaultContract};

const SWAP_ROUTER_KEY: Symbol = symbol_short!("zap_rtr");

/// Errors for `zap_compound` and `set_swap_router`. `VaultError` is at
/// Soroban's 50-variant cap, so these get their own enum.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ZapError {
    /// Caller is not the stored admin.
    Unauthorized = 1,
    /// No admin / stake token is stored.
    NotInitialized = 2,
    /// Arithmetic overflowed.
    ArithmeticError = 3,
    /// Reward and stake tokens differ but no swap router is configured.
    RouterNotSet = 4,
    /// The router address is not usable (it is the vault itself).
    InvalidRouter = 5,
    /// The claim paid nothing, so there is nothing to compound.
    NothingToCompound = 6,
    /// The swap delivered less than `min_out` of the stake token.
    SlippageExceeded = 7,
    /// The pool is in withdraw-only mode, so deposits are closed.
    WithdrawOnlyMode = 8,
    /// The claim or deposit step was rejected (paused, sunsetting, cap, ...).
    StepRejected = 9,
}

impl From<VaultError> for ZapError {
    fn from(err: VaultError) -> Self {
        match err {
            VaultError::Unauthorized => ZapError::Unauthorized,
            VaultError::NotInitialized | VaultError::NoAdmin => ZapError::NotInitialized,
            VaultError::ArithmeticError => ZapError::ArithmeticError,
            _ => ZapError::StepRejected,
        }
    }
}

/// The configured swap router, if any.
pub fn get_router(env: &Env) -> Option<Address> {
    env.storage().instance().get(&SWAP_ROUTER_KEY)
}

#[cfg_attr(not(feature = "testutils"), contractimpl)]
impl VaultContract {
    /// Admin: register the router `zap_compound` swaps rewards through.
    ///
    /// The router must implement the same `swap(from_token, to_token,
    /// amount_in, min_amount_out, to) -> i128` shape as the DEX router used by
    /// `swap_and_stake`.
    ///
    /// # Authorization
    /// Requires admin.
    ///
    /// # Errors
    /// `InvalidRouter` if `router` is this contract.
    ///
    /// # Events
    /// Emits `zap_rtr_s` with `(router, ledger)`.
    pub fn set_swap_router(env: Env, admin: Address, router: Address) -> Result<(), ZapError> {
        admin::require_admin_as(&env, &admin)?;
        if router == env.current_contract_address() {
            return Err(ZapError::InvalidRouter);
        }
        env.storage().instance().set(&SWAP_ROUTER_KEY, &router);
        env.events()
            .publish((symbol_short!("zap_rtr_s"),), (router, env.ledger().sequence()));
        Ok(())
    }

    /// Read-only: the router `zap_compound` swaps through, if configured.
    pub fn get_swap_router(env: Env) -> Option<Address> {
        get_router(&env)
    }

    /// Claim rewards, swap them to the stake token and deposit the result in
    /// one call. Returns the shares minted by the deposit.
    ///
    /// `min_out` is the minimum amount of stake token the swap must deliver;
    /// the whole call reverts with `SlippageExceeded` otherwise. Pass `0` to
    /// accept any output (not recommended).
    ///
    /// # Authorization
    /// `user` must sign. The reward pull-back and the final deposit are
    /// transfers from `user`, so those token calls must be authorized too.
    ///
    /// # Errors
    /// - `WithdrawOnlyMode`: deposits are closed.
    /// - `RouterNotSet`: reward and stake tokens differ and no router is set.
    /// - `NothingToCompound`: the claim paid nothing.
    /// - `SlippageExceeded`: the swap output is below `min_out`.
    /// - `StepRejected`: the claim or deposit failed its own rules.
    ///
    /// # Events
    /// Emits `zap_cmp` with `(reward_in, stake_out, shares, ledger)`, besides
    /// the events of the claim and deposit.
    pub fn zap_compound(env: Env, user: Address, min_out: i128) -> Result<i128, ZapError> {
        user.require_auth();
        if crate::vault::withdraw_only_mode::is_active(&env) {
            return Err(ZapError::WithdrawOnlyMode);
        }

        let stake_token = VaultContract::get_stake_token(env.clone())?;
        let reward_token = balance::get_reward_token(&env).unwrap_or_else(|| stake_token.clone());
        let needs_swap = reward_token != stake_token;
        let router = if needs_swap {
            Some(get_router(&env).ok_or(ZapError::RouterNotSet)?)
        } else {
            None
        };

        // Claim under the normal claim rules (fee, minimum, cooldown, vesting).
        let reward_in = VaultContract::claim(env.clone(), user.clone())?;
        if reward_in <= 0 {
            return Err(ZapError::NothingToCompound);
        }

        let stake_out = match router {
            None => reward_in,
            Some(router_addr) => {
                let vault = env.current_contract_address();
                // `claim` paid the reward to the user; pull it back so the
                // router can take it from this contract.
                token::Client::new(&env, &reward_token).transfer(&user, &vault, &reward_in);

                let stake_client = token::Client::new(&env, &stake_token);
                let before = stake_client.balance(&user);
                DexRouterClient::new(&env, &router_addr).swap(
                    &reward_token,
                    &stake_token,
                    &reward_in,
                    &min_out,
                    &user,
                );
                stake_client
                    .balance(&user)
                    .checked_sub(before)
                    .ok_or(ZapError::ArithmeticError)?
            }
        };

        if stake_out <= 0 || stake_out < min_out {
            return Err(ZapError::SlippageExceeded);
        }

        let shares = VaultContract::stake(env.clone(), user.clone(), stake_out)?;

        env.events().publish(
            (symbol_short!("zap_cmp"), user),
            (reward_in, stake_out, shares, env.ledger().sequence()),
        );
        Ok(shares)
    }
}
