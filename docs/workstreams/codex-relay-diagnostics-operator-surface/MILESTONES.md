# Codex Relay Diagnostics Operator Surface — Milestones

Status: Implemented
Last updated: 2026-05-19

## M0 — Scope And Evidence Freeze

Exit criteria:

- Problem and target state are explicit.
- TUI-first scope is explicit.
- Non-goals rule out periodic probes and automatic mutation.

Primary evidence:

- `docs/workstreams/codex-relay-diagnostics-operator-surface/DESIGN.md`
- `docs/workstreams/codex-relay-diagnostics-operator-surface/TODO.md`

## M1 — Reusable Core Diagnostic Contract

Exit criteria:

- HTTP route delegates to a reusable core service method.
- Request/response DTOs can be consumed by TUI without duplicating JSON structs.
- Existing admin API tests still pass.

Primary gates:

- `cargo nextest run -p codex-helper-core codex_capabilities_api`

## M2 — TUI Settings Diagnostic Surface

Exit criteria:

- Settings page advertises and triggers a manual diagnostic action.
- Diagnostic runs asynchronously and cannot block the key handler for upstream timeout duration.
- Result block displays target, expected/observed status, mismatches, recommendation, warnings, and failure state.

Primary gates:

- `cargo nextest run -p codex-helper-tui codex_relay_diagnostics`

## M3 — Docs And Closeout

Exit criteria:

- Configuration docs and changelog mention the TUI path.
- Final formatting and targeted tests pass.
- Remaining GUI/CLI work is deferred or split.

Primary gates:

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core codex_capabilities_api`
- `cargo nextest run -p codex-helper-tui codex_relay_diagnostics`
