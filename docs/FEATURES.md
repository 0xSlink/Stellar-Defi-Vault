# Cargo feature profiles

The default build enables all five optional module groups:

```text
governance, referrals, mirroring, strategies, lockdrop
```

Use the default profile to preserve the complete contract interface. A bare
vault build disables those groups:

```bash
cargo build --target wasm32-unknown-unknown --release --no-default-features --features minimal
```

Individual modules can be selected without defaults, for example:

```bash
cargo build --target wasm32-unknown-unknown --release --no-default-features --features governance
```

| Feature | Module group |
|---|---|
| `governance` | Governance parameters, roadmap voting, and daily community voting |
| `referrals` | Referral entrypoints and the shared staker network graph |
| `mirroring` | Position mirroring and the shared staker network graph |
| `strategies` | Strategy allocation, harvest, loss accounting, and unwind |
| `lockdrop` | Lockdrop campaign entrypoints |
| `minimal` | No optional module groups |

`default` enables all optional groups. `minimal` is a profile marker and must be
used with `--no-default-features`. `testutils` is independent of deployment
profiles and should only be enabled for tests. The selected set of contract
entrypoints is compiled into the Wasm spec; disabled module entrypoints are not
included.

CI builds, runs scenario tests, and reports optimized Wasm sizes for the
default, minimal, and one-feature profiles. A size increase should be reviewed
against the profile's enabled modules.
