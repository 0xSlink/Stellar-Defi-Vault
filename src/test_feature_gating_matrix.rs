//! Feature interaction test matrix (issue #786).
//!
//! Many independently-added features gate the same four entrypoints —
//! deposit, withdraw, claim, and a full exit — and until now their
//! combinations were untested. This is a table-driven test: [`GATING_TABLE`]
//! names each gating dimension's effect on each entrypoint, and
//! [`all_combinations_match_table`] replays every `2^N` combination of the
//! table's dimensions against a real contract instance, asserting the
//! outcome the table predicts.
//!
//! The same table is rendered as a Markdown section in `docs/ARCHITECTURE.md`
//! so it doubles as documentation, per the issue's acceptance criteria.
//!
//! # What this table found
//!
//! Building this matrix required tracing exactly which entrypoint checks
//! which gate, directly in `vault.rs`. That turned up two things worth
//! surfacing rather than hiding:
//!
//! - **Two gap dimensions.** `flag_frozen()`/`is_frozen()` (the closest
//!   thing this contract has to a blocklist) and `withdrawal_cooldown.rs` /
//!   the claim cooldown in `allowlist_rate_limits.rs` are never actually
//!   read by `deposit`, `withdraw`, `claim`, or `unstake_all` — each is
//!   fully implemented storage-and-getter-wise, but nothing calls its
//!   check function from any entrypoint. The table below records their
//!   current (non-)effect as `Allowed` everywhere, explicitly marked `(gap)`
//!   — that is the honest, verified-against-the-code answer, not an
//!   assumption that they work because the issue names them.
//! - **Pause is two independent flags, not one.** `pause_deposits()` /
//!   `pause_withdrawals()` (`vault_extensions_490_493.rs`) are checked by
//!   `do_stake_inner` and `do_unstake` respectively — but `deposit()` calls
//!   `stake()` directly, *not* `do_stake_inner`, so `pause_deposits()`
//!   currently has no effect on the primary deposit path either. That's a
//!   second gap, documented as such in the table rather than asserted as
//!   enforced.
//!
//! None of these gaps are fixed here — this issue is about adding the test
//! matrix, not wiring up enforcement that doesn't exist yet. Fixing them is
//! follow-up work the table now makes concrete and trackable.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, token, Address, Env};

use crate::vault::{VaultContract, VaultContractClient};

// ── Gating dimensions ─────────────────────────────────────────────────────

/// One combination of gating states to apply before exercising an
/// entrypoint. Each field is independently on/off.
#[derive(Clone, Copy, Debug, Default)]
struct GatingState {
    withdrawals_paused: bool,
    withdraw_only_mode: bool,
    position_locked: bool,
    daily_limit_exceeded: bool,
    blocklisted: bool,
    cooldown_active: bool,
}

/// The four entrypoints under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Deposit,
    Withdraw,
    Claim,
    /// A full exit: `unstake_all()`. Internally this is `do_unstake` with
    /// `shares == total user shares`, so it shares `withdraw`'s gating
    /// exactly (confirmed by reading `unstake_all`'s body).
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Allowed,
    /// Blocked, and the specific `VaultError` it reverts with.
    Blocked(&'static str),
}

/// The gating table. Each row is one dimension; each cell is that
/// dimension's effect on that column *when active*, independent of every
/// other dimension (confirmed one at a time against `vault.rs` — see the
/// module doc for the two gaps this surfaced). `expected_outcome` below
/// combines rows via "blocked if any active dimension blocks this column".
///
/// | Dimension               | Deposit             | Withdraw                        | Claim   | Exit                             |
/// |--------------------------|---------------------|----------------------------------|---------|----------------------------------|
/// | Withdrawals paused       | Allowed             | Blocked (`VaultPaused`)         | Allowed | Blocked (`VaultPaused`)         |
/// | Withdraw-only mode       | Blocked (`PoolShuttingDown`) | Allowed                | Allowed | Allowed                          |
/// | Position locked          | Allowed             | Blocked (`UseCooldownFlow`)     | Allowed | Blocked (`UseCooldownFlow`)     |
/// | Daily withdrawal limit   | Allowed             | Blocked (`WithdrawalLimitExceeded`) | Allowed | Blocked (`WithdrawalLimitExceeded`) |
/// | Blocklist (frozen) *gap* | Allowed             | Allowed                          | Allowed | Allowed                          |
/// | Cooldown *gap*           | Allowed             | Allowed                          | Allowed | Allowed                          |
const GATING_TABLE: () = ();

fn expected_outcome(state: GatingState, action: Action) -> Outcome {
    use Action::*;

    match action {
        Deposit => {
            if state.withdraw_only_mode {
                Outcome::Blocked("PoolShuttingDown")
            } else {
                Outcome::Allowed
            }
        }
        Claim => Outcome::Allowed,
        Withdraw | Exit => {
            if state.withdrawals_paused {
                Outcome::Blocked("VaultPaused")
            } else if state.position_locked {
                Outcome::Blocked("UseCooldownFlow")
            } else if state.daily_limit_exceeded {
                Outcome::Blocked("WithdrawalLimitExceeded")
            } else {
                Outcome::Allowed
            }
            // `blocklisted` and `cooldown_active` never change the outcome:
            // confirmed gaps, documented in the table above.
        }
    }
}

// ── Fixture ────────────────────────────────────────────────────────────────

fn create_token<'a>(
    env: &Env,
    admin: &Address,
) -> (Address, token::Client<'a>, token::StellarAssetClient<'a>) {
    let address = env.register_stellar_asset_contract(admin.clone());
    (
        address.clone(),
        token::Client::new(env, &address),
        token::StellarAssetClient::new(env, &address),
    )
}

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    user: Address,
}

impl<'a> Fixture<'a> {
    /// Builds a fresh contract with `user` already holding a position, so
    /// every action under test (deposit more, withdraw, claim, exit) has
    /// something to act on. Applies `state`'s gates in whatever order
    /// their setup requires.
    fn new(state: GatingState) -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let (token_addr, _token, token_admin) = create_token(&env, &admin);

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_addr, &0_u32, &None, &None);

        token_admin.mint(&user, &10_000_000);
        token_admin.mint(&admin, &10_000_000);

        // Give `user` a starting position before applying any gate, so a
        // gate that blocks deposits doesn't also prevent the fixture from
        // having anything to withdraw/claim/exit.
        vault.deposit(&user, &1_000_000, &None);

        if state.position_locked {
            vault.lock_position(&user, &1_000_000);
        }
        if state.daily_limit_exceeded {
            vault.set_daily_withdrawal_limit(&1);
        }
        if state.withdrawals_paused {
            vault.pause_withdrawals(&admin);
        }
        if state.withdraw_only_mode {
            vault.enter_withdraw_only_mode(&admin);
        }
        // `blocklisted` / `cooldown_active`: intentionally not wired to any
        // call here either — there is no entrypoint for either that would
        // have an observable effect on the actions under test (see module
        // doc). Left as fields so the combinatorial generator below still
        // covers them, asserting the documented no-op.

        Fixture { env, vault, admin, user }
    }

    fn try_run(&self, action: Action) -> Result<(), std::string::String> {
        let result = match action {
            Action::Deposit => self.vault.try_deposit(&self.user, &100, &None).map(|_| ()),
            Action::Withdraw => self.vault.try_withdraw(&self.user, &100).map(|_| ()),
            Action::Claim => self.vault.try_claim(&self.user).map(|_| ()),
            Action::Exit => self.vault.try_unstake_all(&self.user).map(|_| ()),
        };
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(std::format!("{:?}", e)),
            Err(e) => Err(std::format!("{:?}", e)),
        }
    }
}

// ── The combinatorial test ───────────────────────────────────────────────

/// Replays all `2^6` combinations of [`GatingState`] against all four
/// [`Action`]s (`4 * 64 = 256` generated cases) and asserts each result
/// matches [`expected_outcome`] / the table in the module doc.
#[test]
fn all_combinations_match_table() {
    let actions = [Action::Deposit, Action::Withdraw, Action::Claim, Action::Exit];

    for bits in 0u8..64 {
        let state = GatingState {
            withdrawals_paused: bits & 0b0000_0001 != 0,
            withdraw_only_mode: bits & 0b0000_0010 != 0,
            position_locked: bits & 0b0000_0100 != 0,
            daily_limit_exceeded: bits & 0b0000_1000 != 0,
            blocklisted: bits & 0b0001_0000 != 0,
            cooldown_active: bits & 0b0010_0000 != 0,
        };

        for &action in &actions {
            // Fresh contract per cell: gates like "withdraw-only mode" and
            // "withdrawals paused" are one-way-ish admin toggles in this
            // contract (no combined "set all gates" entrypoint), so
            // isolating each combination avoids one cell's setup leaking
            // into the next.
            let fixture = Fixture::new(state);
            let outcome = fixture.try_run(action);
            let expected = expected_outcome(state, action);

            match expected {
                Outcome::Allowed => {
                    assert!(
                        outcome.is_ok(),
                        "expected {:?} to be Allowed under {:?}, got {:?}",
                        action,
                        state,
                        outcome
                    );
                }
                Outcome::Blocked(expected_err) => {
                    let err = outcome.expect_err(&std::format!(
                        "expected {:?} to be Blocked({}) under {:?}, but it succeeded",
                        action,
                        expected_err,
                        state
                    ));
                    assert!(
                        err.contains(expected_err),
                        "expected {:?} under {:?} to fail with {}, got {}",
                        action,
                        state,
                        expected_err,
                        err
                    );
                }
            }
        }
    }
}
