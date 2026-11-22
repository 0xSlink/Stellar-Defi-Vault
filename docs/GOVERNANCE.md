# Governance

This document describes the lifecycle of a governance proposal, the `ProposalState` enum, and how to query the current state of any proposal.

---

## Proposal lifecycle diagram

```
                        create_proposal(user, param, value, duration)
                                       │
                                       ▼
                                    Active          ◀── voting open (ledger ≤ ends_at)
                                       │
                     cancel_proposal ──┤
                     (proposer only,   │
                      no votes cast)   ▼
                                   Cancelled ──── (terminal)
                                       │
                      ─────────────────┘
                      voting window ends (ledger > ends_at)
                                       │
                       ┌───────────────┴───────────────┐
                 passes vote?                      fails vote?
                (votes_for > votes_against          (or no quorum)
                 and quorum/threshold met)
                       │                               │
                       ▼                               ▼
                    Queued                          Expired ──── (terminal)
              (execution delay                 (can never be enacted)
               not yet elapsed)
                       │
            guardian_veto_proposal ──▶ Vetoed ──── (terminal)
                       │
              delay elapsed
                       │
                       ▼
                   Succeeded ◀── ready to enact
                       │
                enact_proposal()
                       │
           ┌───────────┴────────────┐
      passed quorum             failed quorum
      & threshold                or threshold
           │                        │
           ▼                        ▼
        Executed ──── (terminal)  Defeated ──── (terminal)
   (parameter applied)        (no change applied)
```

> **Pending** is reserved for future use, in case a pre-vote delay is added.  
> Currently, all proposals are immediately **Active** after creation.

---

## State definitions

| State | Meaning |
|---|---|
| `Active` | The voting window is open (`current_ledger ≤ ends_at`). |
| `Cancelled` | The proposer cancelled the proposal before any vote was cast. Terminal. |
| `Expired` | Voting ended but quorum or pass-threshold was not met. Can never be enacted. Terminal. |
| `Queued` | Voting ended and the proposal passed, but the execution delay has not elapsed yet. The guardian may still veto. |
| `Vetoed` | The guardian vetoed the proposal before the execution delay elapsed. Can never be enacted. Terminal. |
| `Succeeded` | Voting ended, the proposal passed, and the execution delay has elapsed. Call `enact_proposal` to apply the change. |
| `Executed` | `enact_proposal` ran and the parameter change was applied. Terminal. |
| `Defeated` | `enact_proposal` ran but quorum or pass-threshold was not met; no parameter change occurred. Terminal. |
| `Pending` | Reserved — currently identical to `Active` since there is no pre-vote delay. |

---

## Priority order for state derivation

`get_proposal_state` checks conditions in this order so that terminal states shadow later ones:

1. **Cancelled** — `proposal_cancel::is_cancelled`
2. **Executed** — `enacted == true` and the vote passed
3. **Defeated** — `enacted == true` and the vote failed
4. **Vetoed** — `guardian_veto::is_vetoed` and not yet enacted
5. **Queued** — voting ended, vote passed, `now < ends_at + execution_delay`
6. **Succeeded** — voting ended, vote passed, `now ≥ ends_at + execution_delay`
7. **Expired** — voting ended, vote failed
8. **Active** — `now ≤ ends_at`

---

## API

```rust
/// Returns None for an unknown proposal id.
pub fn get_proposal_state(env: Env, id: u32) -> Option<ProposalState>
```

Derived purely from on-chain storage and the current ledger sequence; never reverts.

### Example

```rust
match client.get_proposal_state(&proposal_id) {
    Some(ProposalState::Active)    => println!("voting is open"),
    Some(ProposalState::Succeeded) => client.enact_proposal(&proposal_id).unwrap(),
    Some(ProposalState::Executed)  => println!("already applied"),
    Some(s)                        => println!("state: {s:?}"),
    None                           => println!("unknown proposal"),
}
```

---

## Execution delay

The admin can configure a delay between a proposal passing its vote and becoming executable, giving the guardian a window to veto it:

```rust
client.set_execution_delay(&admin, &delay_ledgers);
```

A delay of `0` (the default) means a passed proposal is immediately `Succeeded` after `ends_at`.

- **`Queued`** — `ends_at < now < ends_at + execution_delay`
- **`Succeeded`** — `now ≥ ends_at + execution_delay`

---

## Quorum and pass threshold

Governance parameters can be configured per-deployment:

```rust
client.set_governance_params(&admin, &GovernanceParams {
    quorum_bps: 2_000,         // 20% participation required
    pass_threshold_bps: 6_000, // 60% of votes cast must be in favour
});
```

Parameters are snapshotted at proposal creation; changing them mid-vote does not affect open proposals.  When no parameters are configured, the simple-majority rule (`votes_for > votes_against`) applies.

---

## Related source files

| File | Purpose |
|---|---|
| `src/proposal_state.rs` | `ProposalState` enum, `proposal_state()` derivation, `get_proposal_state` entrypoint |
| `src/proposal_cancel.rs` | Bond, cancellation window, `is_cancelled` |
| `src/guardian_veto.rs` | Execution delay, `is_vetoed`, `guardian_veto_proposal` |
| `src/governance_params.rs` | Quorum / pass-threshold, `passes()` |
| `src/test_issue_843.rs` | Tests covering every state at boundary ledgers |
