# Codex Official Relay Bridge — TODO

Status: Complete
Last updated: 2026-05-18

## M0 — Scope And Evidence Freeze

- [x] CORB-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-official-relay-bridge]
  Goal: Freeze first-stage target around remote compaction v1 through official relay behavior.
  Validation: workstream docs exist and agree on non-goals, staged scope, and evidence anchors.
  Evidence: `docs/workstreams/codex-official-relay-bridge/DESIGN.md`
  Handoff: WebSocket and remote compaction v2 are deliberately deferred until v1 HTTP compact works.

## M1 — Remote Compact V1 Bridge Proof

- [x] CORB-020 [owner=main] [deps=CORB-010] [scope=crates/core/src/codex_integration.rs,crates/core/src/proxy]
  Goal: Add a Codex patch mode and proxy behavior that make `/responses/compact` reachable through helper-backed relays.
  Validation: targeted unit tests prove Codex TOML patch output and compact request routing/logging behavior.
  Review: review-workstream before accepting completion.
  Evidence: `cargo nextest run -p codex-helper-core`; targeted official relay tests recorded in `EVIDENCE_AND_GATES.md`.
  Handoff: Added `official-relay-bridge`, preserving default/chatgpt/imagegen behavior; WebSocket remains disabled and v2 compaction remains deferred. Unsupported `/responses/compact` statuses remain visible for operator fallback diagnostics.

## M2 — Operator Configuration And Diagnostics

- [x] CORB-030 [owner=main] [deps=CORB-020] [scope=docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,logging surfaces]
  Goal: Document how operators opt into official relay compact mode and diagnose compact request type from logs.
  Validation: docs match implemented CLI/config surface; log examples avoid sensitive data.
  Review: review-workstream before accepting completion.
  Evidence: `cargo nextest run -p codex-helper-core request_ledger`; `cargo nextest run -p codex-helper-core capabilities`; `cargo nextest run -p codex-helper-gui request_ledger`; `cargo run -q --bin codex-helper -- usage find --path responses/compact --limit 20`.
  Handoff: Documented CLI/admin request-ledger `path` filtering and known sub2api-style `/responses/compact` expectations.

## M3 — Capability Hints And Fallback Hardening

- [x] CORB-040 [owner=main] [deps=CORB-020] [scope=config/routing/proxy as needed]
  Goal: Decide whether helper needs explicit compact-capability hints or can rely on user-selected mode for first release.
  Validation: tests or design note prove unsupported relays fail predictably and can revert to compatible mode.
  Review: review-workstream before accepting completion.
  Evidence: `docs/CONFIGURATION.md`; `docs/CONFIGURATION.zh.md`; `EVIDENCE_AND_GATES.md`
  Handoff: First release stays static and user-selected. Unsupported compact relays should fail visibly on `/responses/compact`, and operators can switch back to `default`. Active probing/capability hints are deferred to a follow-on.

## M4 — Closeout

- [x] CORB-050 [owner=main] [deps=CORB-030,CORB-040] [scope=docs/workstreams/codex-official-relay-bridge]
  Goal: Run final gates, record evidence, and split WebSocket/v2 follow-ons if still pending.
  Validation: fresh verification evidence exists before marking the lane complete.
  Review: review-workstream has no blocking findings.
  Evidence: `cargo nextest run --workspace`; `EVIDENCE_AND_GATES.md`, `WORKSTREAM.json`
  Handoff: WebSocket upgrade forwarding, remote compaction v2, and active compact probing/capability hints remain follow-ons.
