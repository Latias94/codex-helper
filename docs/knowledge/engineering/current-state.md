---
type: "Current State"
title: "Current Engineering State"
description: "Short durable summary of the active engineering state."
tags: ["engineering-memory"]
timestamp: 2026-06-20T05:43:50Z
status: "active"
---

# Current State

- Goal: Reduce steady-state memory use in `codex-helper` TUI / dashboard, with emphasis on recent-request retention, forecast cache size, and branch cache defaults.
- Branch: `main`
- Last verified: `cargo fmt --all`, `cargo check -p codex-helper-core -p codex-helper-tui`, and focused `cargo nextest` passed.
- Done: `recent_finished_max()` now defaults to 1000 and clamps to `200..10000`; dashboard snapshot no longer forces `recent_limit` up to 2000; forecast tail default is now 1000 with clamp `200..10000`; forecast ledger cache stores shared `Arc` slices; forecast sample rows were trimmed further by removing the redundant `service` field.
- In progress: Evaluating whether `DashboardSnapshot.recent` / GUI runtime snapshots should move to shared ownership to remove the last deep-copy hotspot.
- Blocked: None
- Next action: If continuing the memory pass, switch the remaining `recent` holders to shared ownership (`Arc<Vec<FinishedRequest>>` or similar) and re-run focused tests.

# Citations
[1] [Cargo.toml](../../../Cargo.toml)
