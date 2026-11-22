//! Separate upgrader role with a queued, delayed Wasm upgrade.
//!
//! The power to replace the contract's Wasm is split from day-to-day admin:
//!
//! 1. The admin appoints an `upgrader` with `set_upgrader`.
//! 2. The upgrader announces an upgrade with `queue_upgrade(upgrader, hash)`.
//!    It becomes executable `UPGRADE_DELAY_LEDGERS` ledgers later.
//! 3. During that delay the upgrader or the guardian can call
//!    `cancel_upgrade` to drop it.
//! 4. After the delay the upgrader calls `execute_upgrade`.
//!
//! Once an upgrader is configured, the immediate admin-only `upgrade_wasm`
//! path is disabled (see `vault.rs`), so the delay cannot be bypassed.
//!
//! # Storage
//!
//! `DataKey` is at Soroban's 50-variant cap, so this uses raw `Symbol`-keyed
//! instance storage, matching the other extension modules.

use soroban_sdk::{contractimpl, symbol_short, Address, BytesN, Env, Symbol};

use crate::admin;
use crate::constants::LEDGERS_PER_DAY;
use crate::errors::UpgradeError;
use crate::vault::VaultContract;

/// Ledgers between `queue_upgrade` and the earliest `execute_upgrade` (~2 days).
pub const UPGRADE_DELAY_LEDGERS: u32 = LEDGERS_PER_DAY * 2;

const UPGRADER_KEY: Symbol = symbol_short!("upgrader");
const PENDING_KEY: Symbol = symbol_short!("upg_pend");

/// The configured upgrader, if any.
pub fn get_upgrader(env: &Env) -> Option<Address> {
    env.storage().instance().get(&UPGRADER_KEY)
}

fn get_pending(env: &Env) -> Option<(BytesN<32>, u32)> {
    env.storage().instance().get(&PENDING_KEY)
}

/// Check that `caller` is the stored upgrader and has signed.
fn require_upgrader(env: &Env, caller: &Address) -> Result<(), UpgradeError> {
    match get_upgrader(env) {
        Some(upgrader) if &upgrader == caller => {
            caller.require_auth();
            Ok(())
        }
        _ => Err(UpgradeError::Unauthorized),
    }
}

#[contractimpl]
impl VaultContract {
    /// Admin: appoint (or replace) the upgrader. Replacing the upgrader does
    /// not clear an already queued upgrade; the guardian or the new upgrader
    /// can still cancel it.
    ///
    /// # Authorization
    /// `admin` must be the stored admin and must sign.
    ///
    /// # Errors
    /// - `NotInitialized`: no admin is stored.
    /// - `Unauthorized`: `admin` is not the stored admin.
    ///
    /// # Events
    /// - Topic `("upgr_set",)` with data `(upgrader, ledger_sequence)`.
    pub fn set_upgrader(env: Env, admin: Address, upgrader: Address) -> Result<(), UpgradeError> {
        admin::require_admin_as(&env, &admin)?;
        env.storage().instance().set(&UPGRADER_KEY, &upgrader);
        env.events().publish(
            (symbol_short!("upgr_set"),),
            (upgrader, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the configured upgrader, if any.
    pub fn get_upgrader(env: Env) -> Option<Address> {
        get_upgrader(&env)
    }

    /// Upgrader: announce an upgrade to `wasm_hash`. It can be executed once
    /// `UPGRADE_DELAY_LEDGERS` ledgers have passed.
    ///
    /// # Authorization
    /// `upgrader` must be the configured upgrader and must sign.
    ///
    /// # Errors
    /// - `Unauthorized`: `upgrader` is not the configured upgrader.
    /// - `UpgradeAlreadyQueued`: an upgrade is already pending; cancel it first.
    ///
    /// # Events
    /// - Topic `("upg_queue",)` with data `(upgrader, wasm_hash, executable_at)`.
    pub fn queue_upgrade(
        env: Env,
        upgrader: Address,
        wasm_hash: BytesN<32>,
    ) -> Result<(), UpgradeError> {
        require_upgrader(&env, &upgrader)?;
        if get_pending(&env).is_some() {
            return Err(UpgradeError::UpgradeAlreadyQueued);
        }
        let executable_at = env
            .ledger()
            .sequence()
            .checked_add(UPGRADE_DELAY_LEDGERS)
            .ok_or(UpgradeError::ArithmeticError)?;
        env.storage()
            .instance()
            .set(&PENDING_KEY, &(wasm_hash.clone(), executable_at));
        env.events().publish(
            (symbol_short!("upg_queue"),),
            (upgrader, wasm_hash, executable_at),
        );
        Ok(())
    }

    /// Upgrader: replace the contract Wasm with the queued hash, once the
    /// delay has elapsed.
    ///
    /// # Authorization
    /// `upgrader` must be the configured upgrader and must sign.
    ///
    /// # Errors
    /// - `Unauthorized`: `upgrader` is not the configured upgrader.
    /// - `NoUpgradeQueued`: nothing is pending.
    /// - `UpgradeNotReady`: the delay has not elapsed yet.
    ///
    /// # Events
    /// - Topic `("upg_exec",)` with data `(upgrader, wasm_hash, ledger_sequence)`.
    pub fn execute_upgrade(env: Env, upgrader: Address) -> Result<(), UpgradeError> {
        require_upgrader(&env, &upgrader)?;
        let (wasm_hash, executable_at) =
            get_pending(&env).ok_or(UpgradeError::NoUpgradeQueued)?;
        if env.ledger().sequence() < executable_at {
            return Err(UpgradeError::UpgradeNotReady);
        }
        env.storage().instance().remove(&PENDING_KEY);
        env.deployer()
            .update_current_contract_wasm(wasm_hash.clone());
        env.events().publish(
            (symbol_short!("upg_exec"),),
            (upgrader, wasm_hash, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Cancel the pending upgrade at any time before it is executed.
    ///
    /// # Authorization
    /// `caller` must be the configured upgrader or the guardian, and must sign.
    ///
    /// # Errors
    /// - `Unauthorized`: `caller` is neither the upgrader nor the guardian.
    /// - `NoUpgradeQueued`: nothing is pending.
    ///
    /// # Events
    /// - Topic `("upg_cncl",)` with data `(caller, wasm_hash, ledger_sequence)`.
    pub fn cancel_upgrade(env: Env, caller: Address) -> Result<(), UpgradeError> {
        let is_upgrader = get_upgrader(&env).as_ref() == Some(&caller);
        let is_guardian = crate::guardian_pause::get_guardian(&env).as_ref() == Some(&caller);
        if !is_upgrader && !is_guardian {
            return Err(UpgradeError::Unauthorized);
        }
        caller.require_auth();
        let (wasm_hash, _) = get_pending(&env).ok_or(UpgradeError::NoUpgradeQueued)?;
        env.storage().instance().remove(&PENDING_KEY);
        env.events().publish(
            (symbol_short!("upg_cncl"),),
            (caller, wasm_hash, env.ledger().sequence()),
        );
        Ok(())
    }

    /// Read-only: the pending upgrade as `(wasm_hash, executable_at_ledger)`.
    pub fn get_pending_upgrade(env: Env) -> Option<(BytesN<32>, u32)> {
        get_pending(&env)
    }
}
