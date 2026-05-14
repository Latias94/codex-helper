# TODO: Codex Usage / Balance

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- [x] Upgrade the existing Stats concept instead of adding a competing page.
- [x] Core owns the semantic view model; TUI/GUI only render it.
- [x] Route pages keep compact balance context; detailed analysis lives in
  Usage / Balance.
- [x] Balance refresh failures are visible but non-blocking.
- [x] Unknown balance must not be displayed as healthy.

## Open Questions

- [x] Should the TUI tab label change from `Stats` to `Usage` immediately, or
  keep `Stats` for one release and rename later?
- [ ] Should export default to Markdown, JSON, or both?
- [ ] Which route impact summaries are required for the first release:
  balance only, or balance plus health/cooldown/model/auth?
- [ ] Do we need a user-configurable attention threshold for low balance or
  quota remaining?
- [ ] Should refresh status show the last global refresh only, or also
  per-provider refresh ages?

## WS0 - Baseline And Inventory

- [x] UBG-000 Inventory current Stats, balance, routing explain, and request
  ledger data sources.
- [ ] UBG-001 Capture current TUI Stats screenshot/behavior notes for
  regression comparison.
- [ ] UBG-002 Capture current GUI balance/stats behavior and duplicated logic.
- [x] UBG-003 List all balance status states and their current display strings.
- [x] UBG-004 Define the first release column set for TUI and GUI.

## WS1 - Core UsageBalanceView

- [x] UBG-100 Add a core view model for usage and balance rows.
- [x] UBG-101 Add provider-level rows with usage, cost, primary balance, and
  refresh age.
- [x] UBG-102 Add endpoint-level rows for selected-provider detail.
- [x] UBG-103 Add routing impact summaries from route explain/runtime state.
- [~] UBG-104 Add refresh status summary including attempted/refreshed/failed
  and missing-token counts.
- [x] UBG-105 Add deterministic sorting and filter helpers.
- [~] UBG-106 Add unit tests for ok, exhausted, stale, unknown, error,
  unlimited, subscription, paygo, and quota-only snapshots.

## WS2 - TUI Page

- [x] UBG-200 Rename or relabel the Stats page as Usage / Balance according to
  the accepted transition plan.
- [x] UBG-201 Replace scattered row formatting with the core view model.
- [x] UBG-202 Add a summary band for totals, cost, balance status counts, and
  refresh status.
- [x] UBG-203 Add provider table columns for usage, cost, balance, freshness,
  and routing impact.
- [x] UBG-204 Add selected provider detail panel with endpoint rows and latest
  balance lookup errors.
- [x] UBG-205 Add `g` refresh balances to the page footer and help text.
- [ ] UBG-206 Add attention filters for errors and balance states.
- [ ] UBG-207 Add TUI rendering tests for narrow columns and long provider
  names.

## WS3 - GUI Page

- [x] UBG-300 Move GUI balance/stats rows onto the shared core view model.
- [ ] UBG-301 Add provider table sorting for cost, requests, status, and
  balance freshness.
- [ ] UBG-302 Add provider detail with endpoint balance snapshots.
- [x] UBG-303 Show refresh progress, last message, and last error consistently.
- [x] UBG-304 Keep GUI wording aligned with TUI and docs.

## WS4 - Report Export

- [~] UBG-400 Define report DTO using the same view model.
- [~] UBG-401 Add Markdown/text export for user-facing issue reports.
- [ ] UBG-402 Add JSON export for debugging.
- [ ] UBG-403 Include route impact and balance refresh metadata in exports.
- [ ] UBG-404 Add tests proving exported rows match UI view rows.

## WS5 - Documentation And Changelog

- [x] UBG-500 Update user docs with "how to read Usage / Balance".
- [x] UBG-501 Document balance states and refresh behavior.
- [x] UBG-502 Document why route page only shows compact balance context.
- [x] UBG-503 Add changelog entry with user-facing wording.

## WS6 - Validation

- [x] UBG-600 Run TUI tests.
- [x] UBG-601 Run GUI feature clippy.
- [ ] UBG-602 Add or update snapshot/rendering tests for truncation risks.
- [ ] UBG-603 Manually verify narrow terminal route/usage layouts.
- [x] UBG-604 Verify balance refresh failure does not block other refreshes or
  TUI redraw.
