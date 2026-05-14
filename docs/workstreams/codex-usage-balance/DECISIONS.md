# Decisions: Codex Usage / Balance

## Accepted

### D001 - Upgrade Stats Instead Of Adding A Duplicate Page

The existing Stats page should become the Usage / Balance surface or feed it
directly.

Reason:

- users should not have to choose between "Stats" and "Usage" pages that answer
  overlapping questions;
- the current Stats page already owns the correct navigation slot;
- replacing the shape is safer than accumulating competing tables.

### D002 - Core Owns The Semantic View Model

TUI, GUI, and report export must consume one shared core view model.

Reason:

- usage, balance, cost, and routing impact have product semantics;
- duplicating aggregation in GUI and TUI will create subtle inconsistencies;
- tests are easier when the semantic rows are built once.

### D003 - Route Page Shows Compact Balance Only

The route page should remain focused on routing controls and route explanation.

Reason:

- route editing and usage inspection are different user jobs;
- a full balance report in the route page makes shortcuts and layout fragile;
- compact balance context is still useful while choosing route targets.

### D004 - Unknown Balance Is Not Healthy

Unknown, stale, exhausted, error, and unlimited balance states must remain
distinct.

Reason:

- unknown can mean unsupported adapter, missing token, network failure, or not
  yet refreshed;
- showing unknown as ok makes routing and cost decisions unsafe;
- unlimited quota is a positive known state, not the same as unknown.

### D005 - Refresh Failure Is Non-Blocking

Balance refresh failure must not interrupt other refreshes, snapshot refresh,
or TUI redraw.

Reason:

- one flaky provider should not degrade the whole control plane;
- the user still needs to inspect stale/error state after a failure;
- refresh status is operational data, not a fatal UI condition.

## Pending

### P001 - Page Rename Timing

Options:

- rename tab from `Stats` to `Usage` immediately;
- show `Stats` in navigation but title the page `Usage / Balance`;
- keep `Stats` for one release and rename later.

Recommendation:

- use `Usage` or `Usage / Balance` in the page title immediately;
- choose tab label based on available header width and migration risk.

### P002 - Export Format

Options:

- Markdown/text only;
- JSON only;
- both.

Recommendation:

- both, because the user-facing report and debugging attachment solve different
  problems and can share the same view model.

### P003 - First Release Routing Impact Scope

Options:

- balance-only impact;
- balance plus health/cooldown;
- full route explain impact including model/auth/affinity/overrides.

Recommendation:

- start with balance plus health/cooldown and include explicit overrides if the
  data is already available through route explain.

