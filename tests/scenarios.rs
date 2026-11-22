use serde_json::{json, Value};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};
use std::{fs, path::Path};
use stellar_defi_vault::vault::{VaultContract, VaultContractClient};

struct Fixture<'a> {
    env: Env,
    vault: VaultContractClient<'a>,
    admin: Address,
    alice: Address,
    bob: Address,
}

impl<'a> Fixture<'a> {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(&env, &token_id);
        for user in [&admin, &alice, &bob] {
            token_admin.mint(user, &1_000_000);
        }

        let vault_id = env.register_contract(None, VaultContract);
        let vault = VaultContractClient::new(&env, &vault_id);
        vault.initialize(&admin, &token_id, &0, &None, &None);
        vault.set_max_lock_duration(&admin, &1_000);

        Fixture {
            env,
            vault,
            admin,
            alice,
            bob,
        }
    }

    fn actor(&self, name: &str) -> &Address {
        match name {
            "admin" => &self.admin,
            "alice" => &self.alice,
            "bob" => &self.bob,
            other => panic!("unknown scenario actor: {other}"),
        }
    }
}

fn number(value: &Value, name: &str) -> i128 {
    value[name]
        .as_i64()
        .map(i128::from)
        .unwrap_or_else(|| panic!("scenario argument {name} must be an integer"))
}

fn run_scenario(path: &Path) {
    let text = fs::read_to_string(path).expect("scenario file must be readable");
    let scenario: Value = serde_json::from_str(&text).expect("scenario file must be valid JSON");
    assert_eq!(scenario["schema_version"], 1, "{}", path.display());
    let fixture = Fixture::new();
    let mut last_result = Value::Null;

    for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        let ledger_advance = step["ledger_advance"].as_u64().unwrap_or(0) as u32;
        if ledger_advance > 0 {
            fixture.env.ledger().with_mut(|ledger| {
                ledger.sequence_number += ledger_advance;
            });
        }

        let action = step["action"]
            .as_str()
            .unwrap_or_else(|| panic!("step {index}: action is required"));
        let actor = fixture.actor(
            step["actor"]
                .as_str()
                .unwrap_or_else(|| panic!("step {index}: actor is required")),
        );
        let args = &step["args"];
        last_result = match action {
            "deposit" => json!(fixture.vault.deposit(
                actor,
                &number(args, "amount"),
                &None,
            )),
            "claim" => json!(fixture.vault.claim(actor)),
            "lock" => {
                fixture
                    .vault
                    .lock_position(actor, &(number(args, "duration_ledgers") as u32));
                json!(true)
            }
            "set_fee" => {
                fixture
                    .vault
                    .set_unstake_fee_bps(&fixture.admin, &(number(args, "bps") as u32));
                json!(true)
            }
            "unstake" => json!(fixture.vault.unstake(actor, &number(args, "shares"))),
            other => panic!("step {index}: unsupported action {other}"),
        };
    }

    for assertion in scenario["expected"].as_array().unwrap() {
        let actual = match assertion["type"].as_str().unwrap() {
            "shares" => json!(fixture.vault.shares_of(fixture.actor(
                assertion["actor"].as_str().unwrap(),
            ))),
            "total_shares" => json!(fixture.vault.vault_state().0),
            "lock_active" => json!(fixture
                .vault
                .get_lock_status(fixture.actor(assertion["actor"].as_str().unwrap()))
                .is_some()),
            "fee_bps" => json!(fixture.vault.get_unstake_fee_bps()),
            "last_result" => last_result.clone(),
            other => panic!("unsupported assertion type {other}"),
        };
        assert_eq!(
            actual, assertion["equals"],
            "{}: assertion {} failed",
            scenario["name"].as_str().unwrap(),
            assertion["type"].as_str().unwrap(),
        );
    }
}

#[test]
fn executes_every_json_scenario() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("scenarios");
    let mut paths: Vec<_> = fs::read_dir(&directory)
        .expect("scenario directory must exist")
        .map(|entry| entry.expect("scenario directory entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 10, "add at least ten golden scenarios");

    for path in paths {
        run_scenario(&path);
    }
}
