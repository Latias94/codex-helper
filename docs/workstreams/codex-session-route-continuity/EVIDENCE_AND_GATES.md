# Codex Session Route Continuity - Evidence And Gates

Status: Complete
Last updated: 2026-05-25

## Gate Plan

Targeted gates:

- `cargo nextest run -p codex-helper-core session_route_affinity --no-fail-fast`
- `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`
- `cargo nextest run -p codex-helper-core request_logs --no-fail-fast`

Final gates:

- `cargo fmt --all --check`
- `cargo nextest run -p codex-helper-core`

## Evidence Log

### 2026-05-25 - CSRC-050 documentation and closeout

- Updated README, English README, English/Chinese configuration docs, and the
  changelog with restart-safe route continuity behavior.
- Documented provider-opaque assumptions, state-bound compact fail-closed
  behavior, control-trace diagnostics, and the route affinity ledger path and
  opt-out environment variable.
- Review notes:
  - Workstream compliance: no blocking findings; tasks CSRC-010 through
    CSRC-050 satisfy the target state.
  - Code quality: no blocking findings; persistence is provider-opaque and
    tests cover restart recovery, missing affinity, and compact fallback
    boundaries.
  - Residual risk: safe cross-provider state-bound compact fallback requires a
    future explicit continuity-domain feature.
- Final gates:
  - `cargo fmt --all --check` passed.
  - `cargo nextest run -p codex-helper-core` passed: 693 tests.

### 2026-05-25 - CSRC-040 provider-opaque runtime signals

- Added provider-opaque control-trace fields for route continuity decisions:
  continuity class, affinity source, provider failover allowance, provider
  failover blocked reason, and whether balance signals are authoritative for
  the decision.
- Added missing-affinity coverage proving a provider-state-bound compact
  request returns a continuity error, does not call upstream providers, and
  emits a route continuity block trace.
- Kept balance and runtime-health meanings separate: the continuity block trace
  records `balance_signal_authoritative = false` rather than inferring relay
  internals from a balance probe.
- Gates:
  - `cargo nextest run -p codex-helper-core route_affinity request_logs --no-fail-fast` passed.
  - `cargo nextest run -p codex-helper-core control_trace route_graph_selection_explain --no-fail-fast` passed.
  - `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast` passed.
  - `cargo fmt --all --check` passed.

### 2026-05-25 - CSRC-030 continuity policy

- Added an explicit request continuity decision in provider execution.
- Preserved fallback for non-state-bound compact under `fallback-sticky`.
- Kept state-bound compact pinned to its affinity endpoint when present.
- Added fail-closed behavior for state-bound compact when no route affinity is
  known, for both route graph and legacy execution.
- Gate:
  - `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast` passed.

### 2026-05-25 - CSRC-020 durable route ledger proof

- Added a provider-opaque session route affinity ledger under helper state.
- Added restart recovery coverage proving `/responses/compact` uses the
  restored provider endpoint instead of re-entering preference-group routing.
- Added TTL coverage proving expired persisted affinities are not restored.
- Gates:
  - `cargo nextest run -p codex-helper-core session_route_affinity --no-fail-fast` passed.
  - `cargo nextest run -p codex-helper-core proxy_restores_route_affinity_after_restart_for_responses_compact --no-fail-fast` passed.

### 2026-05-25 - Workstream opened

- Created DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md.
- Initial diagnosis evidence:
  - target session compact stayed on `codex/input7/default` before helper restart,
  - helper restart lost in-memory affinity,
  - post-restart compact selected `codex/input2/default` with `affinity = null`,
  - balance probes reported `exhausted = false` while compact returned 429.

## Open Evidence

- None.
