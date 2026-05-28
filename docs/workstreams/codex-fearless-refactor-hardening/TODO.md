# Codex Fearless Refactor Hardening — TODO

Status: Complete
Last updated: 2026-05-28

## M0 — Scope And Evidence Freeze

- [x] CFR-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-fearless-refactor-hardening]
  Goal: Freeze problem, target state, non-goals, and validation anchors.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json exist and agree.
  Evidence: docs/workstreams/codex-fearless-refactor-hardening/DESIGN.md
  Handoff: Complete; first executable task is CFR-020.

## M1 — Bounded Local Log Store

- [x] CFR-020 [owner=codex] [deps=CFR-010] [scope=src/cli_app.rs,crates/core/src/local_log_store.rs,crates/core/src/logging.rs,crates/gui/src/gui/app.rs,crates/core/src/proxy/codex_relay_evidence.rs]
  Goal: Replace bespoke append-only log rotation with one bounded local log store and apply it to runtime, GUI, request/debug/control/retry traces, and relay evidence.
  Validation: cargo fmt --check; cargo nextest run -p codex-helper-core local_log_store codex_relay_evidence logging --no-fail-fast; cargo nextest run -p codex-helper --no-fail-fast; cargo check -p codex-helper-gui.
  Review: Check that existing oversized files are repaired on startup or first append.
  Evidence: crates/core/src/local_log_store.rs tests and EVIDENCE_AND_GATES.md.
  Handoff: DONE. Root runtime-log tests moved into the shared core module because the implementation now lives in codex-helper-core.

## M2 — Routing Compatibility Boundary

- [x] CFR-030 [owner=codex] [deps=CFR-020] [scope=crates/core/src/config*.rs,src/commands/routing.rs,src/commands/route_view.rs,crates/gui/src/gui/pages]
  Goal: Move graph/compat sync behind a persisted routing document boundary so callers stop manually coordinating legacy fields.
  Validation: cargo nextest run -p codex-helper-core config route routing --no-fail-fast.
  Review: No route selection behavior changes without explicit tests.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Config, CLI, GUI, and admin call sites now use semantic authoring helpers for entry route and provider-reference updates.

## M3 — Request Ledger Read Model

- [x] CFR-040 [owner=codex] [deps=CFR-020] [scope=crates/core/src/request_ledger.rs,crates/core/src/logging.rs,crates/tui/src,crates/gui/src]
  Goal: Introduce a request ledger store/read-model boundary for tail, summary, filtering, and UI consumers.
  Validation: cargo nextest run -p codex-helper-core request_ledger logging --no-fail-fast.
  Review: Preserve JSONL compatibility and UI-visible summaries.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. JSONL compatibility is preserved; recent/filter reads now use a bounded streaming window.

## M4 — Relay Diagnostics Split

- [x] CFR-050 [owner=codex] [deps=CFR-020] [scope=crates/core/src/proxy/codex_relay_live_smoke.rs,crates/core/src/proxy/codex_relay_*]
  Goal: Split relay live-smoke diagnostics by case while keeping registry/orchestration behavior intact.
  Validation: cargo nextest run -p codex-helper-core relay_live_smoke codex_live_smoke --no-fail-fast.
  Review: New case modules must be behavior-preserving.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Case registry/spec/request-body code moved into `codex_relay_live_smoke/cases.rs`.

## M5 — Closeout

- [x] CFR-060 [owner=codex] [deps=CFR-030,CFR-040,CFR-050] [scope=docs/workstreams/codex-fearless-refactor-hardening,CHANGELOG.md]
  Goal: Record final evidence, update changelog, and close or split remaining work.
  Validation: cargo fmt --check; cargo nextest run --workspace --no-fail-fast when practical.
  Review: Workstream compliance and code quality review before final commit.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json.
  Handoff: DONE. All planned slices landed with targeted validation; no follow-on split required for this lane.
