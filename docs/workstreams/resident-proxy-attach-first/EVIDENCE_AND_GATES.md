# Resident Proxy And Attach-First Operator Consoles — Evidence And Gates

Status: Closed
Last updated: 2026-05-20

## Smallest Current Repro

Current architecture couples the interactive TUI and proxy runtime in `src/cli_app.rs::run_server`.
The first proof should show that the same proxy/admin runtime can be started under an explicit
resident lifetime policy without breaking the existing ephemeral path.

## Recent Verification

- `cargo fmt --check` ✅
- `cargo check -p codex-helper-tui` ✅
- `cargo check -p codex-helper-gui` ✅
- `cargo check -p codex-helper` ✅
- `cargo check -p codex-helper -p codex-helper-gui` ✅
- `cargo check --workspace` ✅
- `cargo nextest run -p codex-helper-tui --no-fail-fast` ✅
- `cargo nextest run -p codex-helper-core proxy::tests::api_admin --no-fail-fast` ✅
- `cargo nextest run -p codex-helper-gui --no-fail-fast` ✅
- `cargo nextest run -p codex-helper cli_types listener_bind_help_tests supervisor_tests --no-fail-fast` ✅

Note: an initial TUI nextest attempt exposed stale test fixtures missing the
`session_identity_source` field after the session identity model changed; fixtures were updated and
the rerun passed.

## Implementation Evidence

- TUI attach path: `codex-helper tui --codex/--claude` resolves the service/port and uses the
  local admin API to render a read-only dashboard from `/runtime/status`, `/snapshot`, `/profiles`,
  and route metadata when available.
- Safe TUI exit: attached mode only sets `ui.should_exit`; it never sends the runtime shutdown
  signal or calls the runtime shutdown API. Settings/header/footer copy says `q` exits the console
  only.
- Supervisor crash markers: `daemon supervise` records unexpected resident child exits under
  `~/.codex-helper/run/<service>-<port>.supervisor-crash.json` and clears the marker on clean exit.
- GUI status hints: attached runtime summary surfaces whether the remote shutdown API is available
  and explains close/stop behavior for attached resident proxies.
- Closeout review: DESIGN/TODO/MILESTONES/HANDOFF/WORKSTREAM were checked against the final diff.
  No blocking workstream or code-quality findings remain for this lane.

## Deferred Follow-ons

- Full Windows Service / launchd / systemd installation: intentionally out of scope for this lane.
- Richer attached-TUI write controls: current attached TUI is read-only by design to preserve safe
  lifetime semantics.
- Full `cargo nextest run --workspace -j 4 --no-fail-fast`: skipped to avoid unnecessary
  machine-wide CPU/memory pressure during a stability-focused lane; package and targeted gates above
  cover the changed crates and public seams.

```powershell
cargo check -p codex-helper
cargo nextest run -p codex-helper-core proxy::tests::api_admin --no-fail-fast
```

## Gate Set

### Targeted Iteration Gate

Use the narrowest gate matching the touched slice:

```powershell
cargo check -p codex-helper
cargo nextest run -p codex-helper-core proxy::tests::api_admin --no-fail-fast
```

GUI attach/resident changes:

```powershell
cargo nextest run -p codex-helper-gui --no-fail-fast
cargo check -p codex-helper-gui
```

TUI attach changes:

```powershell
cargo nextest run -p codex-helper-tui --no-fail-fast
cargo check -p codex-helper
```

### Package Gate

```powershell
cargo fmt --check
cargo clippy -p codex-helper-core --all-targets -- -D warnings
cargo nextest run -p codex-helper-core -j 4 --no-fail-fast
cargo nextest run -p codex-helper-gui -j 4 --no-fail-fast
cargo nextest run -p codex-helper-tui -j 4 --no-fail-fast
```

### Broader Closeout Gate

```powershell
cargo fmt --check
cargo check --workspace
cargo nextest run --workspace -j 4 --no-fail-fast
```

Use `-j 4` by default for local verification to avoid creating unrelated machine-wide memory
pressure during a lane whose purpose is long-running stability.

### Review Gate

Run `review-workstream` before accepting task or lane completion. Record blocking findings, missing
gates, and residual risks here or link to the review note.

## Evidence Anchors

- `docs/workstreams/resident-proxy-attach-first/DESIGN.md`
- `docs/workstreams/resident-proxy-attach-first/TODO.md`
- `docs/workstreams/resident-proxy-attach-first/MILESTONES.md`
- `src/cli_app.rs`
- `src/cli_types.rs`
- `crates/gui/src/gui/proxy_control/`
- `crates/tui/src/tui/`
- `crates/core/src/proxy/control_plane*`

## Notes

Fresh verification is required before marking a task, Codex goal, or lane complete.
