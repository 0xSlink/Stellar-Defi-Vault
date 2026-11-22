# Bug Bounty Scope & Severity Classification (Immunefi-Ready)

> **Program Status:** Unfunded / Pre-Launch. As documented in [docs/SECURITY.md § Rewards](./SECURITY.md#rewards) and [docs/AUDIT_READINESS.md § Known Issues](./AUDIT_READINESS.md#known-issues), this project is currently unaudited and does not operate a funded bug bounty treasury. This specification is structured for immediate onboarding onto Immunefi (or an equivalent platform) once bounty funding is allocated.

All scope boundaries, trust assumptions, and severity definitions below are aligned with [docs/SECURITY.md](./SECURITY.md) and [docs/AUDIT_READINESS.md](./AUDIT_READINESS.md).

---

## 1. Assets in Scope

### Smart Contracts

| Asset | Location | Description |
| --- | --- | --- |
| `VaultContract` | `src/vault.rs` and wired `#[contractimpl]` modules in `src/lib.rs` | Core non-custodial Stellar DeFi Vault contract (built from `main` with `--features vault-wasm` / `--no-default-features`, excluding `testutils`). |
| `StakeReceiptNFT` | `src/nft.rs` | Receipt NFT module wired into the vault crate. |

### Critical Build & Deployment Configuration

| Asset | Location | Description |
| --- | --- | --- |
| Crate & release profile | `Cargo.toml` | Direct dependency pinning (`soroban-sdk`) and release WASM overflow/panic settings (`overflow-checks = true`, `panic = "abort"`). |
| Advisory policy | `deny.toml` | `cargo-deny` advisory allowlist and review dates. |
| CI/CD & release workflows | `.github/workflows/` | Workflows governing WASM builds, symbol checks, and release artifacts. |

### Assets Explicitly Excluded

- Test suites and test helpers (`src/test*.rs`, `fuzz/`)
- Testnet faucet contract (`contracts/test-faucet/`)
- Example consumer and indexer code (`examples/`)
- Unreleased, experimental, or unmerged branches outside `main`

---

## 2. Impact Categories & Severity Classification

Severity is assessed by realistic, demonstrated impact on `VaultContract` and its users. Monetary reward ranges are **TBD** until a bounty treasury is funded (see [docs/SECURITY.md § Rewards](./SECURITY.md#rewards)).

### Critical

Impacts resulting in direct loss of user principal, theft of rewards, or permanent protocol-wide immobilization:

- **Direct theft of user principal or accrued rewards**, whether at rest in the vault or in motion during deposit, withdrawal, exit, or claim.
- **Permanent freezing of user principal or rewards** such that funds cannot be recovered by the user or by documented admin recovery paths.
- **Unauthorized minting of vault shares** or share-price manipulation allowing an unprivileged caller to dilute existing depositors and extract their principal.
- **Bypass of documented admin-power boundaries** (see [docs/SECURITY.md § What Admin Cannot Do](./SECURITY.md#what-admin-cannot-do)), such as enabling an admin to extract user principal, change the vault token, or re-initialize the contract.
- **Protocol insolvency** breaking the accounting invariant (`contract token balance >= total_deposited + reward_pool_balance`) without an external SAC clawback.

### High

Impacts with significant but bounded fund-safety or availability consequences:

- **Bounded reward theft or accelerated reward drain** (e.g., draining the reward pool faster than the configured emission rate via rounding or timing exploits).
- **Unauthorized triggering of irreversible state** (e.g., forcing `emergency_stop()` or `start_graceful_shutdown()` without admin authorization).
- **Temporary freezing of funds** outside documented `pause()`, cooldown, or timelock windows (e.g., blocking withdrawals/claims for > 24 hours without admin pause).
- **Fee or penalty bypass / overcharge** causing material loss to users or the protocol treasury across normal operations.

### Medium

Impacts with limited blast radius or non-extractable invariant violations:

- **Incorrect accounting, event emission, or access-control behavior** with a small, bounded financial impact.
- **Documented invariant violations** that do not directly enable theft, minting, or permanent freezing of funds.
- **Griefing / targeted DoS** that delays or increases transaction cost for specific users without permanently blocking `withdraw`, `exit`, or `claim`.

### Low

- **Minor correctness issues, Soroban CPU/memory/footprint inefficiencies**, or defense-in-depth hardening suggestions without a practical exploit path.
- **Stale read-only getter values** in non-adversarial edge cases with no state corruption.

---

## 3. Known Issues (Out of Scope)

The following design decisions and accepted risks are already documented in [docs/AUDIT_READINESS.md § Known Issues](./AUDIT_READINESS.md#known-issues), [docs/SECURITY.md](./SECURITY.md), and [docs/THREAT_MODEL.md](./THREAT_MODEL.md). Reports re-stating these without demonstrating a bypass or new impact are **out of scope**:

1. **Single-admin trust model:** One admin key (and optional emergency admin) controls `pause()`, `unpause()`, rate/fee/cap configuration, `enter_withdraw_only_mode()`, and `rescue_token()` for non-stake/non-reward tokens, without on-chain timelocks on most actions. The admin still cannot withdraw user principal, mint shares to itself, change the vault token, or re-initialize.
2. **Single-step irreversible `transfer_admin`:** Admin rotation is a single atomic step with no on-chain recovery path if transferred to an unreachable address.
3. **SAC issuer trust assumptions:** Stellar Asset Contract (SAC) tokens with `AUTH_REQUIRED`, `AUTH_REVOCABLE`, or `AUTH_CLAWBACK_ENABLED` flags grant the asset issuer authority to freeze or claw back vault balances (mitigated via `set_token_risk_flags()` and `get_solvency_report()`, see [docs/THREAT_MODEL.md](./THREAT_MODEL.md)).
4. **Pinned dependency-advisory exceptions:** `adler`, `paste`, and `spin 0.9.8` exceptions in `deny.toml` pulled in transitively via `soroban-sdk`, unless a concrete exploit path in `VaultContract` is demonstrated.
5. **Unaudited / unfunded status:** Lack of a completed external audit or funded bounty pool is a known project state, not a reportable vulnerability.
6. **Resolved rounding-dust history:** Historical reward rounding dust loss (resolved in Issue #650 via the reward-per-share accumulator) is closed unless a new regression is demonstrated.

---

## 4. Out-of-Scope Items

In addition to the Known Issues above, the following are out of scope:

- Any behavior explicitly documented as intentional in [docs/SECURITY.md § Admin-Only Functions](./SECURITY.md#admin-only-functions) or [§ Failure Scenarios](./SECURITY.md#failure-scenarios).
- Denial-of-service vectors that require spending resources beyond Soroban's native fee and resource-limit model (see [docs/RESOURCE_LIMITS.md](./RESOURCE_LIMITS.md)), unless a missing or bypassable bound is shown.
- Attacks on third-party infrastructure (GitHub, RPC providers, wallets, Stellar validators), social engineering, or physical access to maintainer/user keys.
- Findings without a demonstrated security impact (code style, missing doc comments, Clippy/lint suggestions, theoretical issues without a PoC).
- Issues only present in unmerged branches, forks, or test-only code.

---

## 5. Rules for Proof-of-Concept (PoC) Testing

Researchers **must** adhere to the following rules to remain covered under [docs/SECURITY.md § Safe harbor](./SECURITY.md#safe-harbor):

1. **Testnet Fork / Local Sandbox Only:**
   - All PoCs must be developed and run exclusively against a **local Soroban test environment** (`cargo test --features testutils`), a **local Stellar quickstart/RPC node**, or a **Stellar Testnet fork** using accounts you control.
   - **Never** execute exploit PoCs against Stellar Mainnet, any shared live deployment, or accounts/funds belonging to other users.
2. **No Infrastructure Disruption:**
   - Do not run high-volume automated scanners or stress tests against public Stellar RPC or Horizon endpoints.
3. **Minimal Demonstration:**
   - Demonstrate the vulnerability with the minimum state and token amounts necessary (e.g., a self-contained Rust unit/integration test reproducing the exploit).
4. **Private Reporting & Coordinated Disclosure:**
   - Submit reports privately via [GitHub Security Advisories](../../security/advisories/new) (never via public issues or PRs).
   - Include: affected file(s) and entrypoint(s), commit hash/tag tested, impact classification, and a runnable PoC.
   - Follow the 90-day / post-advisory coordinated disclosure policy in [docs/SECURITY.md § Coordinated disclosure](./SECURITY.md#coordinated-disclosure).
