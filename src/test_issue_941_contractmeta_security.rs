#![cfg(test)]

use crate::vault::{CONTRACT_NAME, CONTRACT_VERSION};

#[test]
fn security_contact_metadata_values_match_security_policy() {
    // contractmeta! requires string literals in vault.rs; this test guards
    // the expected values documented in docs/SECURITY.md and issue #941.
    let expected_contact =
        "https://github.com/Rickyy1017/Stellar-Defi-Vault/security/advisories/new";
    let expected_repo = "https://github.com/Rickyy1017/Stellar-Defi-Vault";
    let expected_policy =
        "https://github.com/Rickyy1017/Stellar-Defi-Vault/blob/main/docs/SECURITY.md";

    let security = include_str!("../docs/SECURITY.md");
    assert!(security.contains(expected_contact));
    assert!(security.contains(expected_repo));
    assert!(security.contains(expected_policy));

    let vault_src = include_str!("vault.rs");
    assert!(vault_src.contains(&format!("key = \"security_contact\",")));
    assert!(vault_src.contains(&format!("val = \"{expected_contact}\"")));
    assert!(vault_src.contains(&format!("key = \"source_repo\",")));
    assert!(vault_src.contains(&format!("val = \"{expected_repo}\"")));
    assert!(vault_src.contains(&format!("key = \"security_policy_url\",")));
    assert!(vault_src.contains(&format!("val = \"{expected_policy}\"")));

    // Keep existing name/version metadata assertions colocated for drift safety.
    assert_eq!(CONTRACT_NAME, "stellar-staking-pool");
    assert_eq!(CONTRACT_VERSION, env!("CARGO_PKG_VERSION"));
}
