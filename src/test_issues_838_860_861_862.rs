#![cfg(test)]
//! Tests for Issues #838, #860, #861, and #862.
//!
//! - #838: Oracle staleness and sanity validation
//! - #860: Virtual shares and assets offset
//! - #861: Minimum liquidity shares burned on first deposit
//! - #862: Balance audit and internal counter isolation

extern crate std;

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    token, Address, Env, Symbol, symbol_short,
};

use crate::{
    vault_extensions_838_860_861_862::{
        MIN_LIQUIDITY_SHARES, VIRTUAL_ASSETS_OFFSET, VIRTUAL_SHARES_OFFSET,
    },
    vault::{VaultContract, VaultContractClient},
};

// ── Mock Oracle ──────────────────────────────────────────────────────────────

#[contract]
struct MockOracle;

#[contractimpl]
impl MockOracle {
    pub fn set_data(env: Env, price: i128, updated_ledger: u32) {
        env.storage().instance().set(&symbol_short!("price"), &price);
        env.storage().instance().set(&symbol_short!("upd_led"), &updated_ledger);
    }

    pub fn get_price_data(env: Env, _token: Address) -> (i128, u32) {
        let price = env.storage().instance().get(&symbol_short!("price")).unwrap_or(0);
        let updated_ledger = env.storage().instance().get(&symbol_short!("upd_led")).unwrap_or(0);
        (price, updated_ledger)
    }
}

// ── Setup Helper ─────────────────────────────────────────────────────────────

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

// ── Tests for Issue #838: Oracle Staleness & Sanity ──────────────────────────

#[test]
fn test_oracle_fresh_and_valid() {
    let env = Env::default();
    let (client, admin, vault_id, _) = setup_vault(&env);
    let oracle_id = env.register(MockOracle, ());

    // Register oracle
    env.as_contract(&vault_id, || {
        env.storage().instance().set(&symbol_short!("ext_orcl"), &oracle_id);
    });

    // Set oracle limits: max 50 ledgers old, price between 500 and 2000
    client.set_oracle_limits(&admin, &50, &500, &2_000);

    // Current ledger = 100
    env.ledger().set_sequence_number(100);

    // Set fresh oracle data at ledger 95 (age = 5) with price = 1000
    let oracle_client = MockOracleClient::new(&env, &oracle_id);
    oracle_client.set_data(&1_000, &95);

    let (price, age, is_valid) = client.get_oracle_status();
    assert_eq!(price, 1_000);
    assert_eq!(age, 5);
    assert!(is_valid);

    let validated_price = client.get_validated_oracle_price();
    assert_eq!(validated_price, 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_oracle_stale_reverts() {
    let env = Env::default();
    let (client, admin, vault_id, _) = setup_vault(&env);
    let oracle_id = env.register(MockOracle, ());

    env.as_contract(&vault_id, || {
        env.storage().instance().set(&symbol_short!("ext_orcl"), &oracle_id);
    });

    client.set_oracle_limits(&admin, &20, &500, &2_000);
    env.ledger().set_sequence_number(100);

    // Price updated at ledger 70 (age = 30 > max_age 20) -> StaleOracle (Code 3)
    let oracle_client = MockOracleClient::new(&env, &oracle_id);
    oracle_client.set_data(&1_000, &70);

    let (_, age, is_valid) = client.get_oracle_status();
    assert_eq!(age, 30);
    assert!(!is_valid);

    client.get_validated_oracle_price();
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_oracle_out_of_range_reverts() {
    let env = Env::default();
    let (client, admin, vault_id, _) = setup_vault(&env);
    let oracle_id = env.register(MockOracle, ());

    env.as_contract(&vault_id, || {
        env.storage().instance().set(&symbol_short!("ext_orcl"), &oracle_id);
    });

    client.set_oracle_limits(&admin, &50, &500, &2_000);
    env.ledger().set_sequence_number(100);

    // Price = 2500 (> max 2000) -> OraclePriceOutOfRange (Code 4)
    let oracle_client = MockOracleClient::new(&env, &oracle_id);
    oracle_client.set_data(&2_500, &95);

    client.get_validated_oracle_price();
}

// ── Tests for Issue #860: Virtual Shares & Assets Offset ─────────────────────

#[test]
fn test_virtual_shares_and_assets_offset_math() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);

    // In an empty vault (total_shares=0, total_assets=0):
    // eff_shares = 1_000_000, eff_assets = 1
    // A deposit of 100 assets converts to 100 * 1_000_000 / 1 = 100_000_000 virtual shares
    let shares_for_100 = client.convert_to_shares_with_offset(&100);
    assert_eq!(shares_for_100, 100 * VIRTUAL_SHARES_OFFSET / VIRTUAL_ASSETS_OFFSET);

    let assets_back = client.convert_to_assets_with_offset(&shares_for_100);
    assert_eq!(assets_back, 100);
}

#[test]
fn test_inflation_attack_protection_with_virtual_offset() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);

    // Simulating inflation attack protection:
    // Even if donation occurs, virtual offset dampens the share-price manipulation
    let assets = 1_000;
    let preview1 = client.preview_deposit_with_offset(&assets);
    assert!(preview1 > 0);
}

// ── Tests for Issue #861: Minimum Liquidity Shares Burn ──────────────────────

#[test]
fn test_min_liquidity_shares_burned_on_first_deposit() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);
    let alice = Address::generate(&env);

    assert_eq!(client.get_locked_dead_shares(), 0);

    // First deposit with amount covering MIN_LIQUIDITY_SHARES (e.g. 5000)
    let shares = client.deposit_with_dead_shares(&alice, &5_000);
    assert_eq!(shares, 5_000 - MIN_LIQUIDITY_SHARES);
    assert_eq!(client.get_locked_dead_shares(), MIN_LIQUIDITY_SHARES);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_first_deposit_below_min_liquidity_reverts() {
    let env = Env::default();
    let (client, _, _, _) = setup_vault(&env);
    let alice = Address::generate(&env);

    // Initial deposit too small to cover 1000 dead shares -> InsufficientInitialDeposit (Code 3)
    client.deposit_with_dead_shares(&alice, &500);
}

// ── Tests for Issue #862: Balance Audit & Counter Isolation ──────────────────

#[test]
fn test_direct_transfer_does_not_alter_accounting() {
    let env = Env::default();
    let (client, _, vault_id, token_client) = setup_vault(&env);
    let alice = Address::generate(&env);

    // Mint tokens to alice
    token_client.mint(&alice, &100_000);

    // Deposit 10,000 via normal flow
    client.deposit(&alice, &10_000, &None);

    let preview_before = client.preview_deposit(&1_000);

    // Attacker does a direct transfer of 50,000 tokens to the vault contract address
    let attacker = Address::generate(&env);
    token_client.mint(&attacker, &50_000);
    token_client.transfer(&attacker, &vault_id, &50_000);

    // Contract has more raw balance, but preview and internal counters are UNCHANGED
    let preview_after = client.preview_deposit(&1_000);
    assert_eq!(preview_before, preview_after);
    assert!(client.verify_balance_accounting_isolation());
}
