# Codex Relay Live Smoke Diagnostics — Evidence And Gates

Status: Active
Last updated: 2026-05-19

## Smallest Current Repro

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
```

This should prove the core live-smoke contract, request builders, classifiers, opt-in guard, and one-request behavior.

## Gate Set

### Core Gate

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
```

### Admin API Gate

```bash
cargo nextest run -p codex-helper-core codex_live_smoke_api
```

### TUI Gate

```bash
cargo nextest run -p codex-helper-tui codex_relay_live_smoke
```

### Format Gate

```bash
cargo fmt --check
```

### Closeout Package Gates

Run when core/TUI contracts changed:

```bash
cargo nextest run -p codex-helper-core
cargo nextest run -p codex-helper-tui
```

## Evidence Anchors

- `docs/workstreams/codex-relay-live-smoke-diagnostics/DESIGN.md`
- `crates/core/src/proxy/codex_relay_live_smoke.rs`
- `crates/core/src/proxy/control_plane/codex_live_smoke.rs`
- `crates/tui/src/tui/view/pages/settings.rs`
- `docs/CONFIGURATION.md`
- `docs/CONFIGURATION.zh.md`

## Recorded Evidence

### 2026-05-19 — RLS-020 Core Live Smoke Contract

Command:

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
```

Result: PASS; 7 tests passed, 528 skipped.

Proves:

- default live-smoke cases exclude hosted image generation,
- missing acknowledgement fails before upstream IO,
- compact live smoke sends one Codex-shaped `/responses/compact` request,
- upstream bearer and `x-api-key` auth are forwarded,
- hosted image smoke sends Codex-shaped `image_generation` tool JSON to `/responses`,
- SSE `image_generation_call` output is classified without retaining raw image bytes,
- upstream `model_mapping` is applied before live smoke calls.

Files:

- `crates/core/src/proxy/codex_relay_live_smoke.rs`
- `crates/core/src/proxy/codex_relay_target.rs`
- `crates/core/src/proxy/service_core.rs`

## Notes

Live relay smoke against a real paid upstream is optional evidence unless the user explicitly requests it. Automated tests must use local fake upstreams.
