# Codex TUI Operator Read Model Refactor - Handoff

Status: Active
Last updated: 2026-05-28

## Current State

The architecture review found that `crates/tui/src/tui/model.rs` still builds
operator provider/station rows directly from raw `ProxyConfig`, while core
already owns related dashboard and operator summary contracts. This workstream
tracks the first fearless-refactor slice that moves those semantics into
`dashboard_core`.

## Active Task

- Task ID: ORM-100
- Owner: Codex
- Files: `crates/core/src/dashboard_core/types.rs`,
  `crates/core/src/dashboard_core/station_options.rs`,
  `crates/tui/src/tui/model.rs`,
  `docs/workstreams/codex-tui-operator-read-model/*`
- Validation: `cargo fmt --check`; `cargo nextest run -p codex-helper-core -p codex-helper-tui --no-fail-fast`; `cargo check -p codex-helper-tui`; `git diff --check`
- Status: AUTOMATED_GATES_PASSED

## Completed Slice

`ORM-100` added `RuntimeProviderOption` and `RuntimeUpstreamOption` to
`dashboard_core`, plus `build_runtime_provider_options_from_mgr`. The TUI
`ProviderOption` and `UpstreamSummary` names are now compatibility aliases for
those core-owned rows, and `build_provider_options` delegates to the core
builder.

## Decisions Since Lane Open

- Open a dedicated cross-crate read-model lane instead of expanding the TUI
  polish lane.
- Keep the first implementation compatible with the existing TUI
  `ProviderOption` name while moving semantic derivation to core.

## Blockers

- None.

## Next Recommended Action

Start `ORM-200` by inventorying the remaining TUI raw-config read-model
derivations, then decide whether to converge attached TUI on the operator
summary payload before renaming the transitional provider terminology.
