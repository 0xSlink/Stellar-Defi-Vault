# Good First Issue Onboarding Set

This is a curated onboarding set for new contributors. Each issue below is intentionally small, names exact files to touch, and includes a reviewable definition of done.

Use the live filtered list: [open `good first issue` issues](https://github.com/Rickyy1017/Stellar-Defi-Vault/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22).

## Curated starter issues

| # | Title | Type | Exact files to touch | Definition of done |
| --- | --- | --- | --- | --- |
| 1 | Document unstake fee behavior in README | Docs | `README.md`, `docs/SECURITY.md` | README includes a short "Unstake fees" section naming the admin setter, max/behavioral constraints, and where fees go; wording matches `docs/SECURITY.md` and current contract behavior. |
| 2 | Add a `cargo fmt` pre-commit reminder to contributor docs | Docs | `CONTRIBUTING.md`, `docs/CONTRIBUTING_GUIDE.md` | Both docs include the same `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` pre-commit expectation; no conflicting command variants remain in edited sections. |
| 3 | Add missing doc comment to `pool_stats` query | Docs | `src/vault.rs`, `docs/INTERFACE_SPEC.md` | `pool_stats` has a clear `///` doc comment describing return fields and auth/side-effect expectations; interface spec/docs mention matches the signature and behavior. |
| 4 | Add a README link to the incident response runbook | Docs | `README.md`, `docs/INCIDENT_RESPONSE.md` | README Operations/Security area links to `docs/INCIDENT_RESPONSE.md` with a one-line description; link path resolves in-repo. |
| 5 | Add a unit test for zero-amount deposit rejection | Test | `src/test.rs` (or existing deposit test module), `src/vault.rs` (only if test reveals mismatch) | New/updated test asserts zero-amount deposit fails with the expected `VaultError`; test passes with `cargo test --features testutils <test_name>` and does not change unrelated behavior. |
| 6 | Add a unit test for paused withdraw rejection | Test | `src/test.rs`, `src/vault.rs` (only if needed) | Test covers pause then withdraw path and asserts `VaultPaused`/expected error; test name and assertions are specific and pass locally. |
| 7 | Add script usage to `scripts/deploy_testnet.sh` header | Script | `scripts/deploy_testnet.sh`, `README.md` | Script header includes env vars, prerequisites, and example invocation; README deployment snippet matches script flags/env vars. |
| 8 | Add a Makefile `fmt-check` target | Script | `Makefile`, `CONTRIBUTING.md` | `make fmt-check` runs `cargo fmt --check` (and exits non-zero on failure); CONTRIBUTING references the target. |
| 9 | Add a docs index entry for storage layout | Docs | `docs/README.md` or `README.md` (docs section), `docs/STORAGE_LAYOUT.md` | Storage layout doc is discoverable from the main docs/index section with a one-sentence description; link resolves. |
| 10 | Add a regression test for `get_version` string | Test | `src/test.rs`, `src/vault.rs` | Test asserts `get_version()` returns the contract version constant/expected value; passes in `cargo test --features testutils` and guards against accidental version string drift. |

## Review checklist for maintainers

- The issue names exact files to touch.
- The definition of done is observable in diff/tests/docs.
- The change is small enough for a first contribution.
- Labels include `good first issue` and an appropriate type label.
