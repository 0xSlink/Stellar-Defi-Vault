# Example Frontend (with Optional Fee-Bump Sponsorship Mode)

This directory illustrates how a vault frontend integrates optional fee-bump sponsorship so users without XLM can still deposit:

- `index.html`: Minimal deposit UI with a **Sponsor fees via Fee-Bump Relayer** toggle.
- `sponsor-mode.ts`: `buildAndSubmitDeposit()` helper supporting `sponsorshipMode: "none" | "fee-bump"`.

When `"fee-bump"` is selected, the user signs only the inner `deposit` transaction and posts its base64 XDR to the sponsor relayer endpoint, which wraps it in a `FeeBumpTransaction` as documented in [`docs/SPONSORSHIP.md`](../../docs/SPONSORSHIP.md).
