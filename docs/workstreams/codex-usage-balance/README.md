# Workstream: Codex Usage / Balance

> 中文速览：本目录定义一等 `Usage / Balance` 决策页面。目标不是再堆一个旧 `Stats` 页，而是把 provider 用量、成本、余额/配额、刷新状态、路由影响和导出报告统一成用户判断“现在该走谁、谁快耗尽、谁不稳定”的可信页面。

## Purpose

This workstream upgrades the existing Stats capability into a first-class
operator decision surface for usage and balance.

The goal is to let a local proxy user answer these questions quickly:

1. Which provider or endpoint is being used most?
2. Which provider is running out of balance or quota?
3. Which provider is failing, stale, unknown, or excluded from routing?
4. How much did this window cost?
5. Did the routing policy select fallback because of balance, health,
   unsupported model, explicit override, or affinity?

## Problem

`codex-helper` already has several pieces of the answer:

- request usage rollups;
- provider balance snapshots;
- route graph explain output;
- TUI/GUI stats pages;
- provider balance refresh controls in some places.

The current product shape is still fragmented:

- the route page shows only compact balance context;
- the Stats page is mostly request scorecard oriented;
- GUI and TUI can drift if they compute display rows separately;
- refresh status and failed balance lookups are not consistently visible;
- users cannot easily see whether balance state changed routing behavior.

This makes common local proxy decisions slower than they should be.

## Target Outcome

- A single core view model powers TUI, GUI, and report export.
- The existing Stats page becomes or feeds `Usage / Balance`; it is not
  duplicated as a competing page.
- Provider and endpoint rows show usage, cost, balance/quota, refresh status,
  health, and routing impact in one place.
- The route page keeps short route context only; detailed usage and balance
  analysis lives here.
- Balance refresh failures are visible but do not block other refreshes or the
  TUI render loop.
- Exported reports use the same rows and wording as the UI.

## Document Map

- `DESIGN.md`
  - target view model, data sources, page layout, refresh semantics, and
    TUI/GUI/report parity rules.
- `TODO.md`
  - implementation checklist split into small reviewable packages.
- `MILESTONES.md`
  - phased delivery gates and acceptance criteria.
- `DECISIONS.md`
  - product and architecture decisions that should stay stable during the
    implementation.

## Scope

In scope:

- usage and cost aggregation display;
- provider and endpoint balance/quota display;
- balance refresh status and error surfaces;
- route graph impact summaries;
- TUI Usage / Balance page;
- GUI Usage / Balance page;
- report export;
- user documentation and changelog wording;
- tests for stale, unknown, exhausted, failed, and refreshed states.

Out of scope:

- changing the route selection algorithm;
- redesigning the persisted route graph schema;
- turning codex-helper into a general provider marketplace;
- scraping every vendor-specific billing portal before the generic adapter
  model is stable;
- long-term database storage beyond the existing request/balance state unless a
  separate ledger workstream accepts that scope.

## Relationship To Routing Workstreams

The routing graph and preference runtime workstreams define how requests are
selected. This workstream explains the observable result:

- what was used;
- what it cost;
- what remains;
- what routing skipped or demoted;
- which data is fresh enough to trust.

It must consume route explain output and runtime state, not reimplement routing
selection.

## Working Principle

Usage and balance are decision data, not decoration.

The page is successful when a user can look at it for a few seconds and decide
whether to keep the current routing policy, refresh balances, pin a provider,
change a route target, or investigate a failing relay.

