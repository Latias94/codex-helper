# Codex TUI Operator Polish - Evidence And Gates

Status: Active
Last updated: 2026-05-28

## Smallest Current Repro

```bash
cargo nextest run -p codex-helper-tui stats requests recent history help chrome --no-fail-fast
```

## Width Contract

- Normal smoke width: 110 columns.
- Minimum usable full-page width: 76 columns.

## Gate Set

### Targeted Iteration Gate

```bash
cargo nextest run -p codex-helper-tui stats requests recent history help chrome --no-fail-fast
```

### Package Gate

```bash
cargo nextest run -p codex-helper-tui --no-fail-fast
```

### Broader Closeout Gate

```bash
cargo check -p codex-helper-tui
```

Use a workspace-wide closeout only if the slice touches shared core contracts or if the TUI package gate is insufficient.

### Review Gate

Run `review-workstream` before accepting lane completion. Record blocking findings, remaining render-state duplication, and any width-related caveats here or in HANDOFF.md.

## Evidence Anchors

- `docs/workstreams/codex-tui-operator-polish/DESIGN.md`
- `docs/workstreams/codex-tui-operator-polish/TODO.md`
- `docs/workstreams/codex-tui-operator-polish/MILESTONES.md`
- `docs/workstreams/codex-tui-operator-polish/SMOKE.md`
- `docs/workstreams/codex-tui-operator-polish/DECISIONS.md`
- code and tests under `crates/tui/src/tui/`

## Latest Automated Evidence

- 2026-05-28 `cargo fmt --check`: passed.
- 2026-05-28 `cargo nextest run -p codex-helper-tui stats requests recent history help chrome --no-fail-fast`: passed, 41 tests.
- 2026-05-28 `cargo nextest run -p codex-helper-tui --no-fail-fast`: passed, 140 tests.
- 2026-05-28 `cargo check -p codex-helper-tui`: passed.
- 2026-05-28 `git diff --check`: passed.

## Notes

Manual smoke remains required for the narrow-width terminal claim. Automated tests cover render invariants and state sync, but not every emulator quirk.
