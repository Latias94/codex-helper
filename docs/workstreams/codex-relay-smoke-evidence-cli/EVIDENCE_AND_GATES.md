# Codex Relay Smoke Evidence CLI — Evidence And Gates

Status: Complete
Last updated: 2026-05-19

## Smallest Current Repro

```bash
cargo nextest run -p codex-helper-core codex_relay_evidence
```

## Gate Set

### Core Evidence Gate

```bash
cargo nextest run -p codex-helper-core codex_relay_evidence
```

### CLI Gate

```bash
cargo nextest run -p codex-helper codex_relay_cli
```

### Existing Relay Regression Gates

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
cargo nextest run -p codex-helper-core codex_relay_probe
cargo nextest run -p codex-helper-core codex_live_smoke_api
```

### Format Gate

```bash
cargo fmt --check
```

## Evidence Anchors

- `crates/core/src/proxy/codex_relay_evidence.rs`
- `crates/core/src/proxy/codex_relay_capabilities.rs`
- `crates/core/src/proxy/codex_relay_live_smoke.rs`
- `src/cli_types.rs`
- `src/commands/codex.rs`
- `docs/CONFIGURATION.md`
- `docs/CONFIGURATION.zh.md`

## Recorded Evidence

### 2026-05-19 — RSE-020 Core Evidence Store

Command:

```bash
cargo nextest run -p codex-helper-core codex_relay_evidence
```

Result: PASS; 3 tests passed, 544 skipped.

Proves:

- evidence entries append and read newest-first,
- evidence filters match kind, station substring, and model substring,
- missing evidence files return an empty list instead of an error.

Files:

- `crates/core/src/proxy/codex_relay_evidence.rs`
- `crates/core/src/proxy/codex_relay_capabilities.rs`
- `crates/core/src/proxy/codex_relay_live_smoke.rs`

### 2026-05-19 — RSE-030 CLI Operator Surface

Command:

```bash
cargo nextest run -p codex-helper codex_relay_cli
cargo run -q --bin codex-helper -- codex relay-evidence --limit 1 --json
```

Result: PASS; 4 tests passed, 16 skipped. The runtime evidence-list command completed and printed
`[]` in the current environment.

Proves:

- `codex-helper codex relay-capabilities` parses model, patch mode, and JSON output flags,
- `codex-helper codex relay-live-smoke` requires an acknowledgement argument,
- live smoke parses the explicit hosted image flag,
- `codex-helper codex relay-evidence` parses kind/station/limit filters.

Files:

- `src/cli_types.rs`
- `src/cli_app.rs`
- `src/commands/codex.rs`

### 2026-05-19 — RSE-040 Regression And Package Gates

Commands:

```bash
cargo nextest run -p codex-helper-core codex_relay_live_smoke
cargo nextest run -p codex-helper-core codex_relay_probe
cargo nextest run -p codex-helper-core codex_live_smoke_api
cargo nextest run -p codex-helper-core
cargo nextest run -p codex-helper
cargo fmt --check
```

Result: PASS.

- Core live-smoke targeted gate: 7 tests passed, 540 skipped.
- Core relay-probe targeted gate: 10 tests passed, 537 skipped.
- Admin live-smoke API targeted gate: 3 tests passed, 544 skipped.
- Core package gate: 547 tests passed, 0 skipped.
- Root CLI package gate: 20 tests passed, 0 skipped.
- Formatting check passed.

Proves:

- evidence append does not break live-smoke safety or API behavior,
- validation-only probe behavior remains unchanged,
- the new CLI and core exports compile through package-level tests,
- docs/changelog were updated after implementation.

## Notes

Real paid relay live smoke remains manual evidence. Automated tests must use fake upstreams or
argument-level tests.
