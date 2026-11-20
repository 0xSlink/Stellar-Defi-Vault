#![no_std]

#[cfg(feature = "mainnet")]
compile_error!("test-faucet must never be built for mainnet!");

use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, Env, Symbol};

const TOKEN_KEY: Symbol = symbol_short!("f_tok");
const AMOUNT_KEY: Symbol = symbol_short!("f_amt");
const COOLDOWN_KEY: Symbol = symbol_short!("f_cool");
const LAST_CLAIM_PREFIX: Symbol = symbol_short!("f_last");

const DEFAULT_CLAIM_AMOUNT: i128 = 100_000_0000000; // 100 tokens
const DEFAULT_COOLDOWN_LEDGERS: u32 = 17_280; // ~1 day

#[contract]
pub struct TestFaucetContract;

#[contractimpl]
impl TestFaucetContract {
    /// Initialize faucet with token address, claim amount, and cooldown period.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        claim_amount: i128,
        cooldown_ledgers: u32,
    ) {
        admin.require_auth();
        env.storage().instance().set(&TOKEN_KEY, &token);
        let amt = if claim_amount > 0 {
            claim_amount
        } else {
            DEFAULT_CLAIM_AMOUNT
        };
        let cool = if cooldown_ledgers > 0 {
            cooldown_ledgers
        } else {
            DEFAULT_COOLDOWN_LEDGERS
        };
        env.storage().instance().set(&AMOUNT_KEY, &amt);
        env.storage().instance().set(&COOLDOWN_KEY, &cool);
    }

    /// Claim tokens from faucet, respecting per-address cooldown.
    pub fn claim(env: Env, claimant: Address) -> i128 {
        claimant.require_auth();
        let current_ledger = env.ledger().sequence();
        let last_claim: u32 = env
            .storage()
            .persistent()
            .get(&(LAST_CLAIM_PREFIX, claimant.clone()))
            .unwrap_or(0);
        let cooldown: u32 = env
            .storage()
            .instance()
            .get(&COOLDOWN_KEY)
            .unwrap_or(DEFAULT_COOLDOWN_LEDGERS);

        if last_claim > 0 && current_ledger < last_claim + cooldown {
            panic!("Faucet: cooldown active");
        }

        let token_addr: Address = env
            .storage()
            .instance()
            .get(&TOKEN_KEY)
            .expect("Faucet: not initialized");
        let amount: i128 = env
            .storage()
            .instance()
            .get(&AMOUNT_KEY)
            .unwrap_or(DEFAULT_CLAIM_AMOUNT);

        env.storage()
            .persistent()
            .set(&(LAST_CLAIM_PREFIX, claimant.clone()), &current_ledger);

        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &claimant,
            &amount,
        );
        amount
    }

    /// Query remaining cooldown ledgers for a given claimant.
    pub fn get_cooldown(env: Env, claimant: Address) -> u32 {
        let current_ledger = env.ledger().sequence();
        let last_claim: u32 = env
            .storage()
            .persistent()
            .get(&(LAST_CLAIM_PREFIX, claimant))
            .unwrap_or(0);
        let cooldown: u32 = env
            .storage()
            .instance()
            .get(&COOLDOWN_KEY)
            .unwrap_or(DEFAULT_COOLDOWN_LEDGERS);

        if last_claim == 0 || current_ledger >= last_claim + cooldown {
            0
        } else {
            (last_claim + cooldown) - current_ledger
        }
    }
}
