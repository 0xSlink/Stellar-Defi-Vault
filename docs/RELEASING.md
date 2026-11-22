# Releasing

## Prerequisites

- Write access to the repository.
- A clean working tree on `main` (no uncommitted changes).
- `Cargo.lock` committed (it already is; never remove it from version control).

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
