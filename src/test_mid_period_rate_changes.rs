#![cfg(test)]
//! Tests for issue #929: reward accrual across mid-period rate changes.

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{
    reward_math::reward_for_ledgers,
    vault::{VaultContract, VaultContractClient, BOOST_BPS_BASE, STELLAR_LEDGERS_PER_YEAR},
};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    alice: Address,
}

impl<'a> Fixture<'a> {
    fn new(initial_rate_bps: u32) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();
        env.ledger().with_mut(|li| {
            li.sequence_number = 1_000;
            li.min_temp_entry_ttl = 10_000_000;
            li.min_persistent_entry_ttl = 10_000_000;
            li.max_entry_ttl = 10_000_000;
        });

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);
        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &initial_rate_bps, &None, &None);

        token_admin.mint(&admin, &100_000_000_000);
        token_admin.mint(&alice, &100_000_000_000);
        vault.fund_reward_pool(&admin, &50_000_000_000);

        Fixture {
            env,
            vault,
            admin,
            alice,
        }
    }

    fn set_ledger(&self, seq: u32) {
        self.env.ledger().with_mut(|li| li.sequence_number = seq);
    }
}

/// Independent reference model for a single-rate segment without multipliers.
fn ref_segment(amount: i128, rate_bps: u32, elapsed: u32) -> i128 {
    if amount == 0 || rate_bps == 0 || elapsed == 0 {
        return 0;
    }
    amount * (rate_bps as i128) * (elapsed as i128)
        / (BOOST_BPS_BASE as i128)
        / (STELLAR_LEDGERS_PER_YEAR as i128)
}

#[test]
fn mid_period_rate_increase_settles_old_and_new_rates() {
    let f = Fixture::new(1_000); // 10% APR
    let principal: i128 = 10_000_000_000;
    f.vault.deposit(&f.alice, &principal);

    // 100_000 ledgers at 1_000 bps
    f.set_ledger(101_000);
    f.vault.set_reward_rate_bps(&2_000); // 20% APR

    // 100_000 ledgers at 2_000 bps
    f.set_ledger(201_000);
    let claimed = f.vault.claim(&f.alice);

    let expected_seg1 = ref_segment(principal, 1_000, 100_000);
    let expected_seg2 = ref_segment(principal, 2_000, 100_000);
    let ref_model = reward_for_ledgers(
        principal,
        1_000,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        100_000,
    )
    .unwrap()
        + reward_for_ledgers(
            principal,
            2_000,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            100_000,
        )
        .unwrap();

    // Allow at most 1 stroop difference from sub-unit remainder carry across segments.
    assert!(
        (claimed - (expected_seg1 + expected_seg2)).abs() <= 1,
        "claimed={claimed}, expected={}",
        expected_seg1 + expected_seg2
    );
    assert!((claimed - ref_model).abs() <= 1);
}

#[test]
fn mid_period_rate_decrease_settles_old_and_new_rates() {
    let f = Fixture::new(2_000); // 20% APR
    let principal: i128 = 10_000_000_000;
    f.vault.deposit(&f.alice, &principal);

    // 100_000 ledgers at 2_000 bps
    f.set_ledger(101_000);
    f.vault.set_reward_rate_bps(&500); // 5% APR

    // 100_000 ledgers at 500 bps
    f.set_ledger(201_000);
    let claimed = f.vault.claim(&f.alice);

    let expected = ref_segment(principal, 2_000, 100_000) + ref_segment(principal, 500, 100_000);
    let ref_model = reward_for_ledgers(
        principal,
        2_000,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        100_000,
    )
    .unwrap()
        + reward_for_ledgers(
            principal,
            500,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            100_000,
        )
        .unwrap();

    assert!((claimed - expected).abs() <= 1);
    assert!((claimed - ref_model).abs() <= 1);
}

#[test]
fn multiple_rate_changes_between_user_interactions() {
    let f = Fixture::new(500);
    let principal: i128 = 10_000_000_000;
    f.vault.deposit(&f.alice, &principal);

    // Segment 1: [1_000, 51_000) at 500 bps (50_000 ledgers)
    f.set_ledger(51_000);
    f.vault.set_reward_rate_bps(&1_500);

    // Segment 2: [51_000, 151_000) at 1_500 bps (100_000 ledgers)
    f.set_ledger(151_000);
    f.vault.set_reward_rate_bps(&800);

    // Segment 3: [151_000, 201_000) at 800 bps (50_000 ledgers)
    f.set_ledger(201_000);
    f.vault.set_reward_rate_bps(&2_500);

    // Segment 4: [201_000, 301_000) at 2_500 bps (100_000 ledgers)
    f.set_ledger(301_000);
    let claimed = f.vault.claim(&f.alice);

    let expected = ref_segment(principal, 500, 50_000)
        + ref_segment(principal, 1_500, 100_000)
        + ref_segment(principal, 800, 50_000)
        + ref_segment(principal, 2_500, 100_000);

    let ref_model = reward_for_ledgers(
        principal,
        500,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        BOOST_BPS_BASE,
        50_000,
    )
    .unwrap()
        + reward_for_ledgers(
            principal,
            1_500,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            100_000,
        )
        .unwrap()
        + reward_for_ledgers(
            principal,
            800,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            50_000,
        )
        .unwrap()
        + reward_for_ledgers(
            principal,
            2_500,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            BOOST_BPS_BASE,
            100_000,
        )
        .unwrap();

    assert!((claimed - expected).abs() <= 3);
    assert!((claimed - ref_model).abs() <= 3);
    let _ = f.admin;
}

#[test]
fn rate_change_in_same_ledger_as_claim_uses_old_rate_for_prior_ledgers() {
    let f = Fixture::new(1_000);
    let principal: i128 = 10_000_000_000;
    f.vault.deposit(&f.alice, &principal);

    // Advance 100_000 ledgers, then change rate AND claim in the same ledger.
    f.set_ledger(101_000);
    f.vault.set_reward_rate_bps(&5_000);
    let claimed = f.vault.claim(&f.alice);

    // All 100_000 ledgers elapsed before ledger 101_000, so they must settle at 1_000 bps.
    let expected = ref_segment(principal, 1_000, 100_000);
    assert_eq!(claimed, expected);
}
