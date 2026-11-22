# JSON scenario tests

Every `tests/scenarios/*.json` file is run by the `scenarios` integration test.
Each file creates a fresh vault and runs its steps in order.

```json
{
  "schema_version": 1,
  "name": "deposit and lock",
  "steps": [
    {
      "action": "deposit",
      "actor": "alice",
      "args": { "amount": 1000 },
      "ledger_advance": 0
    },
    {
      "action": "lock",
      "actor": "alice",
      "args": { "duration_ledgers": 10 },
      "ledger_advance": 0
    }
  ],
  "expected": [
    { "type": "shares", "actor": "alice", "equals": 1000 },
    { "type": "lock_active", "actor": "alice", "equals": true }
  ]
}
```

Supported actors are `admin`, `alice`, and `bob`. Supported actions are
`deposit` (`amount`), `claim`, `lock` (`duration_ledgers`), `set_fee` (`bps`),
and `unstake` (`shares`). `ledger_advance` is an optional number of ledgers to
advance before the action. Assertions support `shares`, `total_shares`,
`lock_active`, `fee_bps`, and `last_result`.

Add a file with a unique descriptive name and include at least one assertion.
Keep amounts within the fixture's funded balances. Run all scenarios with
`cargo test --test scenarios`.
