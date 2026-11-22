# Key Management and Admin Account Setup

This guide describes how to hold the admin, guardian, and upgrader roles safely using Stellar account features. It complements [SECURITY.md](./SECURITY.md) and the [incident response runbook](./INCIDENT_RESPONSE.md).

## Roles

| Role | Contract responsibility | Recommended key custody |
| --- | --- | --- |
| Admin | Configuration, pause/unpause, upgrades, role assignment, and other admin-gated actions documented in `docs/SECURITY.md`. | Hardware wallet backed by a multisig account. Never a hot key on a shared server. |
| Guardian | Emergency pause capability when configured. | Separate hardware wallet, held by a different operator than admin signers. |
| Upgrader | Contract upgrade authority when separated from day-to-day admin operations. | Separate hardware wallet or multisig signer set; do not reuse the routine admin signer. |

Use a different key for each role. Do not reuse the same seed, hardware wallet account path, or signer across admin, guardian, and upgrader responsibilities.

## Hardware wallets

- Store every role key on a hardware wallet. Do not keep role seeds in plaintext files, chat logs, CI secrets, or browser extensions.
- Use a dedicated device profile for production operations. Avoid using the same browser profile for day-to-day browsing and signing.
- Verify the Stellar address on the hardware wallet screen before signing any transaction.
- Keep firmware current, but do not upgrade firmware during an active incident or immediately before a planned admin action.
- Record the derivation path/account label in an offline operations record. Do not record the seed phrase digitally.

## Multisig thresholds and weights

For production admin control, use a Stellar multisig account instead of a single signer.

- **Thresholds**: Set low/medium/high thresholds so routine admin actions require the expected quorum and high-risk actions cannot be executed by one compromised signer.
- **Weights**: Assign each signer a weight that reflects its custody quality. Hardware-wallet signers operated by different people should have equal weight unless your policy explicitly requires otherwise.
- **Separation of duties**: No single person should control enough signers to reach the admin threshold alone.
- **Reserve signers**: Keep backup signers offline in separate physical locations. Backup signers should be able to participate in rotation or recovery, but should not be used for routine operations.
- **Test first**: Before moving production authority, test the multisig configuration on Testnet or Futurenet with the same threshold and signer weights.

## Worked example: 3-of-5 admin account

Goal: require any 3 of 5 hardware-wallet signers to authorize an admin action. Each signer has weight 1; thresholds are 3.

Example public keys (replace with your real signer public keys):

```text
PRIMARY_ADMIN=GPRIMARYADMINACCOUNTXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
SIGNER_A=GSIGNERAEXAMPLEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
SIGNER_B=GSIGNERBEXAMPLEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
SIGNER_C=GSIGNERCEXAMPLEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
SIGNER_D=GSIGNERDEXAMPLEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
SIGNER_E=GSIGNERDEXAMPLEXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
```

Configure the account with Stellar CLI:

```bash
# Add five signers, each with weight 1.
stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --signer "$SIGNER_A" --signer-weight 1

stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --signer "$SIGNER_B" --signer-weight 1

stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --signer "$SIGNER_C" --signer-weight 1

stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --signer "$SIGNER_D" --signer-weight 1

stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --signer "$SIGNER_E" --signer-weight 1

# Require 3 signatures for low/medium/high thresholds.
stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --low-threshold 3 \
  --medium-threshold 3 \
  --high-threshold 3

# Optional: reduce the master key weight so it cannot act alone.
stellar tx new set-options \
  --source-account "$PRIMARY_ADMIN" \
  --master-weight 0
```

Operational notes:

- Run the commands against the intended network and verify each transaction hash.
- If you set `--master-weight 0`, make sure at least 3 signer keys are already installed and accessible; otherwise the account can become unusable.
- Keep the five signers on separate hardware wallets and in separate physical custody.
- Document which 3 signers are required for routine changes and which backup signers are used for recovery.

## Backup procedures

- Create two or more offline backups of each recovery seed phrase. Store them in separate secure locations.
- Never photograph, email, cloud-sync, or print seeds to a shared printer.
- Use tamper-evident storage for seed backups.
- Keep an offline record of: account addresses, signer public keys, thresholds, derivation paths, hardware wallet serial/label, and who has custody.
- Periodically verify backups by performing a recovery drill on a spare hardware wallet in an offline/controlled environment. Do not expose the seed on a general-purpose computer.
- When a custodian leaves or a device is lost, treat it as a rotation event, not a documentation-only update.

## Key rotation procedure

The contract uses a two-step admin transfer. The current admin proposes the new admin, and the new admin accepts.

1. **Prepare the new admin account**. Create a new hardware-wallet-backed multisig account (for example, a new 3-of-5 set). Verify all signers and thresholds before starting the rotation.
2. **Confirm contract state**. Read the current admin, pending admin, pause state, and any active timelocks before submitting transactions.
3. **Propose the transfer** from the current admin:

   ```bash
   stellar contract invoke \
     --id <VAULT_CONTRACT_ID> \
     --source-account <CURRENT_ADMIN> \
     --network <NETWORK> \
     -- \
     propose_admin \
     --admin <CURRENT_ADMIN_ADDRESS> \
     --new_admin <NEW_ADMIN_ADDRESS>
   ```

4. **Verify pending state**. Confirm `get_pending_admin()` returns the expected pending admin and that no unexpected admin event occurred.
5. **Accept the transfer** from the new admin:

   ```bash
   stellar contract invoke \
     --id <VAULT_CONTRACT_ID> \
     --source-account <NEW_ADMIN_ADDRESS> \
     --network <NETWORK> \
     -- \
     accept_admin
   ```

6. **Verify final state**. Confirm `get_admin()` (or the contract's admin query) returns the new admin and that the pending admin is cleared.
7. **Revoke old access**. Remove old signers from multisig accounts, rotate guardian/upgrader keys if they shared custody, and update the offline operations record.

Never rotate directly to an untested address. A bad destination can permanently lock admin operations.

## Key-compromise response checklist

Use this checklist immediately if any role key, seed backup, hardware wallet, or signing workstation is suspected compromised. Follow the [Incident Response Runbook](./INCIDENT_RESPONSE.md) for severity, communications, and evidence handling.

- [ ] **Classify severity**. Treat confirmed or credible admin/guardian/upgrader compromise as SEV-1.
- [ ] **Contain**. If an uncompromised guardian is available, pause the vault. If only admin is available and uncompromised, use the documented pause path. Preserve ledger/time, tx hashes, and RPC responses.
- [ ] **Assess blast radius**. Identify which keys signed recent transactions, which role they held, and whether backup seeds or signing devices were exposed.
- [ ] **Rotate authority**. If the current admin is still uncompromised, transfer admin using the two-step procedure above to a newly created secure account. Rotate guardian and upgrader keys separately.
- [ ] **Revoke compromised signers**. Remove compromised signer keys from Stellar multisig accounts and replace them with new hardware-wallet signers. Re-check thresholds after every signer change.
- [ ] **Invalidate operational access**. Revoke RPC/API credentials, CI secrets, password manager sessions, and workstation access associated with the compromised custodian or device.
- [ ] **Communicate**. Use the incident communication templates in [INCIDENT_RESPONSE.md](./INCIDENT_RESPONSE.md). Do not disclose seeds, private signer details, or unconfirmed loss figures.
- [ ] **Post-incident review**. Record root cause, timeline, corrective actions, and deadline. Update this guide and operational records before closing the incident.
