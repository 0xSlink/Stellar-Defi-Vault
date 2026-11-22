#![cfg(test)]

#[test]
fn key_management_guide_covers_required_operational_controls() {
    let doc = include_str!("../docs/KEY_MANAGEMENT.md");

    for required in [
        "hardware wallets",
        "multisig thresholds and weights",
        "separate keys per role",
        "backup procedures",
        "3-of-5 admin account",
        "stellar tx new set-options",
        "Key rotation procedure",
        "propose_admin",
        "accept_admin",
        "Key-compromise response checklist",
        "Incident Response Runbook",
    ] {
        assert!(doc.contains(required), "missing: {required}");
    }
}
