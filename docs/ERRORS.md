# Error code ranges

Error codes are unsigned 32-bit values exposed through the contract interface.
They are stable ABI: once a released contract has returned a code, neither the
meaning nor the number may be changed or reused.

## Legacy codes

Existing enums retain their current numeric values. They predate this
domain-based scheme and will not be renumbered by this change. In particular,
the core `VaultError` values `1`–`50`, the companion errors mirroring those
values, and existing lock errors `51`–`56` remain unchanged. The snapshots in
[`src/test_error_codes.rs`](../src/test_error_codes.rs) protect these values.

## New code ranges

New errors use the following ranges, with the hundreds digit identifying the
domain:

| Range | Domain | Examples |
|---:|---|---|
| 100–199 | Authentication and authorization | unauthorized caller, invalid signer |
| 200–299 | Arithmetic and conversion | overflow, invalid conversion |
| 300–399 | Contract state and lifecycle | invalid state transition, missing state |
| 400–499 | Governance | proposal and voting validation |
| 500–599 | Token and transfer | token interface and transfer failures |

Do not add a new error to a legacy enum with an unrelated `1`-based code.
Place new errors in the appropriate domain range, in an enum with remaining
Soroban spec capacity, and add every new discriminant to the error-code
snapshot test. Existing codes are never moved into these ranges.

When a domain's 100-code range is exhausted, define a documented extension
range rather than reusing a retired code. A retired code remains reserved.
