# Design: Codex Usage / Balance

## Background

The current UI has request statistics, provider balance snapshots, and route
details, but they are spread across pages. The new surface should treat usage
and balance as one operational decision model.

This is not a pure analytics dashboard. It is a local proxy control surface for
active decisions.

## Target Page Identity

Preferred name:

```text
Usage / Balance
```

Acceptable short labels:

- `Usage`
- `Balance`
- `Stats` only as a compatibility label during transition

The existing Stats page should evolve into this page. A second overlapping page
would make the product harder to understand.

## Data Sources

The core view model should combine these existing sources:

- request usage rollup;
- request ledger summary;
- active and finished request observations;
- provider balance snapshots;
- provider endpoint runtime state;
- health and cooldown state;
- route graph explain output;
- pricing catalog and cost estimates;
- last balance refresh result.

The UI must not perform independent business aggregation beyond local
formatting, sorting, filtering, and viewport state.

## Core View Model

Introduce a canonical read model such as:

```text
UsageBalanceView
  service_name
  window
  generated_at_ms
  totals
  provider_rows[]
  endpoint_rows[]
  routing_impacts[]
  refresh_status
  warnings[]
```

The exact Rust names can change, but the ownership should not: core owns the
semantic rows, UI owns rendering.

### Totals

Totals should include:

- request count;
- success rate;
- error count;
- input tokens;
- cached input tokens;
- output tokens;
- reasoning output tokens when available;
- estimated cost;
- balance snapshot counts by status.

### Provider Row

Each provider row should include:

- provider id;
- display name and alias;
- tags relevant to routing, especially `billing`;
- current routing role;
- enabled/disabled state;
- request count;
- success rate;
- average time to first byte;
- output tokens per second;
- token totals;
- estimated cost;
- primary balance/quota summary;
- balance status: ok, exhausted, stale, unknown, error;
- last balance refresh age;
- latest balance lookup error when present;
- route impact summary.

### Endpoint Row

Endpoint rows should be available in a detail panel or expanded view:

- provider id;
- endpoint id;
- base URL host summary;
- enabled/disabled state;
- tags;
- supported model hints;
- request count and cost;
- runtime health/cooldown;
- balance snapshot bound to the endpoint if known.

### Routing Impact

The view should summarize why usage is moving away from preferred providers:

- trusted balance exhaustion;
- stale balance data;
- unknown balance data;
- health cooldown;
- unsupported model;
- missing auth;
- explicit session/global route target;
- affinity;
- manual pin.

Routing impact is explanatory. It must not silently rewrite route behavior.

## Page Layout

### TUI

The TUI page should stay dense and keyboard friendly:

- top summary band: totals, cost, balance status counts, refresh status;
- main table: provider rows;
- detail panel: selected provider endpoint rows and recent errors;
- optional routing impact section;
- footer shortcuts:
  - `d` cycle window;
  - `Tab` switch table/detail focus if needed;
  - `g` refresh balances;
  - `y` export report;
  - `e` show errors only;
  - `?` help.

The route page should continue showing only compact balance summaries and
route-focused controls.

### GUI

The GUI should use the same core rows:

- overview strip;
- provider table with sortable columns;
- selected provider details;
- refresh button with status;
- report export action;
- clear stale/error explanations.

GUI can expose more columns than TUI, but it must not invent different
semantics.

### Report Export

The report should use the same view model:

- machine-readable JSON export for debugging;
- human-readable Markdown or text report for issue reports;
- include window, generated time, provider rows, endpoint rows, and routing
  impacts.

## Balance Refresh Semantics

Balance refresh is advisory and must not block the UI.

Rules:

- one provider failure must not cancel other provider refreshes;
- a refresh failure must not interrupt snapshot refresh or TUI redraw;
- stale/error/unknown states should remain visible;
- repeated user refreshes should deduplicate or show "refresh in progress";
- route graph pages should resync route explain after refresh results land.

## Sorting And Filtering

Default sort should prioritize decision urgency:

1. routing-selected or active providers;
2. exhausted/error/stale/unknown balance states;
3. highest recent cost or request count;
4. provider name for stable tie-breaking.

Filters should be simple:

- all;
- errors only;
- balance attention;
- active route only;
- monthly/paygo tag.

## Compatibility

Legacy station-shaped data may appear in historical logs, but the usage view
should prefer provider and endpoint identity.

If a value is only available through legacy station compatibility metadata, the
UI may show it as legacy context, not as the primary grouping key.

## Non-Goals

- Do not add a new persisted config version only for this page.
- Do not make TUI and GUI maintain separate aggregation implementations.
- Do not hide unknown balance data as "ok".
- Do not treat zero balance, unknown balance, and unlimited quota as the same
  state.

