#![cfg(test)]
//! Tests for generic proposal execution (#890) and ledger time-travel helpers (#892).

extern crate std;

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    token, Address, Bytes, Env, Symbol, Vec, Val, IntoVal,
};

use crate::{
    time_travel::{advance_days, advance_ledgers, set_ledger, LEDGERS_PER_DAY, SECONDS_PER_LEDGER},
    vault_extensions_890::{GenericProposalState},
    vault::{VaultContract, VaultContractClient},
};

#[contract]
struct MockTargetContract;

#[contractimpl]
impl MockTargetContract {
    pub fn execute_action(_env: Env, _val: i128) -> i128 {
        42
    }

    pub fn failing_action(_env: Env) {
        panic!("Intended target failure");
    }
}

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

// ── Generic Proposal Execution Tests (Issue #890) ──────────────────────────

#[test]
fn test_allow_call_target_management() {
    let env = Env::default();
    let (client, admin, _) = setup_vault(&env);
    let target = Address::generate(&env);
    let func = Symbol::new(&env, "execute_action");

    assert!(!client.is_call_target_allowed(&target, &func));

    client.allow_call_target(&admin, &target, &func);
    assert!(client.is_call_target_allowed(&target, &func));

    client.disallow_call_target(&admin, &target, &func);
    assert!(!client.is_call_target_allowed(&target, &func));
}

#[test]
#[should_panic(expected = "Target not whitelisted")]
fn test_create_generic_proposal_rejects_unwhitelisted_target() {
    let env = Env::default();
    let (client, _admin, _) = setup_vault(&env);
    let proposer = Address::generate(&env);
    let target = Address::generate(&env);
    let func = Symbol::new(&env, "unauthorized_fn");
    let payload = Bytes::from_slice(&env, b"test_args");

    client.create_generic_proposal(&proposer, &target, &func, &payload);
}

#[test]
fn test_create_and_execute_generic_proposal_success() {
    let env = Env::default();
    let (client, admin, _) = setup_vault(&env);
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);

    let target = env.register(MockTargetContract, ());
    let func = Symbol::new(&env, "execute_action");
    client.allow_call_target(&admin, &target, &func);

    let payload = Bytes::from_slice(&env, b"valid_args");
    let prop_id = client.create_generic_proposal(&proposer, &target, &func, &payload);
    assert_eq!(prop_id, 1);

    let mut call_args = Vec::new(&env);
    call_args.push_back(100i128.into_val(&env));

    let success = client.execute_generic_proposal(&executor, &prop_id, &payload, &call_args);
    assert!(success);

    let prop = client.get_generic_proposal(&prop_id).unwrap();
    assert_eq!(prop.state, GenericProposalState::Executed);
}

#[test]
#[should_panic(expected = "Args hash mismatch")]
fn test_execute_generic_proposal_rejects_tampered_args() {
    let env = Env::default();
    let (client, admin, _) = setup_vault(&env);
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);

    let target = env.register(MockTargetContract, ());
    let func = Symbol::new(&env, "execute_action");
    client.allow_call_target(&admin, &target, &func);

    let payload = Bytes::from_slice(&env, b"original_args");
    let prop_id = client.create_generic_proposal(&proposer, &target, &func, &payload);

    let tampered_payload = Bytes::from_slice(&env, b"tampered_args");
    let mut call_args = Vec::new(&env);
    call_args.push_back(100i128.into_val(&env));

    client.execute_generic_proposal(&executor, &prop_id, &tampered_payload, &call_args);
}

#[test]
fn test_execute_generic_proposal_failure_marks_failed_without_revert() {
    let env = Env::default();
    let (client, admin, _) = setup_vault(&env);
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);

    let target = env.register(MockTargetContract, ());
    let func = Symbol::new(&env, "failing_action");
    client.allow_call_target(&admin, &target, &func);

    let payload = Bytes::from_slice(&env, b"fail_payload");
    let prop_id = client.create_generic_proposal(&proposer, &target, &func, &payload);

    let call_args: Vec<Val> = Vec::new(&env);
    let success = client.execute_generic_proposal(&executor, &prop_id, &payload, &call_args);
    assert!(!success);

    let prop = client.get_generic_proposal(&prop_id).unwrap();
    assert_eq!(prop.state, GenericProposalState::Failed);
}

// ── Ledger Time-Travel Helper Tests (Issue #892) ───────────────────────────

#[test]
fn test_time_travel_set_ledger() {
    let env = Env::default();
    set_ledger(&env, 100);
    assert_eq!(env.ledger().sequence(), 100);

    set_ledger(&env, 500);
    assert_eq!(env.ledger().sequence(), 500);
    assert_eq!(env.ledger().timestamp(), 400 * SECONDS_PER_LEDGER);
}

#[test]
fn test_time_travel_advance_ledgers() {
    let env = Env::default();
    set_ledger(&env, 100);
    advance_ledgers(&env, 50);
    assert_eq!(env.ledger().sequence(), 150);
    assert_eq!(env.ledger().timestamp(), 50 * SECONDS_PER_LEDGER);
}

#[test]
fn test_time_travel_advance_days() {
    let env = Env::default();
    set_ledger(&env, 100);
    advance_days(&env, 2);
    assert_eq!(env.ledger().sequence(), 100 + 2 * LEDGERS_PER_DAY);
}

// Ten test migration scenarios exercising time-travel helpers
#[test]
fn test_scenario_1_stake_cooldown() {
    let env = Env::default();
    set_ledger(&env, 1_000);
    advance_ledgers(&env, 100);
    assert_eq!(env.ledger().sequence(), 1_100);
}

#[test]
fn test_scenario_2_reward_epoch() {
    let env = Env::default();
    set_ledger(&env, 5_000);
    advance_days(&env, 7);
    assert_eq!(env.ledger().sequence(), 5_000 + 7 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_3_vesting_cliff() {
    let env = Env::default();
    set_ledger(&env, 10_000);
    advance_days(&env, 30);
    assert_eq!(env.ledger().sequence(), 10_000 + 30 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_4_timelock_delay() {
    let env = Env::default();
    set_ledger(&env, 20_000);
    advance_ledgers(&env, 500);
    assert_eq!(env.ledger().sequence(), 20_500);
}

#[test]
fn test_scenario_5_ragequit_window() {
    let env = Env::default();
    set_ledger(&env, 50_000);
    advance_days(&env, 3);
    assert_eq!(env.ledger().sequence(), 50_000 + 3 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_6_twab_checkpoint() {
    let env = Env::default();
    set_ledger(&env, 100_000);
    advance_ledgers(&env, 10_000);
    assert_eq!(env.ledger().sequence(), 110_000);
}

#[test]
fn test_scenario_7_unbond_queue() {
    let env = Env::default();
    set_ledger(&env, 200_000);
    advance_days(&env, 14);
    assert_eq!(env.ledger().sequence(), 200_000 + 14 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_8_boost_decay() {
    let env = Env::default();
    set_ledger(&env, 300_000);
    advance_days(&env, 90);
    assert_eq!(env.ledger().sequence(), 300_000 + 90 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_9_governance_voting_window() {
    let env = Env::default();
    set_ledger(&env, 400_000);
    advance_days(&env, 5);
    assert_eq!(env.ledger().sequence(), 400_000 + 5 * LEDGERS_PER_DAY);
}

#[test]
fn test_scenario_10_annual_emission_cycle() {
    let env = Env::default();
    set_ledger(&env, 500_000);
    advance_days(&env, 365);
    assert_eq!(env.ledger().sequence(), 500_000 + 365 * LEDGERS_PER_DAY);
}
