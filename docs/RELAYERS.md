# Relayers, Smart Accounts, and Passkey (`secp256r1`) Signers

`VaultContract` authorizes both users and administrators via Soroban's `Address::require_auth()` framework. Because Soroban delegates authorization for contract addresses (`C...`) to the account contract's `__check_auth` entrypoint, **both user flows (`deposit`, `claim`, `withdraw`) and admin flows (`set_reward_rate_bps`, `pause`, etc.) work transparently when held by a smart account or passkey wallet**.

## Passkey (`secp256r1`) Smart Accounts

Modern smart wallets authenticate users with WebAuthn / FIDO2 passkeys backed by NIST P-256 (`secp256r1`) hardware keys rather than Ed25519 keypairs.

On Soroban (Protocol 21+, CAP-0051), a passkey smart account implements `CustomAccountInterface`:

1. Stores the 65-byte uncompressed SEC1 `secp256r1` public key (`0x04 || x || y`) in contract instance storage.
2. Implements `__check_auth(env, signature_payload, signature, auth_contexts)` and verifies the 64-byte low-S normalized ECDSA signature using `env.crypto().secp256r1_verify(&public_key, &signature_payload, &signature)`.
3. Optionally validates WebAuthn `authenticatorData` and `clientDataJSON` challenge binding against `signature_payload` before calling `secp256r1_verify`.

### Verified Flows

[`src/test_passkey_account.rs`](../src/test_passkey_account.rs) exercises a `MockPasskeyAccount` contract implementing `secp256r1_verify` in `__check_auth` and confirms:

- **Signature verification**: Valid low-S RFC 6979 P-256 signatures succeed; tampered signatures fail.
- **Admin call**: With `MockPasskeyAccount` set as the vault admin, `set_reward_rate_bps` executes and updates the pool rate.
- **User deposit & claim**: `deposit` and `claim` execute with `MockPasskeyAccount` as the staker address.

For general contract-account admin tests (independent of signature scheme), see [`src/test_admin_contract_auth.rs`](../src/test_admin_contract_auth.rs).

## Resource Cost of `secp256r1` Signature Verification

When budgeting Soroban CPU instructions, memory, and relayer `maxFee` caps for passkey smart accounts, keep the following cost characteristics in mind:

- **Host-native execution**: `env.crypto().secp256r1_verify` runs as a Soroban host function (CAP-0051) rather than pure-Wasm elliptic-curve arithmetic, keeping it well within the per-transaction Soroban CPU budget.
- **Higher CPU than Ed25519**: P-256 ECDSA verification costs roughly **3x–5x more CPU instructions** than `ed25519_verify` (typically ~150k–300k CPU instructions vs. ~40k–80k for Ed25519), plus extra host calls if `__check_auth` hashes WebAuthn `clientDataJSON` and `authenticatorData` with `env.crypto().sha256`.
- **Larger auth payload**: A `secp256r1` credential carries a 65-byte uncompressed public key and 64-byte signature (and optional WebAuthn JSON/authData bytes), increasing transaction size and bandwidth fees compared to a 32-byte Ed25519 key.
- **Relayer & multisig guidance**:
  - Account for `__check_auth` overhead when setting `MAX_SPONSOR_FEE` in a fee-bump relayer (see [`docs/SPONSORSHIP.md`](./SPONSORSHIP.md)).
  - For passkey multisig admins, keep threshold `M-of-N` small (e.g., `2-of-3`) so repeated `secp256r1_verify` calls in a single `__check_auth` invocation stay comfortably below the Soroban transaction instruction limit.
  - Ensure signatures are **low-S normalized** (`s <= n / 2`) client-side before submission; high-S signatures are rejected by `secp256r1_verify`.

## Relayer Integration

Relayers can submit transactions on behalf of passkey smart accounts in two ways:

1. **Meta-transactions (`claim_on_behalf`)**: A user approves an authorized relayer via `approve_relayer(user, relayer)` so the relayer can call `claim_on_behalf(relayer, user)` without requiring a passkey prompt on every claim.
2. **Fee-bump / source-account sponsorship**: Because a contract account (`C...`) cannot be the outer `sourceAccount` of a Stellar transaction, a relayer or sponsor account acts as the outer `sourceAccount` (paying XLM fees) while the passkey smart account signs the Soroban `SorobanAuthorizationEntry` payload. See [`docs/SPONSORSHIP.md`](./SPONSORSHIP.md) for sponsor rate-limiting and abuse controls.
