# TODO: Codex TUI Operator Polish

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- [x] Create a focused TUI polish workstream instead of expanding
  Usage / Balance indefinitely.
- [x] Preserve route and balance semantics from their owning workstreams.
- [x] Prefer detail panes/help overlays over misleading table truncation.
- [x] Treat narrow terminal behavior as product behavior, not best-effort
  decoration.

## Open Questions

- [x] Should attention filters be global across TUI pages or scoped to
  Usage / Balance first?
- [ ] Should the help overlay become page-aware before or after footer cleanup?
- [ ] What is the minimum supported terminal width for full-page operation?
- [ ] Should route candidate chains use horizontal scrolling, folded summaries,
  or both?

## WS0 - Baseline And Test Fixtures

- [ ] TUI-000 Capture current Usage, Routing, Stations, and Settings behavior
  notes for normal and narrow terminal widths.
- [ ] TUI-001 Add deterministic fixtures with long provider names, CJK labels,
  unlimited quota, zero balance, stale balance, unknown balance, and refresh
  errors.
- [ ] TUI-002 Define the critical-field visibility invariants for Usage and
  Routing pages.
- [ ] TUI-003 Identify render helpers that still mix business derivation with
  terminal formatting.

## WS1 - Usage / Balance Polish

- [~] TUI-100 Add attention filters for balance errors, exhausted, stale,
  unknown, and low-balance states.
  - First slice covers error, exhausted, stale, unknown, and request-error
    rows. Configurable low-balance thresholds remain open.
- [x] TUI-101 Add independent endpoint/detail scrolling for selected provider
  details.
- [x] TUI-102 Make balance amount rendering atomic: show a complete amount or a
  clear state, never a misleading partial currency string.
- [x] TUI-103 Keep provider identity, balance status, and route impact visible
  under narrow widths.
- [ ] TUI-104 Show last refresh success/failure counts and the latest relevant
  provider error without blocking other refreshes.
- [x] TUI-105 Add render tests for long provider names and balance strings.

## WS2 - Routing Page Polish

- [ ] TUI-200 Rename or shorten repeated route-target labels where page context
  already carries the meaning.
- [ ] TUI-201 Fold long candidate chains into a count plus selected/full detail.
- [ ] TUI-202 Keep override source, selected target, and balance warnings
  visible under narrow widths.
- [ ] TUI-203 Invalidate route preview immediately after global/session
  override changes.
- [ ] TUI-204 Add render tests for long route chains, many providers, and CJK
  station/provider labels.

## WS3 - Footer And Help

- [~] TUI-300 Define page-critical footer actions for each page.
- [ ] TUI-301 Move secondary actions into a page-aware help overlay.
- [~] TUI-302 Add display-width compaction for footer segments.
- [~] TUI-303 Ensure hidden footer actions remain discoverable in help.
- [ ] TUI-304 Add tests for footer overflow and page-specific help text.

## WS4 - Page State And View Models

- [ ] TUI-400 Introduce or tighten page view models for Usage and Routing.
- [ ] TUI-401 Keep selection, viewport, and detail state synchronized after
  refresh, resize, and page switch.
- [ ] TUI-402 Remove duplicated row derivation between render, selection, and
  report/export paths.
- [ ] TUI-403 Add tests proving selected row and detail pane remain aligned
  after filtering and refresh.

## WS5 - Validation

- [ ] TUI-500 Run `cargo fmt`.
- [ ] TUI-501 Run `cargo nextest run -p codex-helper-tui`.
- [ ] TUI-502 Run workspace nextest when shared core view models change.
- [ ] TUI-503 Run clippy with GUI feature before release.
- [ ] TUI-504 Manually smoke test normal-width and narrow terminal operation.

## Candidate First Slice

Recommended first implementation goal:

1. add Usage / Balance attention filter;
2. make selected provider endpoint detail scrollable;
3. fix atomic balance amount rendering in Usage and Routing tables;
4. add narrow render tests for long provider names and CJK labels;
5. clean page footer entries touched by the above changes.
