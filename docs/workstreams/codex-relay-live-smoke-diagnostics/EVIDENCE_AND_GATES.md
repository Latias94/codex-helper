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

### 2026-05-19 — RLS-030 Admin API Surface

Command:

```bash
cargo nextest run -p codex-helper-core codex_live_smoke_api
```

Result: PASS; 3 tests passed, 535 skipped.

Proves:

- `POST /__codex_helper/api/v1/codex/relay-live-smoke` is listed in API v1 endpoints,
- `surface_capabilities.codex_relay_live_smoke` is advertised,
- operator summary links include `codex_relay_live_smoke`,
- missing acknowledgement returns `400` before upstream IO,
- an acknowledged compact live smoke request reaches one selected upstream and returns a summarized result.

Files:

- `crates/core/src/proxy/control_plane/codex_live_smoke.rs`
- `crates/core/src/proxy/control_plane_manifest.rs`
- `crates/core/src/proxy/control_plane_routes/capability_session.rs`
- `crates/core/src/dashboard_core/types.rs`
- `crates/core/src/dashboard_core/operator_summary.rs`

### 2026-05-19 — RLS-030 Regression Gates

Commands:

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
cargo fmt --check
```

Result: PASS; core live-smoke tests passed again (7 passed, 531 skipped) and formatting check passed.

### 2026-05-19 — RLS-040 TUI Operator Flow

Command:

```bash
cargo nextest run -p codex-helper-tui codex_relay_live_smoke
```

Result: PASS; 3 tests passed, 117 skipped.

Proves:

- live smoke confirmation expires after 3 seconds and is mode-specific,
- one `X` key press on Settings only arms compact-only confirmation and does not start a live request,
- Settings renders live smoke confirmation, compact result, hosted image-generation result, and warnings.

Files:

- `crates/tui/src/tui/codex_relay_live_smoke.rs`
- `crates/tui/src/tui/input/normal.rs`
- `crates/tui/src/tui/view/pages/settings.rs`
- `crates/tui/src/tui/state.rs`

### 2026-05-19 — RLS-040 Regression Gates

Commands:

```bash
cargo nextest run -p codex-helper-core codex_live_smoke_api
cargo nextest run -p codex-helper-core codex_relay_live_smoke
cargo fmt --check
```

Result: PASS; admin API tests passed (3 passed, 535 skipped), core live-smoke tests passed (7 passed, 531 skipped), and formatting check passed.

## Notes

Live relay smoke against a real paid upstream is optional evidence unless the user explicitly requests it. Automated tests must use local fake upstreams.
