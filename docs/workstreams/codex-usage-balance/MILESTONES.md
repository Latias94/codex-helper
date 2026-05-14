# Milestones: Codex Usage / Balance

## P0 - Workstream Shape

- [x] Define the product problem.
- [x] Decide that this upgrades Stats instead of creating a duplicate page.
- [x] Define core/TUI/GUI/report/doc boundaries.
- [x] Record initial decisions and open questions.

Acceptance:

- the workstream can be used as the next implementation goal;
- it is clear what is in scope and what is intentionally excluded.

## P1 - Core View Model

- [ ] Add a canonical `UsageBalanceView` or equivalent core read model.
- [ ] Move provider and endpoint usage/balance aggregation into core.
- [ ] Add routing impact summaries.
- [ ] Add tests for balance state semantics and row sorting.

Acceptance:

- TUI and GUI can render provider rows without recomputing business semantics;
- unknown, stale, exhausted, error, and unlimited states are distinguishable.

## P2 - TUI Usage / Balance

- [ ] Upgrade the existing Stats page into Usage / Balance.
- [ ] Add balance refresh shortcut and status display.
- [ ] Add provider table and selected provider detail panel.
- [ ] Add narrow-width rendering tests for long provider names and balance
  summaries.

Acceptance:

- a user can answer provider usage, cost, balance, and routing impact questions
  from the TUI page;
- route page remains focused on routing and does not become a report page.

## P3 - GUI Parity

- [ ] Render the same core rows in GUI.
- [ ] Keep refresh controls and status semantics aligned with TUI.
- [ ] Add endpoint detail display.

Acceptance:

- GUI and TUI show the same provider status for the same snapshot;
- GUI may be richer, but it does not use a different aggregation model.

## P4 - Report Export

- [ ] Export user-facing Markdown/text.
- [ ] Export debugging JSON.
- [ ] Include usage, cost, balance, freshness, refresh status, and routing
  impact.

Acceptance:

- exported reports match the same rows shown in UI;
- users can attach the report to an issue without copying raw logs manually.

## P5 - Documentation And Release Readiness

- [ ] Update user configuration/operation docs.
- [ ] Update changelog with clear user-facing wording.
- [ ] Run targeted tests and workspace clippy with GUI feature.
- [ ] Record any deferred work in TODO.

Acceptance:

- docs explain how to read the page and when to refresh balances;
- the release note does not expose internal implementation details unless they
  affect user behavior.

## Done When

- Usage / Balance is the canonical place for provider use, cost, balance, and
  quota inspection.
- TUI and GUI share one semantic data model.
- Balance refresh failures are visible and non-blocking.
- Route impact is explainable without opening raw logs.
- Existing Stats behavior is either preserved through the new page or replaced
  with documented equivalent behavior.

