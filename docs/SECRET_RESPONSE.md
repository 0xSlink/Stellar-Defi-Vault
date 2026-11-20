# Secret Incident Response

This document describes the steps to follow if a secret (private key, API
credential, RPC URL with embedded auth) is accidentally committed to this
repository (#851).

## Guiding principle: **rotate first, ask questions later**

A secret in git history is a secret that is already compromised.  Assume the
worst — that it was read the moment it was pushed — and rotate it immediately,
before doing anything else.

---

## Step-by-step response

### 1. Rotate the secret immediately

| Secret type | How to rotate |
|-------------|---------------|
| Stellar signing key (S… / mnemonic) | Generate a new keypair (`stellar keys generate`), move funds to the new address, and never use the old key again |
| RPC endpoint with auth token | Regenerate the token in your RPC provider's dashboard and update all deployments |
| GitHub Actions secret | Go to **Settings → Secrets and variables → Actions** and update or delete the secret |
| Any other API key | Consult the provider's documentation for key rotation |

**Do not wait until the git history is cleaned up.**  Rotation takes effect
immediately; history rewriting takes time and may not reach all forks.

### 2. Revoke access on-chain (if a signing key was exposed)

If the exposed key has signing authority on a Stellar account or Soroban
contract:

1. Using a *different*, secure key with sufficient weight, remove the exposed
   key from the account's signers:
   ```sh
   stellar tx new set-options \
     --source-account <SAFE_KEY_PUBLIC> \
     --remove-signer <EXPOSED_KEY_PUBLIC> \
     --network mainnet
   ```
2. If the exposed key is the sole signer, add a new key first, then remove the
   old one.
3. Verify the change with `stellar account show <ACCOUNT_ID>`.

### 3. Remove the secret from git history

> ⚠️ Rewriting published history is disruptive to collaborators.  Do this
> after rotation, not instead of it.

Use [git-filter-repo](https://github.com/newren/git-filter-repo) (preferred
over `git filter-branch`):

```sh
pip install git-filter-repo
git filter-repo --secret "THE_ACTUAL_SECRET_VALUE" --replace-text /dev/stdin <<'EOF'
THE_ACTUAL_SECRET_VALUE==>***REDACTED***
EOF
```

Then force-push every branch and ask all collaborators to re-clone.  GitHub
Support can also help remove cached views of the old commits.

### 4. Add the commit to the gitleaks allowlist

Once the secret is rotated and history is clean, add the offending commit SHA
to the `commits` list in `.gitleaks.toml` with a justification comment so the
scheduled scan does not keep alerting on it:

```toml
[allowlist]
commits = [
  "abc123de",  # Rotated 2026-01-01: deploy key accidentally committed; key revoked within 1h
]
```

### 5. Post-incident review

- Record what was exposed, for how long, and what blast radius it had.
- Check RPC and on-chain logs for suspicious activity during the exposure
  window.
- Update `.gitleaks.toml` and CI patterns if the detection rule should have
  caught this sooner.
- Share a brief internal write-up so the team can prevent recurrence.

---

## Prevention

- **Never hardcode secrets in scripts.**  Use environment variables or
  `stellar keys` / `soroban config identity` for local signing keys.
- **Use `.env` files** (git-ignored) for local credentials; keep an
  `.env.example` with placeholders.
- **Run `pre-commit install`** after cloning (see `.pre-commit-config.yaml`)
  to catch secrets before they ever reach the remote.
- **Review CI logs** for accidental `echo $SECRET` or `--secret-key $KEY`
  expansions; use `--redact` in gitleaks and mask secrets in GitHub Actions.

---

## References

- gitleaks documentation: <https://github.com/gitleaks/gitleaks>
- Stellar key management: <https://developers.stellar.org/docs/learn/glossary#keypair>
- git-filter-repo: <https://github.com/newren/git-filter-repo>
- GitHub: removing sensitive data: <https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository>
