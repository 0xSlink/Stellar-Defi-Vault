//! Versioned migrate() entry point for post-upgrade state migrations (issue #897).
//!
//! Upgrades that modify storage layouts or initialize newly introduced entries
//! require a controlled, one-time migration step rather than ad-hoc scripts.

use soroban_sdk::{
    contractimpl, symbol_short, Address, Env, Symbol,
};

use crate::admin;
use crate::vault::VaultContract;

/// The target schema version implemented by the current contract code.
pub const CURRENT_STORAGE_VERSION: u32 = 2;

/// Key storing the schema version in instance storage.
pub const STORAGE_VERSION_KEY: Symbol = symbol_short!("stor_ver");

#[contractimpl]
impl VaultContract {
    /// Query current stored storage version. Defaults to 1 for unmigrated contracts.
    pub fn get_storage_version(env: Env) -> u32 {
        env.storage().instance().get(&STORAGE_VERSION_KEY).unwrap_or(1)
    }

    /// Perform a versioned state migration from the stored version up to CURRENT_STORAGE_VERSION.
    /// Only the admin/upgrader can invoke this.
    /// Reverts with panic("AlreadyMigrated") if the contract is already at or above CURRENT_STORAGE_VERSION.
    pub fn migrate(env: Env, upgrader: Address) -> (u32, u32) {
        admin::require_admin(&env, &upgrader);

        let stored_version = Self::get_storage_version(env.clone());
        if stored_version >= CURRENT_STORAGE_VERSION {
            panic!("AlreadyMigrated");
        }

        // Run sequential migration steps
        let mut ver = stored_version;
        if ver == 1 {
            // Migration Step 1 -> 2:
            // Set flag verifying v2 migration was executed
            let init_flag: Symbol = symbol_short!("mig_v2_ok");
            env.storage().instance().set(&init_flag, &true);
            ver = 2;
        }

        // Commit updated storage version
        env.storage().instance().set(&STORAGE_VERSION_KEY, &ver);

        // Publish migrated event (from_version, to_version)
        env.events().publish(
            (Symbol::new(&env, "migrated"), upgrader),
            (stored_version, ver),
        );

        (stored_version, ver)
    }
}
