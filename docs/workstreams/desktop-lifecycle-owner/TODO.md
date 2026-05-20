# Desktop Lifecycle Owner — TODO

Status: Complete
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

- [x] DLO-010 [owner=planner] [deps=none] [scope=docs/workstreams/desktop-lifecycle-owner]
  Goal: Freeze problem, target state, non-goals, architecture direction, and evidence anchors.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json agree.
  Evidence: docs/workstreams/desktop-lifecycle-owner/DESIGN.md
  Handoff: DONE — workstream opened and scoped on 2026-05-20.

## M1 — Lifecycle Domain And Owner Metadata

- [x] DLO-020 [owner=main] [deps=DLO-010] [scope=crates/core/src/runtime_manager.rs,crates/core/src/lib.rs]
  Goal: Add first-class lifecycle mode and owner metadata types with parsing/display helpers.
  Validation: cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast
  Review: Ensure terminology is explicit: EphemeralConsole, AttachedObserver, ResidentDaemon, DesktopOwned.
  Evidence: crates/core/src/runtime_manager.rs tests.
  Handoff: DONE — added core lifecycle mode, owner kind, owner marker model, marker path/read/write/clear helpers, and focused tests.

- [x] DLO-030 [owner=main] [deps=DLO-020] [scope=crates/core/src/runtime_manager.rs,src/cli_app.rs]
  Goal: Add owner marker read/write/clear helpers under `~/.codex-helper/run/` and have resident/supervisor flows write owner metadata.
  Validation: cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast; cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast
  Review: Marker files must be best-effort and never block proxy shutdown on cleanup failure.
  Evidence: owner marker tests and daemon/supervisor CLI tests.
  Handoff: DONE_WITH_CONCERNS — manual resident, supervisor, and hidden desktop-managed owner kinds are representable; marker writes/clears are best-effort. Concern: no live process integration test for `serve --resident` marker due to needing real configured upstream/listeners.

## M2 — Manager Seam And Adapter Convergence

- [x] DLO-040 [owner=main] [deps=DLO-030] [scope=crates/core/src/runtime_manager.rs,crates/gui/src/gui/proxy_control.rs,crates/gui/src/gui/proxy_control/runtime_lifecycle.rs]
  Goal: Introduce a `RuntimeManager`-style seam for stop/detach/shutdown decisions and migrate GUI owned-vs-attached stop logic onto it.
  Validation: cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast
  Review: GUI close/default exit must stop only GUI-owned runtime; attached exit must detach without remote shutdown.
  Evidence: GUI proxy_control lifecycle tests.
  Handoff: DONE — added core stop-decision API and migrated GUI stop/stop_owned policy through it; UI keeps explicit remote stop while normal owner exit detaches attached proxies only.

- [x] DLO-050 [owner=main] [deps=DLO-040] [scope=src/cli_app.rs,src/cli_types.rs,crates/tui/src/tui/attached.rs]
  Goal: Align CLI daemon/status/stop and attached TUI copy/behavior with owner metadata and lifecycle modes.
  Validation: cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast; cargo nextest run -p codex-helper-tui attached --no-fail-fast
  Review: `daemon status` should surface owner where available; `daemon stop` remains explicit stop.
  Evidence: CLI/TUI targeted tests.
  Handoff: DONE — `daemon status` reads owner markers best-effort, supervisor children keep supervisor ownership via hidden `--supervisor-managed`, and attached TUI/help text now says observer exit only detaches the console.

## M3 — Desktop-Managed Sidecar Preparation

- [x] DLO-060 [owner=main] [deps=DLO-050] [scope=src/cli_types.rs,src/cli_app.rs,docs]
  Goal: Add explicit desktop-managed resident child semantics suitable for a future tray/Tauri backend, without making it default.
  Validation: cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast
  Review: The CLI surface may be hidden/advanced, but owner marker must identify DesktopOwned.
  Evidence: CLI parse/owner marker tests.
  Handoff: DONE — hidden `serve --desktop-managed` implies resident behavior and writes `Desktop` / `DesktopOwned` marker; README/config docs state this is reserved for future visible desktop/tray owner and not a user-facing default.

## M4 — Documentation, Gates, And Closeout

- [x] DLO-070 [owner=main] [deps=DLO-060] [scope=README.md,README_EN.md,docs/CONFIGURATION*.md,docs/workstreams/desktop-lifecycle-owner]
  Goal: Document simple default, attached observer semantics, resident daemon, and future desktop owner path.
  Validation: cargo fmt --check; cargo check -p codex-helper -p codex-helper-gui -p codex-helper-tui
  Review: Docs must not recommend daemon commands for ordinary users unless they intentionally choose advanced mode.
  Evidence: EVIDENCE_AND_GATES.md final command log.
  Handoff: DONE — README/README_EN/config docs distinguish simple default from advanced resident/attached mode and record the hidden future desktop sidecar path. Full Tauri app/service install remains deferred.
