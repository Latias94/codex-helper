# Codex Session Route Continuity - Milestones

Status: Complete
Last updated: 2026-05-25

## M0 - Scope And Evidence Freeze

Exit criteria:

- Workstream docs agree on the provider-opaque continuity problem.
- The first executable task is small enough to validate independently.

## M1 - Durable Route Ledger Proof

Exit criteria:

- Successful route affinity is persisted by stable provider endpoint identity.
- A new proxy state can restore the affinity before route selection.
- Invalid, expired, or pruned affinity does not crash startup.
- Targeted tests prove restart recovery.

## M2 - Continuity Policy And Fail-Closed Compact

Exit criteria:

- Request continuity is represented explicitly rather than as scattered compact
  booleans.
- State-bound compact with missing affinity fails closed or reports a clear
  continuity error.
- Non-state-bound compact keeps the existing safe fallback behavior when the
  affinity endpoint fails.
- State-bound compact does not silently fallback after affinity endpoint
  failure unless a future explicit continuity domain allows it.

## M3 - Provider-Opaque Runtime Signals

Exit criteria:

- Logs distinguish balance state from runtime request health.
- 429 compact failures are observable without assuming the relay implementation.
- Control trace records why provider failover was allowed or blocked.

## M4 - Documentation And Closeout

Exit criteria:

- Public docs explain the restart-safe continuity behavior.
- Evidence gates are fresh.
- Residual risks are recorded in HANDOFF.md.

Result:

- Complete on 2026-05-25.
