#![cfg(test)]
//! Tests for evidence-based operator slashing (issue #933).

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token, Address, BytesN, Env, Symbol, TryFromVal,
};

use crate::vault::{
    operator_slash::{OperatorSlashError, MAX_SLASH_BPS_PER_INCIDENT},
    VaultContract, VaultContractClient,
};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    token_client: token::Client<'a>,
    token_admin: token::StellarAssetClient<'a>,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| {
            li.sequence_number = 100;
            li.min_temp_entry_ttl = 10_000_000;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
        });
        let admin = Address::generate(&env);
        let token_address = env.register_stellar_asset_contract(admin.clone());
        let token_client = token::Client::new(&env, &token_address);
        let token_admin = token::StellarAssetClient::new(&env, &token_address);
        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_address, &0, &None, &None);
        Fixture {
            env,
            vault,
            admin,
            token_client,
            token_admin,
        }
    }

    fn set_seq(&self, s: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = s);
    }
}

#[test]
fn slash_cap_per_incident_enforced() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let alice = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[1u8; 32]);
    let evidence = BytesN::from_array(&f.env, &[9u8; 32]);

    f.vault.register_operator(&op, &500, &meta).unwrap();
    f.token_admin.mint(&alice, &100_000);
    f.vault.delegate_to_operator(&alice, &op, &100_000).unwrap();

    // Above hard cap (2,000 bps) reverts.
    assert_eq!(
        f.vault.try_slash_operator(
            &f.admin,
            &op,
            &(MAX_SLASH_BPS_PER_INCIDENT + 1),
            &evidence
        ),
        Err(Ok(OperatorSlashError::SlashCapExceeded))
    );

    // Zero bps reverts.
    assert_eq!(
        f.vault.try_slash_operator(&f.admin, &op, &0, &evidence),
        Err(Ok(OperatorSlashError::InvalidSlashBps))
    );

    // Lower admin cap works and is enforced.
    f.vault.set_max_slash_bps(&f.admin, &1_000).unwrap();
    assert_eq!(
        f.vault.try_slash_operator(&f.admin, &op, &1_001, &evidence),
        Err(Ok(OperatorSlashError::SlashCapExceeded))
    );
}

#[test]
fn appeal_delay_blocks_early_execution_and_appeal_can_overturn() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let alice = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[2u8; 32]);
    let evidence = BytesN::from_array(&f.env, &[3u8; 32]);
    let counter = BytesN::from_array(&f.env, &[4u8; 32]);

    f.vault.set_slash_appeal_window(&f.admin, &50).unwrap();
    f.vault.register_operator(&op, &500, &meta).unwrap();
    f.token_admin.mint(&alice, &100_000);
    f.vault.delegate_to_operator(&alice, &op, &100_000).unwrap();

    // Queued at seq 100 -> executable at 150.
    let slash_id = f
        .vault
        .slash_operator(&f.admin, &op, &1_000, &evidence)
        .unwrap();

    // Cannot execute before appeal window closes.
    f.set_seq(149);
    assert_eq!(
        f.vault.try_execute_operator_slash(&slash_id),
        Err(Ok(OperatorSlashError::AppealWindowActive))
    );

    // Operator appeals at seq 149.
    f.vault
        .appeal_operator_slash(&op, &slash_id, &counter)
        .unwrap();

    // Even at seq 150, active appeal blocks execution.
    f.set_seq(150);
    assert_eq!(
        f.vault.try_execute_operator_slash(&slash_id),
        Err(Ok(OperatorSlashError::SlashUnderAppeal))
    );

    // Admin overturns appeal -> slash is cancelled.
    f.vault
        .resolve_operator_slash_appeal(&f.admin, &slash_id, &false)
        .unwrap();
    assert_eq!(
        f.vault.try_execute_operator_slash(&slash_id),
        Err(Ok(OperatorSlashError::AlreadyCancelled))
    );
    assert_eq!(f.vault.get_delegation(&alice, &op).0, 100_000);
}

#[test]
fn slashed_funds_go_to_insurance_first_then_delegators_pro_rata_and_emit_event() {
    let f = Fixture::new();
    let op = Address::generate(&f.env);
    let alice = Address::generate(&f.env);
    let bob = Address::generate(&f.env);
    let meta = BytesN::from_array(&f.env, &[5u8; 32]);
    let evidence = BytesN::from_array(&f.env, &[6u8; 32]);

    f.vault.set_slash_appeal_window(&f.admin, &20).unwrap();
    // Insurance reserve target = 3,000; current reserve = 0.
    f.vault.set_slash_insurance_target(&f.admin, &3_000).unwrap();

    f.vault.register_operator(&op, &500, &meta).unwrap();
    f.token_admin.mint(&alice, &300_000);
    f.token_admin.mint(&bob, &100_000);

    // Alice 300k (75%), Bob 100k (25%); total = 400k.
    f.vault.delegate_to_operator(&alice, &op, &300_000).unwrap();
    f.vault.delegate_to_operator(&bob, &op, &100_000).unwrap();

    // Slash 1,000 bps (10%) = 40,000 total slashed.
    let slash_id = f
        .vault
        .slash_operator(&f.admin, &op, &1_000, &evidence)
        .unwrap();

    f.set_seq(120);
    let (to_ins, to_del) = f.vault.execute_operator_slash(&slash_id).unwrap();

    // 3,000 fills insurance reserve first; 37,000 goes pro rata to delegators.
    assert_eq!(to_ins, 3_000);
    assert_eq!(to_del, 37_000);
    assert_eq!(f.vault.get_insurance_reserve(), 3_000);

    // Delegations reduced by 10%: Alice 270k, Bob 90k.
    assert_eq!(f.vault.get_delegation(&alice, &op).0, 270_000);
    assert_eq!(f.vault.get_delegation(&bob, &op).0, 90_000);

    // Pro-rata refunds: Alice 75% of 37,000 = 27,750; Bob 25% of 37,000 = 9,250.
    assert_eq!(f.vault.claim_delegator_rewards(&alice, &op).unwrap(), 27_750);
    assert_eq!(f.vault.claim_delegator_rewards(&bob, &op).unwrap(), 9_250);
    assert_eq!(f.token_client.balance(&alice), 27_750);
    assert_eq!(f.token_client.balance(&bob), 9_250);

    // Verify operator_slashed event was emitted.
    let events = f.env.events().all();
    let has_event = events.iter().any(|(_, topics, _)| {
        topics
            .get(0)
            .and_then(|t| Symbol::try_from_val(&f.env, &t).ok())
            == Some(Symbol::new(&f.env, "operator_slashed"))
    });
    assert!(has_event);
}
