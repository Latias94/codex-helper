# Decisions: Codex TUI Operator Polish

## D001 - Create A Focused TUI Polish Workstream

Decision:

Create `codex-tui-operator-polish` instead of continuing to place all TUI work
inside `codex-usage-balance` or the broad operator-experience refactor.

Rationale:

- Usage / Balance owns the semantic view model, not every terminal interaction.
- Operator Experience is intentionally broad and should not become the task
  tracker for narrow footer, scroll, and render-test work.
- The remaining TUI problems cross Usage, Routing, Stations, Settings, and help.

Consequence:

- Route and balance semantics stay in their owning workstreams.
- TUI polish can ship in focused slices without re-opening larger refactors.

## D002 - Preserve Meaning Before Density

Decision:

When terminal space is constrained, preserve decision-critical meaning before
visual density.

Rationale:

- A partial balance amount can be worse than no amount because it implies false
  precision.
- Current route, status, and freshness affect user action more than decorative
  labels.

Consequence:

- Tables may show shorter summaries while details carry full strings.
- Footer labels may shorten or move to help when width is limited.

## D003 - Do Not Change Core Semantics For Layout

Decision:

TUI polish must not change routing, balance, provider eligibility, or config
migration semantics.

Rationale:

- Those semantics already have dedicated workstreams and tests.
- Layout pressure should be solved with better view models and details, not by
  weakening the domain model.

Consequence:

- If a layout problem reveals a semantic issue, open or update the owning
  workstream rather than folding the change into TUI polish.

## D004 - Use Page-Aware Help Instead Of Footer Cramming

Decision:

The footer should expose page-critical actions, while secondary actions belong
in page-aware help.

Rationale:

- TUI pages now have enough actions that a single global footer cannot stay
  readable in narrow terminals.
- Users still need discoverability for less common actions.

Consequence:

- Help text and footer labels must be generated from aligned page action
  metadata where practical.
- Tests should cover both visible footer actions and discoverability through
  help.

## D005 - Test Invariants Rather Than Full Screens Only

Decision:

TUI render tests should assert critical invariants and use snapshots only where
they are stable enough to maintain.

Rationale:

- Full-screen snapshots can become noisy when unrelated copy changes.
- The important risk is losing or misleading key fields, not preserving every
  border character.

Consequence:

- Tests should assert visibility, atomic formatting, selected/detail alignment,
  and footer fallback behavior.
- Full snapshots are still acceptable for small, stable render helpers.
