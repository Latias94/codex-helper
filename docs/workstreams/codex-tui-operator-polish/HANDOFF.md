# Codex TUI Operator Polish - Handoff

Status: Historical (superseded 2026-07-12)
Last updated: 2026-05-28

> Historical status (superseded 2026-07-12): the forecast provenance and mutable operator assumptions in this handoff no longer describe the product. Forecasting was removed, and TUI/desktop now consume the same typed, redacted, query-only operator read model. The narrow-width polish evidence remains historical UI evidence.

## Current State

The lane already has user-facing usage-label cleanup committed. The next refactor slices are:

1. move Recent/History/Requests/Sessions selection synchronization out of render helpers and into UiState/page sync boundaries;
2. make Usage forecast sample provenance explicit instead of inferring it from vector shape;
3. keep the operator polish lane documented with machine-readable workstream metadata and evidence.

The current implementation has now landed those three slices and keeps the lane
open only for the manual narrow-width smoke confirmation.

## Active Task

- Task ID: TUI-006
- Owner: Codex
- Files: crates/tui/src/tui/state.rs, crates/tui/src/tui/view/pages/*.rs, crates/tui/src/tui/model.rs, docs/workstreams/codex-tui-operator-polish/*
- Validation: cargo fmt --check; cargo nextest run -p codex-helper-tui stats requests recent history help chrome --no-fail-fast; cargo nextest run -p codex-helper-tui --no-fail-fast; cargo check -p codex-helper-tui; git diff --check
- Status: AUTOMATED_GATES_PASSED
- Review: no blocking workstream or code-quality findings in this slice; manual smoke remains outside the agent shell
- Evidence: current render-time selection sync points and forecast provenance helpers in `crates/tui/src/tui/`
- Manual smoke: still required for the 110-column / 76-column terminal claim

## Decisions Since Last Update

- Reuse `codex-tui-operator-polish` instead of creating a fresh workstream.
- Treat the forecast provenance fix as a model change, not another copy of the existing Vec-length heuristic.
- Keep the narrow-width smoke requirement documented even though it is not yet automated.

## Blockers

- None.

## Next Recommended Action

- Run the manual smoke checklist in `SMOKE.md`, then close the workstream
  evidence and review gates.
