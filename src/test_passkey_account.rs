#![cfg(test)]
//! Passkey (`secp256r1`) custom smart account tests (issue #947).
//!
//! Confirms that a Soroban custom account verifying `secp256r1` signatures in
//! `__check_auth` can act as both a vault user (`deposit`, `claim`) and the
//! vault `admin` (`set_reward_rate_bps`).

extern crate std;

use soroban_sdk::{
    auth::{Context, CustomAccountInterface},
    contract, contracterror, contractimpl, contracttype,
    crypto::Hash,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    token, vec, Address, BytesN, Env, IntoVal, Val, Vec,
};

use crate::vault::{VaultContract, VaultContractClient};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
enum PasskeyAccountError {
    NotInitialized = 1,
    EmptyAuthContexts = 2,
}

#[contracttype]
enum PasskeyKey {
    PublicKey,
}

#[contract]
struct MockPasskeyAccount;

#[contractimpl]
impl MockPasskeyAccount {
    pub fn init(env: Env, public_key: BytesN<65>) {
        env.storage()
            .instance()
            .set(&PasskeyKey::PublicKey, &public_key);
    }
}

#[contractimpl]
impl CustomAccountInterface for MockPasskeyAccount {
    type Signature = BytesN<64>;
    type Error = PasskeyAccountError;

    fn __check_auth(
        env: Env,
        signature_payload: Hash<32>,
        signature: BytesN<64>,
        auth_contexts: Vec<Context>,
    ) -> Result<(), PasskeyAccountError> {
        if auth_contexts.is_empty() {
            return Err(PasskeyAccountError::EmptyAuthContexts);
        }
        let public_key: BytesN<65> = env
            .storage()
            .instance()
            .get(&PasskeyKey::PublicKey)
            .ok_or(PasskeyAccountError::NotInitialized)?;

        // Host-native CAP-0051 secp256r1 verification; traps on invalid signature.
        env.crypto()
            .secp256r1_verify(&public_key, &signature_payload, &signature);
        Ok(())
    }
}

// RFC 6979 Appendix A.2.5 (P-256 / SHA-256, message = "sample"), low-S normalized.
const RFC6979_PUBKEY_UNCOMPRESSED: [u8; 65] = [
    0x04, // uncompressed SEC1 prefix
    0x60, 0xfe, 0xd4, 0xba, 0x25, 0x5a, 0x9d, 0x31, 0xc9, 0x61, 0xeb, 0x74, 0xc6, 0x35, 0x6d, 0x68,
    0xc0, 0x49, 0xb8, 0x92, 0x3b, 0x61, 0xfa, 0x6c, 0xe6, 0x69, 0x62, 0x2e, 0x60, 0xf2, 0x9f, 0xb6,
    0x79, 0x03, 0xfe, 0x10, 0x08, 0xb8, 0xbc, 0x99, 0xa4, 0x1a, 0xe9, 0xe9, 0x56, 0x28, 0xbc, 0x64,
    0xf2, 0xf1, 0xb2, 0x0c, 0x2d, 0x7e, 0x9f, 0x51, 0x77, 0xa3, 0xc2, 0x94, 0xd4, 0x46, 0x22, 0x99,
];

const RFC6979_SHA256_DIGEST: [u8; 32] = [
    0xaf, 0x2b, 0xdb, 0xe1, 0xaa, 0x9b, 0x6e, 0xc1, 0xe2, 0xad, 0xe1, 0xd6, 0x94, 0xf4, 0x1f, 0xc7,
    0x1a, 0x83, 0x1d, 0x02, 0x68, 0xe9, 0x89, 0x15, 0x62, 0x11, 0x3d, 0x8a, 0x62, 0xad, 0xd1, 0xbf,
];

const RFC6979_SIG_LOW_S: [u8; 64] = [
    // r
    0xef, 0xd4, 0x8b, 0x2a, 0xac, 0xb6, 0xa8, 0xfd, 0x11, 0x40, 0xdd, 0x9c, 0xd4, 0x5e, 0x81, 0xd6,
    0x9d, 0x2c, 0x87, 0x7b, 0x56, 0xaa, 0xf9, 0x91, 0xc3, 0x4d, 0x0e, 0xa8, 0x4e, 0xaf, 0x37, 0x16,
    // low-S normalized s' = n - s
    0x08, 0x34, 0xe3, 0x6a, 0xd2, 0x9a, 0x83, 0xbf, 0x49, 0x1d, 0x60, 0x9a, 0x2b, 0xc9, 0x38, 0x5d,
    0xc8, 0xfd, 0xf9, 0xd1, 0xed, 0x67, 0xaa, 0x7e, 0xa5, 0xf5, 0x1f, 0x93, 0x78, 0x28, 0x57, 0xa9,
];

fn mock_account_auth(
    env: &Env,
    contract_id: &Address,
    account_id: &Address,
    fn_name: &str,
    args: Vec<Val>,
) {
    let invocation = MockAuthInvoke {
        contract: contract_id,
        fn_name,
        args,
        sub_invokes: &[],
    };
    env.mock_auths(&[MockAuth {
        address: account_id,
        invoke: &invocation,
    }]);
}

#[test]
fn passkey_account_verifies_secp256r1_signature_in_check_auth() {
    let env = Env::default();
    let account_id = env.register_contract(None, MockPasskeyAccount);
    let account = MockPasskeyAccountClient::new(&env, &account_id);

    let pk = BytesN::from_array(&env, &RFC6979_PUBKEY_UNCOMPRESSED);
    account.init(&pk);

    let payload = Hash::from_bytes(BytesN::from_array(&env, &RFC6979_SHA256_DIGEST));
    let sig = BytesN::from_array(&env, &RFC6979_SIG_LOW_S);
    let dummy_context = Context::Contract(soroban_sdk::auth::ContractContext {
        contract: account_id.clone(),
        fn_name: soroban_sdk::Symbol::new(&env, "deposit"),
        args: vec![&env],
    });
    let contexts = vec![&env, dummy_context];

    account.__check_auth(&payload, &sig, &contexts);

    // Flipping a bit in the signature must fail verification.
    let mut bad_sig_bytes = RFC6979_SIG_LOW_S;
    bad_sig_bytes[0] ^= 0x01;
    let bad_sig = BytesN::from_array(&env, &bad_sig_bytes);
    assert!(account.try___check_auth(&payload, &bad_sig, &contexts).is_err());
}

#[test]
fn passkey_account_executes_deposit_claim_and_admin_call() {
    let env = Env::default();
    env.ledger().with_mut(|li| {
        li.min_temp_entry_ttl = 10_000_000;
        li.min_persistent_entry_ttl = 10_000_000;
        li.max_entry_ttl = 10_000_000;
    });

    let bootstrap_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract(bootstrap_admin.clone());
    let token_admin = token::StellarAssetClient::new(&env, &token_id);

    let vault_id = env.register_contract(None, VaultContract);
    let vault = VaultContractClient::new(&env, &vault_id);

    let passkey_id = env.register_contract(None, MockPasskeyAccount);
    let passkey_client = MockPasskeyAccountClient::new(&env, &passkey_id);
    passkey_client.init(&BytesN::from_array(&env, &RFC6979_PUBKEY_UNCOMPRESSED));

    env.mock_all_auths();
    vault.initialize(&bootstrap_admin, &token_id, &0_u32, &None, &None);
    token_admin.mint(&passkey_id, &50_000_000);
    token_admin.mint(&bootstrap_admin, &50_000_000);
    vault.fund_reward_pool(&bootstrap_admin, &10_000_000);

    // Transfer admin role to the secp256r1 passkey smart account.
    env.as_contract(&vault_id, || crate::admin::set_admin(&env, &passkey_id));

    // 1. Admin call through the passkey smart account.
    mock_account_auth(
        &env,
        &vault_id,
        &passkey_id,
        "set_reward_rate_bps",
        vec![&env, 500_u32.into_val(&env)],
    );
    vault.set_reward_rate_bps(&500);
    assert_eq!(vault.get_reward_rate_bps(), 500);

    // 2. Deposit executed by the passkey smart account.
    let deposit_amount: i128 = 10_000_000;
    mock_account_auth(
        &env,
        &vault_id,
        &passkey_id,
        "deposit",
        vec![
            &env,
            passkey_id.clone().into_val(&env),
            deposit_amount.into_val(&env),
            Option::<Address>::None.into_val(&env),
        ],
    );
    let minted = vault.deposit(&passkey_id, &deposit_amount, &None);
    assert_eq!(minted, deposit_amount);
    assert_eq!(vault.shares_of(&passkey_id), deposit_amount);

    // 3. Advance ledgers so rewards accrue, then claim through the passkey account.
    env.ledger().with_mut(|li| {
        li.sequence_number += 10_000;
    });
    mock_account_auth(
        &env,
        &vault_id,
        &passkey_id,
        "claim",
        vec![&env, passkey_id.clone().into_val(&env)],
    );
    let claimed = vault.claim(&passkey_id);
    assert!(claimed > 0);
}
