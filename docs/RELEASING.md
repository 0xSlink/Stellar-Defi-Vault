# Releasing

## Prerequisites

- Write access to the repository.
- A clean working tree on `main` (no uncommitted changes).
- `Cargo.lock` committed (it already is; never remove it from version control).
- Completed testnet release-candidate (RC) soak (see below) before tagging a mainnet release.

## Release-Candidate (RC) Testnet Soak

Every release must be deployed to Stellar Testnet as a release candidate (`vX.Y.Z-rc.N`) and run under scripted load for a fixed soak window before tagging `vX.Y.Z` for mainnet.

### Soak Duration

| Release Type | Minimum Soak Duration | Notes |
|---|---|---|
| Major / Minor (`vX.Y.0`) | **24 hours** (1,440 minutes) | Full randomized load + continuous watcher & health polling |
| Patch / Hotfix (`vX.Y.Z`) | **4 hours** (240 minutes) | Focused regression soak on testnet RC |

### Running the RC Soak

1. Deploy the RC build to Stellar Testnet (`./scripts/deploy_testnet.sh`) and record `CONTRACT_ID` and the deployment ledger.
2. Start the event watcher in a separate terminal or supervisor:
   ```bash
   RPC_URL=https://soroban-testnet.stellar.org CONTRACT_ID=$CONTRACT_ID \
     LARGE_WITHDRAWAL_THRESHOLD=1000000000 npx tsx scripts/watch-events.ts
   ```
3. Run the randomized load generator and health/solvency poller:
   ```bash
   CONTRACT_ID=$CONTRACT_ID USER_SECRETS=$TEST_USER_SECRETS \
     npx tsx scripts/soak-testnet.ts --duration-minutes 1440
   ```
4. After the soak finishes, run the post-soak invariant sweep from the deployment ledger:
   ```bash
   CONTRACT_ID=$CONTRACT_ID RPC_URL=https://soroban-testnet.stellar.org \
     npx tsx scripts/invariant-sweep.ts --start-ledger $DEPLOY_LEDGER
   ```

### Pass Criteria

A release candidate passes the soak **only** if all of the following hold for the entire window:

- **Monitoring green**: `scripts/watch-events.ts` emits zero unexpected `pause`, `circuit_breaker_trip`, or `low_reward_runway` alerts.
- **Health & solvency green**: Every `pool_health_report` and `get_solvency_report` poll succeeds (`is_paused == false`, `is_stopped == false`, `is_solvent_7_days == true`, `surplus_or_deficit >= 0`).
- **Error budget**: Transaction failure rate stays at or below the configured threshold (default `<= 5%`), with zero contract panics.
- **Invariant sweep passes**: `scripts/invariant-sweep.ts` exits `0` at the end of the soak, confirming off-chain event recomputation matches on-chain `vault_state()`.

### Recording Soak Results in Release Notes

Copy the Markdown summary printed by `scripts/soak-testnet.ts` (along with the `invariant-sweep.ts` result and `watch-events.ts` alert count) into the GitHub Release notes for `vX.Y.Z`:

```markdown
### Testnet RC Soak Results
- **RC Tag**: vX.Y.Z-rc.1
- **Status**: PASS
- **Contract ID**: `C...`
- **Window**: YYYY-MM-DDTHH:MM:SSZ -> YYYY-MM-DDTHH:MM:SSZ (1440 min)
- **Operations**: 8640 total (8640 ok, 0 failed, error rate 0.00%)
- **Health & Solvency Checks**: 1440 passed, 0 failed
- **Event Watcher Alerts**: 0 unexpected alerts
- **Post-Soak Invariant Sweep**: PASS
```

## Automated Release Drafts (Release Drafter)

Every merge to `main` updates a **draft** GitHub release via
[`.github/workflows/release-drafter.yml`](../.github/workflows/release-drafter.yml)
and [`.github/release-drafter.yml`](../.github/release-drafter.yml):

- Merged PRs are grouped into categories (`Breaking Changes`, `Security`,
  `Features`, `Bug Fixes`, `Documentation`, `Dependencies & Maintenance`) based on
  their PR labels.
- Version bump resolution is driven by labels: `breaking` / `breaking-change` /
  `semver:major` force a **major** bump, `enhancement` / `feature` / `feat` /
  `semver:minor` select a **minor** bump, and other PRs default to **patch**.
- Drafts are created with `publish: false` — a maintainer **must** review the
  draft release notes, verify the resolved version, and sync `CHANGELOG.md` and
  `Cargo.toml` before tagging or publishing.

## Steps

1. **Bump the version** in `Cargo.toml`:

   ```toml
   [package]
   version = "X.Y.Z"
   ```

2. **Update `Cargo.lock`** if the bump touches the package entry:

   ```bash
   cargo update --workspace
   ```

3. **Commit and push** to `main`:

   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "chore: bump version to vX.Y.Z"
   git push origin main
   ```

4. **Tag the release** (triggers `.github/workflows/release.yml`):

   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

## What the release workflow does

`.github/workflows/release.yml` runs on every `v*.*.*` tag push:

| Step | Detail |
|---|---|
| Checkout | Full source at the tagged commit |
| Install `cargo-cyclonedx` | Built from crates.io with `--locked` |
| Generate SBOM | `cargo cyclonedx --locked --format json` → `sbom.cdx.json` |
| Attach to GitHub release | `sbom.cdx.json` uploaded via `softprops/action-gh-release` |

The SBOM is a [CycloneDX](https://cyclonedx.org/) JSON document listing every
Rust dependency (direct and transitive) at the exact versions recorded in
`Cargo.lock`. Auditors and integrators can inspect it without cloning the
repository.

### `--locked` builds

The workflow always passes `--locked` to Cargo so the SBOM and any future Wasm
build are guaranteed to use the dependency versions recorded in `Cargo.lock`,
not whatever the resolver would pick today. Keep `Cargo.lock` committed and
up-to-date.

## Wasm artifacts (future)

The release workflow contains commented-out steps for building and attaching the
compiled Wasm binary and its SHA-256 hash. These are disabled because of the
dual-contract symbol conflict described in `ci.yml`. Once that is resolved (by
splitting contracts into separate crates), uncomment those steps and add the
Wasm files to the `files:` list in the workflow.

## Verifying an SBOM locally

```bash
cargo install cargo-cyclonedx --locked
cargo cyclonedx --locked --format json --output-file sbom.cdx.json
```

The resulting `sbom.cdx.json` can be inspected directly or fed into any
CycloneDX-compatible analysis tool (e.g. `cyclonedx-cli`, Dependency-Track).
