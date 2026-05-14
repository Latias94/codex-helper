# Workstream: Codex Routing Preference Runtime

## Purpose

This workstream defines the next breaking routing runtime after the v4 route
graph work, including the retirement of station as a runtime compatibility
surface.

The goal is to make user preference the strongest routing signal and make
automatic stickiness a scoped optimization instead of a hidden override.

## Problem

The v4 route graph preserves authoring intent, but runtime behavior can still
surprise users when fallback succeeds.

Example:

1. A route is configured as monthly first.
2. Monthly candidates temporarily return `502` or `429`.
3. The request falls back to a paygo relay such as `chili`.
4. The successful fallback is recorded as session affinity.
5. Later requests in that session continue selecting the fallback provider even
   when monthly providers are usable again.

That behavior is useful for cache locality, but it violates the common local
proxy expectation: "try my preferred cost class first, then use fallback only
when needed."

## Target Outcome

- v5 route execution is driven by route candidates and preference groups, not
  by a synthetic compatibility station.
- Preference groups are explicit runtime semantics.
- Automatic affinity is scoped to the selected preference group by default.
- Fallback affinity is temporary and must not permanently outrank preferred
  candidates.
- Logs and explain output can answer why a fallback was selected and why it was
  or was not reused.
- Station data is migration-only. New runtime state, new route selection, new
  affinity, and new public APIs do not consume station identity.

## Document Map

- `DESIGN.md`
  - target runtime model, selection algorithm, affinity policy, and migration
    stance.
- `FEARLESS_REFACTOR.md`
  - deletion candidates, station retirement boundaries, and design rules.
- `TODO.md`
  - implementation checklist split into small reviewable work packages.
- `MILESTONES.md`
  - phased delivery gates and acceptance criteria.
- `COMPLETION_AUDIT.md`
  - strict prompt-to-artifact checklist, accepted defaults, test evidence, and
    remaining closure gaps.

## Scope

In scope:

- route candidate execution;
- preference group derivation;
- session affinity semantics;
- fallback affinity TTL and preferred reprobe;
- provider endpoint keyed runtime state;
- request logs and control trace;
- CLI/TUI/GUI explain output;
- config migration if new persisted syntax is required.
- station-to-route migration and public station API retirement.

Out of scope:

- pricing math;
- model price catalog redesign;
- provider account balance scraping internals;
- Codex session transcript storage;
- unrelated GUI layout work.

## Breaking Change Stance

A persisted config and public API breaking change is acceptable if it removes
ambiguous runtime behavior.

The expected migration posture is:

- keep loading v4 configs and older station-shaped configs only as migration
  inputs;
- emit a deterministic migration into the next schema;
- preserve provider definitions and route graph topology;
- default old v4 configs to `affinity_policy = "preferred-group"` unless the
  operator explicitly opts into old fallback-sticky behavior;
- reject new station writes and remove station identity from new APIs;
- document behavior changes even when the TOML schema does not need to change.

## Working Principle

User intent wins over runtime convenience.

Affinity may improve cache locality and reduce churn, but it must not silently
turn a fallback path into the effective default path.

Compatibility artifacts must not survive as architecture. Station is an input
format to migrate away from, not an internal runtime model to preserve.
