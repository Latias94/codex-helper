# Codex Fearless Refactor Hardening — Design

Status: Complete
Last updated: 2026-05-28

## Problem

codex-helper has several architecture surfaces that grew from local fixes: runtime logs, GUI logs,
request/control traces, relay evidence, route graph compatibility sync, request ledger read paths,
and relay diagnostics. The immediate production risk is unbounded local append-only files, which has
already produced oversized logs on existing installs. The broader risk is that compatibility and
diagnostic behavior remains spread across callers instead of being owned by deep modules.

## Target State

- Local append-only files are written through one bounded log store with startup repair, runtime
  rotation, and retention tests.
- Route authoring compatibility is owned by a single persisted-routing boundary instead of manual
  graph/compat sync calls in CLI and GUI code.
- Request ledger readers consume a store/read-model interface instead of duplicating raw JSONL
  assumptions.
- Relay live-smoke diagnostics are split by diagnostic case with a small registry/orchestrator.

## Scope

- `src/cli_app.rs`
- `crates/core/src/logging.rs`
- `crates/core/src/logging/control_trace.rs`
- `crates/core/src/proxy/codex_relay_evidence.rs`
- `crates/gui/src/gui/app.rs`
- route configuration and route-view compatibility callers
- request ledger readers used by CLI/TUI/GUI/admin surfaces
- relay diagnostic modules under `crates/core/src/proxy`

## Non-goals

- No config format removal without migration tests.
- No behavior change to route selection.
- No change to Codex/Claude upstream request semantics.
- No storage backend migration to SQLite in this lane.

## Architecture Direction

Prefer deep modules with small caller interfaces. The first module is a shared bounded log store:
callers provide a path and retention policy; the module owns rotation naming, legacy repair,
pruning, and append writer behavior. Later tasks should apply the same rule to routing,
request-ledger, and relay diagnostic boundaries.
