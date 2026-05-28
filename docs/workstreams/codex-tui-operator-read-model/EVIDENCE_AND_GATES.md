# Codex TUI Operator Read Model Refactor - Evidence And Gates

Status: Active
Last updated: 2026-05-28

## Gate Set

```bash
cargo fmt --check
cargo nextest run -p codex-helper-core -p codex-helper-tui --no-fail-fast
cargo check -p codex-helper-tui
git diff --check
```

Use narrower test filters during iteration, but record the package-level
core/TUI gate before claiming a slice is complete.

## Evidence Anchors

- `crates/core/src/dashboard_core/station_options.rs`
- `crates/core/src/dashboard_core/types.rs`
- `crates/tui/src/tui/model.rs`
- `docs/workstreams/codex-tui-operator-read-model/TODO.md`

## Latest Automated Evidence

- 2026-05-28 `cargo check -p codex-helper-tui`: passed.
- 2026-05-28 `cargo fmt --all`: completed before verification.
- 2026-05-28 `cargo nextest run -p codex-helper-core build_runtime_provider_options_from_mgr_owns_tui_station_metadata --no-fail-fast`: passed, 1 test.
- 2026-05-28 `cargo nextest run -p codex-helper-core -p codex-helper-tui --no-fail-fast`: passed, 890 tests.
- 2026-05-28 `cargo fmt --check`: passed.
- 2026-05-28 `cargo check -p codex-helper-tui`: passed.
- 2026-05-28 `git diff --check`: passed.

## Notes

This lane intentionally starts with the runtime `ServiceConfigManager` path
because the TUI still receives legacy runtime config. Provider-catalog
`ServiceViewV2` DTOs remain in core and should converge in a later slice.
