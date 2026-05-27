# Codex Remote Compaction V2 Continuity - Milestones

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Evidence Freeze

Exit criteria:

- Workstream docs agree on the v2 compact detection problem.
- Provider-opaque constraints and non-goals are explicit.
- The first implementation slice is small enough to validate independently.

## M1 - V2 Compact Classification And Logging

Exit criteria:

- `POST /responses` bodies with structured `compaction_trigger` are marked as
  v2 compact.
- `CodexBridgeLog` records `remote_compaction_v2_request`.
- Ordinary `/responses` user turns are not misclassified.

## M2 - Continuity Policy For V2 Compact

Exit criteria:

- V2 compact is included in the common remote-compaction predicate.
- Known route affinity is honored for v2 compact.
- Missing route affinity follows the active policy: `fallback-sticky` can
  bootstrap; `hard` fails closed with the existing continuity error path.
- Hard cross-provider fallback remains blocked for state-bound compact unless
  an explicit continuity domain allows it.

## M3 - Documentation And Gates

Exit criteria:

- Public docs explain that v2 compact appears as `/responses` plus
  `remote_compaction_v2_request`.
- Evidence gates are fresh.
- Residual cross-provider continuity-domain risk is recorded.

Result:

- Complete on 2026-05-26.
