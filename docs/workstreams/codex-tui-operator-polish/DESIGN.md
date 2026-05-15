# Design: Codex TUI Operator Polish

> 中文速览：TUI polish 的核心是“先保证关键事实不误导，再优化密度”。每个页面都要明确哪些字段永远不能被截断到失真，哪些字段可以折叠到详情，哪些动作应该放 footer，哪些动作应该放 help overlay。

## Product Contract

The TUI should answer the daily operator loop without requiring GUI, logs, or
raw config files:

1. What route am I currently using?
2. Is the selected route explicit, automatic, sticky, degraded, or exhausted?
3. Which providers are usable, stale, unknown, or failing balance refresh?
4. What action can I safely take from this page?
5. If the terminal is narrow, where did the omitted detail move?

The answer can be compact, but it must not be ambiguous.

## Page Contracts

### Usage / Balance

Primary facts:

- provider name;
- usage/cost summary;
- balance/quota status;
- balance amount or explicit unlimited/unknown/error state;
- freshness;
- route impact.

Interaction:

- `g` refreshes balances without blocking redraw;
- attention filters show rows with error, exhausted, stale, unknown, or low
  balance states;
- selected-provider detail can scroll independently when endpoint rows or error
  messages are long;
- the detail pane shows full balance strings when the table has to compact.

Narrow behavior:

- keep provider identity and status visible first;
- shorten secondary usage/cost details before hiding balance state;
- move full endpoint and latest error text into the detail pane;
- never show a partial currency string that looks like a real amount.

### Routing

Primary facts:

- current effective route target;
- whether the route target is global, session, automatic, or sticky;
- candidate order;
- skipped/demoted reasons;
- compact balance state only.

Interaction:

- route target overrides remain page-local actions;
- changing an override should invalidate the relevant route preview immediately;
- long candidate chains should be scrollable or folded with a visible count.

Narrow behavior:

- preserve source and selected target before candidate chains;
- use a short candidate summary in the table and full chain in details;
- avoid repeating "route target" labels when page context already explains it.

### Stations And Settings

Primary facts:

- station health;
- balance eligibility;
- runtime enable/disable state;
- retry/failover policy boundaries.

Interaction:

- row selection and viewport must stay synchronized after refresh, resize, and
  page switch;
- policy preview text should use the same vocabulary as routing docs.

## Layout Priorities

Display priority from highest to lowest:

1. active route/provider identity;
2. error/exhausted/stale state;
3. action required or next available shortcut;
4. balance amount and freshness;
5. usage/cost summaries;
6. long chains, URLs, and explanatory labels.

If a lower-priority item cannot fit, it should move to detail/help rather than
compress a higher-priority item into misleading text.

## Footer And Help Model

The footer should show only the actions that are:

- page-local;
- currently available;
- likely to be used in the operator loop.

Secondary actions should move to the help overlay. The help overlay should be
grouped by page and reuse the same labels as the footer.

Footer text must be display-width aware. If it cannot fit:

1. keep navigation and page-critical actions;
2. shorten labels before hiding keys;
3. expose omitted actions in help.

## View Model Rules

TUI rendering should not repeatedly reconstruct business rows inside multiple
render paths.

Preferred shape:

- core owns semantic facts;
- TUI owns page view models and selection/scroll state;
- render functions consume prepared rows and width hints;
- export/report paths reuse semantic rows, not terminal-truncated strings.

Avoid:

- deriving route/balance meaning from already-formatted text;
- storing truncated display strings in state;
- mutating selection or scroll state inside pure render helpers unless the
  function is explicitly a layout-state sync step.

## Render Test Strategy

Coverage should focus on layouts that are most likely to regress:

- long provider names;
- long route target chains;
- CJK labels and values;
- narrow terminal widths;
- zero balance, unlimited quota, unknown balance, stale balance, and balance
  refresh errors;
- footer overflow;
- selected detail panes with more rows than available height.

Snapshot-style tests do not need to assert every character of a full screen when
that would be brittle. They should assert invariants:

- critical labels remain visible;
- currency values are either complete or replaced by a clear state;
- no stale row remains after page switch/resize;
- footer fallback still exposes help;
- selected row and detail pane describe the same provider.

## Manual Verification

Before marking a TUI polish slice done, run a short manual smoke test:

1. open a normal-width terminal and inspect Usage, Routing, Stations, Settings;
2. resize to a narrow width and switch pages repeatedly;
3. use long provider names and at least one CJK provider label;
4. trigger balance refresh success and failure;
5. scroll provider and endpoint details;
6. toggle global/session route override and confirm the route preview updates.

## Non-goals

- Do not make TUI duplicate GUI charts.
- Do not hide core uncertainty by choosing optimistic labels.
- Do not put every action into the footer.
- Do not change route or balance semantics to make a layout easier.
