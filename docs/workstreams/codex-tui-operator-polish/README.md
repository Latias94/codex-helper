# Workstream: Codex TUI Operator Polish

> 中文速览：本目录专门收敛 TUI 日常操作体验问题。目标不是重新设计路由或余额语义，而是让用户在窄终端、中文文本、长 provider 名、余额/套餐文本、快捷键帮助和滚动列表下都能稳定判断当前代理状态并完成操作。

## Purpose

This workstream turns the TUI into a dependable daily operator console for the
local proxy.

The routing and usage/balance workstreams define the facts. This workstream
defines how those facts should be rendered and operated in a terminal:

1. no misleading truncation for important balance and route information;
2. predictable keyboard behavior across Usage, Routing, Stations, and Settings;
3. compact but discoverable help text;
4. width-aware layouts for long provider names, CJK text, and narrow terminals;
5. shared TUI view models so render, selection, help text, and exports do not
   drift.

## Problem

Recent refactors made routing, balances, usage, and request observability much
more capable. The remaining risk is user trust in the terminal surface:

- compact route and balance summaries can still hide the part users need;
- provider names, route target strings, and balance texts can compete for the
  same narrow columns;
- detail panes need consistent scrolling when endpoint rows or errors are long;
- footer shortcuts can disappear or become noisy when pages gain more actions;
- render-time formatting is still easy to duplicate across pages;
- regression coverage for terminal width, CJK width, and long labels is not yet
  strong enough.

These are product issues, not cosmetic issues. A local proxy user must be able
to trust what the TUI says before changing routing policy or deciding whether a
provider is exhausted.

## Target Outcome

- TUI pages prioritize operator-critical facts before secondary decoration.
- Usage and Routing pages have explicit narrow-width behavior.
- Long provider names, route chains, endpoint names, and balance strings degrade
  predictably.
- Page-specific actions stay discoverable without forcing every shortcut into a
  single crowded footer line.
- Selection, scroll offset, detail pane content, and refresh status update from
  one coherent page state.
- Snapshot-style render tests cover the layouts that previously produced
  truncation or stale display issues.

## Document Map

- `DESIGN.md`
  - page contracts, layout priorities, interaction model, render constraints,
    and test strategy.
- `TODO.md`
  - implementation checklist split into reviewable tasks.
- `MILESTONES.md`
  - phased execution plan and acceptance gates.
- `DECISIONS.md`
  - product decisions that should stay stable while polishing the TUI.

## Scope

In scope:

- TUI Usage / Balance page polish;
- TUI Routing page polish;
- TUI footer/help behavior;
- detail pane scrolling and selection state;
- narrow terminal and CJK display-width handling;
- render/view-model cleanup inside TUI;
- TUI regression tests for truncation, stale rows, and key help;
- documentation notes when a shortcut or page behavior changes.

Out of scope:

- changing route selection semantics;
- changing balance adapter semantics;
- redesigning persisted configuration;
- adding a GUI-only usage dashboard;
- building a full terminal layout engine beyond the needs of this product.

## Relationship To Other Workstreams

`codex-usage-balance` owns the core usage/balance view model and balance
semantics.

`codex-routing-graph-refactor` and `codex-routing-preference-runtime-refactor`
own route selection and preference semantics.

`codex-operator-experience-refactor` remains the broad product umbrella. This
workstream is the focused execution track for the TUI polish that is too narrow
to belong in that umbrella and too broad to stay inside Usage / Balance.

## Working Principle

Terminal UI is an operational control surface.

When space is limited, preserve the facts that affect operator decisions:
current route, provider identity, balance status, freshness, errors, and the
action needed next. Secondary labels and explanations can move into details,
help overlays, or exports.
