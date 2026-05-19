# Codex Relay Diagnostics Operator Surface — Evidence And Gates

Status: Implemented
Last updated: 2026-05-19

## Smallest Current Repro

```bash
cargo nextest run -p codex-helper-tui codex_relay_diagnostics
```

This should prove the TUI view/state/action path for the operator-facing diagnostic.

## Gate Set

### Core Contract Gate

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Proves the HTTP admin API remains compatible after moving the diagnostic into a reusable service method.

### TUI Operator Surface Gate

```bash
cargo nextest run -p codex-helper-tui codex_relay_diagnostics
```

Proves the Settings page can render and update the operator-facing diagnostic state.

### Format Gate

```bash
cargo fmt --check
```

Proves Rust formatting did not drift.

### Broader Closeout Gate

If shared core contracts change in a way that may affect other packages, run:

```bash
cargo nextest run -p codex-helper-core
cargo nextest run -p codex-helper-tui
```

Use the narrower targeted gates if package gates are too slow, and record the reason.

### Review Gate

Run review before accepting lane completion. The review should check:

- no periodic active upstream probing,
- no automatic patch-mode mutation,
- DTOs are not duplicated between HTTP and TUI,
- unknown hosted image generation remains explicitly uncertain.

## Evidence Anchors

- `docs/workstreams/codex-relay-diagnostics-operator-surface/DESIGN.md`
- `docs/workstreams/codex-relay-diagnostics-operator-surface/TODO.md`
- `crates/core/src/codex_capability_profile.rs`
- `crates/core/src/proxy/control_plane/codex_capabilities.rs`
- `crates/tui/src/tui/view/pages/settings.rs`

## Recorded Evidence

### 2026-05-19 - RDO-020 reusable core service

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Result: passed, 2 tests. Proves the admin API still returns expected/observed/mismatches and still infers patch mode from current Codex switch state after routing through `ProxyService::codex_relay_capabilities`.

### 2026-05-19 - RDO-030 TUI diagnostic surface

```bash
cargo nextest run -p codex-helper-tui codex_relay_diagnostics
```

Result: passed, 4 tests. Proves the TUI chooses a useful model hint for diagnostics and renders observed endpoint support, mismatches, warnings, and patch-mode recommendation.

### 2026-05-19 - RDO-040 documentation

```bash
rg "relay diagnostics|能力诊断|Settings" docs/CONFIGURATION.md docs/CONFIGURATION.zh.md CHANGELOG.md -n
```

Result: passed. Proves the TUI path is documented alongside the admin API path.

### 2026-05-19 - Final format gate

```bash
cargo fmt --check
```

Result: passed. Proves Rust formatting did not drift after core, TUI, and documentation updates.

### 2026-05-19 - RDO-050 package closeout gates

```bash
cargo nextest run -p codex-helper-tui
```

Result: passed, 117 tests. Proves the TUI package remains healthy after adding the Settings shortcut, async diagnostic state, rendering, and i18n/footer copy.

```bash
cargo nextest run -p codex-helper-core
```

Result: passed, 528 tests. Proves the reusable core diagnostic service and HTTP adapter extraction did not regress the broader core contract.

Closeout review: no blocking workstream or code-quality findings against the TUI-first operator surface. Residual risks are explicitly deferred follow-ons: GUI/CLI surfaces still use the admin API path, and no live relay smoke was run.

## Notes

Fresh verification was recorded before marking the Codex goal and workstream complete.
