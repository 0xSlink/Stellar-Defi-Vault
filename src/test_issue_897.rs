#![cfg(test)]
//! Tests for versioned migrate() entry point (issue #897).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _},
    Address, Env, Symbol, symbol_short,
};

use crate::{
    vault_extensions_897::{CURRENT_STORAGE_VERSION},
    vault::{VaultContract, VaultContractClient},
};

fn setup_vault<'a>(env: &Env) -> (VaultContractClient<'a>, Address, Address) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(token_admin);
    let vault_id = env.register(VaultContract, ());
    let client = VaultContractClient::new(env, &vault_id);
    client.initialize(&admin, &token_id);
    (client, admin, vault_id)
}

#[test]
fn test_initial_storage_version_defaults_to_one() {
    let env = Env::default();
    let (client, _, _) = setup_vault(&env);
    assert_eq!(client.get_storage_version(), 1);
}

#[test]
fn test_migrate_upgrades_version_and_emits_event() {
    let env = Env::default();
    let (client, admin, vault_id) = setup_vault(&env);

    assert_eq!(client.get_storage_version(), 1);

    let (from_ver, to_ver) = client.migrate(&admin);
    assert_eq!(from_ver, 1);
    assert_eq!(to_ver, CURRENT_STORAGE_VERSION);
    assert_eq!(client.get_storage_version(), CURRENT_STORAGE_VERSION);

    // Verify v2 migration step took effect in storage
    let v2_flag = env.as_contract(&vault_id, || {
        env.storage().instance().get::<Symbol, bool>(&symbol_short!("mig_v2_ok")).unwrap_or(false)
    });
    assert!(v2_flag);
}

#[test]
#[should_panic(expected = "AlreadyMigrated")]
fn test_migrate_reverts_if_already_migrated() {
    let env = Env::default();
    let (client, admin, _) = setup_vault(&env);

    client.migrate(&admin);
    // Second migration must fail with AlreadyMigrated
    client.migrate(&admin);
}
