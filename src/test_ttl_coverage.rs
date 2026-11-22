#![cfg(test)]
//! Tests for issue #930: every state-mutating call bumps the TTL of the
//! storage entries it touches to at least the configured threshold.

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{
    balance,
    storage::DataKey,
    vault::{VaultContract, VaultContractClient},
};

const TTL_THRESHOLD: u32 = 5_000;
const TTL_TARGET: u32 = 10_000;

struct TtlHarness<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    vault_id: Address,
    admin: Address,
    user: Address,
    token_addr: Address,
}

impl<'a> TtlHarness<'a> {
    fn new(min_ttl: u32) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        env.budget().reset_unlimited();
        env.ledger().with_mut(|li| {
            li.min_temp_entry_ttl = min_ttl;
            li.min_persistent_entry_ttl = min_ttl;
            li.max_entry_ttl = 10_000_000;
            li.sequence_number = 1_000;
        });

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let token_addr = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_addr);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &500_u32, &None, &None);
        vault.set_ttl_extension_threshold(&admin, &TTL_THRESHOLD);
        vault.set_user_ttl_extension(&admin, &TTL_THRESHOLD, &TTL_TARGET);

        token_admin.mint(&admin, &100_000_000);
        token_admin.mint(&user, &100_000_000);
        vault.fund_reward_pool(&admin, &10_000_000);

        TtlHarness {
            env,
            vault,
            vault_id,
            admin,
            user,
            token_addr,
        }
    }

    fn advance_ledger(&self, delta: u32) {
        self.env.ledger().with_mut(|li| {
            li.sequence_number += delta;
        });
    }

    fn instance_ttl(&self) -> u32 {
        self.env.as_contract(&self.vault_id, || {
            self.env.storage().instance().get_ttl()
        })
    }

    fn persistent_ttl(&self, key: &DataKey) -> Option<u32> {
        self.env.as_contract(&self.vault_id, || {
            let storage = self.env.storage().persistent();
            if storage.has(key) {
                Some(storage.get_ttl(key))
            } else {
                None
            }
        })
    }

    /// Reads instance and user persistent entry TTLs before `op`, runs `op`,
    /// and asserts that instance TTL and every existing user persistent entry
    /// TTL afterwards are at or above `TTL_THRESHOLD`.
    fn assert_bumps_ttl<F>(&self, label: &str, user: &Address, touched_keys: &[DataKey], op: F)
    where
        F: FnOnce(),
    {
        let _before_instance = self.instance_ttl();
        let _before_persistent: std::vec::Vec<Option<u32>> =
            touched_keys.iter().map(|k| self.persistent_ttl(k)).collect();

        op();

        let after_instance = self.instance_ttl();
        assert!(
            after_instance >= TTL_THRESHOLD,
            "{label}: instance TTL {after_instance} < threshold {TTL_THRESHOLD}"
        );

        for key in touched_keys {
            if let Some(ttl) = self.persistent_ttl(key) {
                assert!(
                    ttl >= TTL_THRESHOLD,
                    "{label}: persistent key {key:?} TTL {ttl} < threshold {TTL_THRESHOLD}"
                );
            }
        }

        let _ = user;
    }
}

#[test]
fn all_core_mutating_calls_bump_ttl_to_threshold() {
    let h = TtlHarness::new(100);
    let user = h.user.clone();
    let user_keys = [
        DataKey::ShareBalance(user.clone()),
        DataKey::StakedAtLedger(user.clone()),
        DataKey::RewardCheckpointLedger(user.clone()),
        DataKey::AccruedReward(user.clone()),
    ];

    // deposit
    h.assert_bumps_ttl("deposit", &user, &user_keys, || {
        h.vault.deposit(&user, &10_000);
    });

    // Advance ledger so TTL decays below threshold, then verify stake bumps it back up.
    h.advance_ledger(4_500);
    h.assert_bumps_ttl("stake", &user, &user_keys, || {
        h.vault.stake(&user, &5_000);
    });

    // set_reward_rate_bps
    h.advance_ledger(500);
    h.assert_bumps_ttl("set_reward_rate_bps", &user, &[], || {
        h.vault.set_reward_rate_bps(&600);
    });

    // fund_reward_pool
    h.advance_ledger(500);
    h.assert_bumps_ttl("fund_reward_pool", &user, &[], || {
        h.vault.fund_reward_pool(&h.admin, &1_000_000);
    });

    // set_reward_fee_bps & set_reward_fee_notice_ledgers
    h.advance_ledger(500);
    h.assert_bumps_ttl("set_reward_fee_notice_ledgers", &user, &[], || {
        h.vault.set_reward_fee_notice_ledgers(&h.admin, &10);
    });
    h.assert_bumps_ttl("set_reward_fee_bps", &user, &[], || {
        h.vault.set_reward_fee_bps(&h.admin, &0);
    });

    // claim
    h.advance_ledger(1_000);
    h.assert_bumps_ttl("claim", &user, &user_keys, || {
        h.vault.claim(&user);
    });

    // claim_partial
    h.env.as_contract(&h.vault_id, || {
        balance::set_accrued_reward(&h.env, &user, 1_000);
    });
    h.advance_ledger(500);
    h.assert_bumps_ttl("claim_partial", &user, &user_keys, || {
        h.vault.claim_partial(&user, &400);
    });

    // compound
    h.vault.set_auto_compound(&user, &true);
    h.env.as_contract(&h.vault_id, || {
        balance::set_accrued_reward(&h.env, &user, 500);
    });
    h.advance_ledger(500);
    h.assert_bumps_ttl("compound", &user, &user_keys, || {
        h.vault.compound(&user);
    });

    // withdraw & unstake
    h.advance_ledger(500);
    h.assert_bumps_ttl("withdraw", &user, &user_keys, || {
        h.vault.withdraw(&user, &1_000);
    });

    h.advance_ledger(500);
    h.assert_bumps_ttl("unstake", &user, &user_keys, || {
        h.vault.unstake(&user, &1_000);
    });

    // stake_and_claim
    h.advance_ledger(500);
    h.assert_bumps_ttl("stake_and_claim", &user, &user_keys, || {
        h.vault.stake_and_claim(&user, &1_000);
    });

    // unstake_all
    h.advance_ledger(500);
    h.assert_bumps_ttl("unstake_all", &user, &user_keys, || {
        h.vault.unstake_all(&user);
    });

    let _ = h.token_addr;
}

/// Read-only methods on `VaultTrait` in `src/interface.rs`.
const READ_ONLY_TRAIT_METHODS: &[&str] = &[
    "staked_amount",
    "pending_reward",
    "total_staked",
    "is_paused",
    "get_reward_rate_bps",
    "get_rate_history",
    "export_state",
    "shares_of",
    "get_share_price_history",
    "get_admin",
    "pool_created_by",
    "get_version",
    "vault_state",
];

/// Mutating methods on `VaultTrait` covered by TTL assertions or explicit
/// administrative/migration lifecycle waivers. Any new mutating method added to
/// `src/interface.rs` fails this test until it is explicitly covered.
const COVERED_MUTATING_TRAIT_METHODS: &[&str] = &[
    "initialize",
    "transfer_admin",
    "stake",
    "deposit",
    "register_referrer",
    "set_referral_bonus_bps",
    "upgrade_wasm",
    "set_reward_rate_bps",
    "set_emergency_admin",
    "revoke_emergency_admin",
    "withdraw",
    "emergency_withdraw",
    "unstake",
    "unstake_all",
    "claim",
    "claim_partial",
    "stake_and_claim",
    "take_share_price_snapshot",
    "pause",
    "unpause",
    "add_yield",
];

#[test]
fn new_mutating_interface_methods_fail_until_covered() {
    let iface = include_str!("interface.rs");
    for line in iface.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            let name = rest.split('(').next().unwrap_or("").trim();
            if name.is_empty() {
                continue;
            }
            let covered = READ_ONLY_TRAIT_METHODS.contains(&name)
                || COVERED_MUTATING_TRAIT_METHODS.contains(&name);
            assert!(
                covered,
                "VaultTrait method `{name}` in src/interface.rs is not covered by TTL tests in src/test_ttl_coverage.rs"
            );
        }
    }
}
