# Codex Official Relay Bridge — Milestones

Status: Complete
Last updated: 2026-05-18

## M0 — Scope And Evidence Freeze

Exit criteria:

- The first deliverable is explicitly remote compaction v1 over HTTP.
- WebSocket and remote compaction v2 are non-goals for the first slice.
- The initial task ledger has bounded file scopes and validation gates.

Primary evidence:

- `docs/workstreams/codex-official-relay-bridge/DESIGN.md`
- `docs/workstreams/codex-official-relay-bridge/TODO.md`

## M1 — Remote Compact V1 Bridge Proof

Exit criteria:

- Codex patch mode can advertise official-provider semantics needed by remote compaction v1.
- Helper can route `/responses/compact` without breaking ordinary `/responses`.
- Tests cover compatibility with existing patch modes.

Primary gates:

- `cargo nextest run -p codex-helper-core codex_integration`
- `cargo nextest run -p codex-helper-core responses_compact`

## M2 — Operator Configuration And Diagnostics

Status: Complete

Exit criteria:

- [x] Operators know when to use the official relay bridge mode.
- [x] Logs or documented queries distinguish `/responses` fallback from `/responses/compact`.
- [x] Documentation covers sub2api-style relay expectations.

Primary gates:

- `cargo fmt --check`
- targeted docs-adjacent tests if available

## M3 — Capability Hints And Fallback Hardening

Status: Complete for first release; active probing deferred.

Exit criteria:

- [x] Unsupported relays have a documented and testable fallback story.
- [x] Any active probing requirement is either implemented narrowly or split into a follow-on.

Primary gates:

- Targeted config/proxy tests chosen after implementation shape is clear.

## M4 — Closeout

Status: Complete

Exit criteria:

- [x] All completed tasks have fresh evidence.
- [x] Remaining WebSocket/v2 work is explicitly deferred or split.
- [x] `WORKSTREAM.json` status and `HANDOFF.md` match the actual implementation state.

Primary gates:

- `cargo fmt --check`
- `cargo nextest run --workspace`
