# Desktop Lifecycle Owner

Status: Active
Last updated: 2026-05-20

## Why This Lane Exists

The previous resident/attach-first work proved the proxy can outlive a single console, but it also exposed a product risk: without a visible tray/desktop owner, users cannot reliably tell whether the local proxy is still running or how to fully exit it. The next architecture step is to make lifecycle ownership explicit and deep enough that a future Clash-like desktop/Tauri shell can own the proxy without confusing the default TUI/GUI behavior.

## Relevant Authority

- Existing docs:
  - `README.md`
  - `README_EN.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
- Related workstreams:
  - `docs/workstreams/resident-proxy-attach-first/`
- Current code seams:
  - `crates/core/src/runtime_host.rs`
  - `src/cli_app.rs`
  - `src/cli_types.rs`
  - `crates/gui/src/gui/proxy_control.rs`
  - `crates/gui/src/gui/app.rs`
  - `crates/tui/src/tui/attached.rs`

## Problem

Proxy lifecycle is still split across CLI, GUI, TUI, and admin API callers. Each caller must know whether it owns a runtime, attaches to an external runtime, should restore client patch state, should send remote shutdown, or should merely detach. This makes default behavior easy to regress and makes a future tray/Tauri owner hard to implement safely.

## Target State

- Lifecycle is represented by explicit modes rather than implied by scattered flags.
- A core `RuntimeManager`-style module owns high-level lifecycle decisions above the low-level `runtime_host` factory.
- Runtime owner metadata records whether a resident proxy is manual CLI, supervisor-owned, or desktop-managed.
- GUI/TUI/daemon become adapters over the same lifecycle semantics:
  - default console/GUI mode remains ephemeral and exits cleanly;
  - attached observer mode detaches by default;
  - explicit desktop/daemon owner mode may keep a sidecar resident proxy alive;
  - explicit Stop/Quit operations have unambiguous shutdown behavior.
- Documentation explains the simple default and the advanced desktop/daemon owner behavior.

## In Scope

- Add explicit lifecycle/owner domain types in core.
- Add owner marker read/write/clear helpers under the codex-helper run directory.
- Refactor shared lifecycle operations into a deeper module instead of leaving them scattered in GUI/CLI code.
- Add CLI surface for managed/desktop-owned resident child semantics if needed by the manager.
- Update GUI adapter to consume manager semantics while preserving current default: UI exit stops owned runtime, attached exit detaches only.
- Keep `daemon status/stop/supervise` working and show/report owner metadata where available.
- Tests for owner markers, lifecycle mode decisions, and attached-vs-owned stop semantics.
- README/configuration docs for the new model.

## Out Of Scope

- Shipping a full Tauri app in this lane.
- Windows Service / launchd / systemd installation.
- Background auto-start policy beyond existing GUI autostart and explicit supervisor mode.
- Changing the default `codex-helper serve` behavior to resident.
- Reintroducing silent attach-first startup as the default.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| `runtime_host` is the right low-level factory seam and should not absorb high-level product policy. | High | Existing resident/GUI runtime code already reuses it. | If wrong, manager abstractions may duplicate factory behavior. |
| User-facing default should remain ephemeral until a visible tray/desktop owner exists. | High | User feedback in this thread; current docs have been adjusted. | If wrong, we may underuse resident capabilities. |
| Owner marker files are sufficient for local desktop/daemon coordination before a full service manager. | Medium | Existing supervisor already writes crash markers under `~/.codex-helper/run/`. | If insufficient, later service integration may need a stronger registry/IPC. |
| Desktop/Tauri should manage a sidecar resident proxy rather than always embedding the proxy in the UI process. | Medium | Crash isolation concern from long-running tasks; Clash-like UX expectation. | If wrong, first desktop implementation could be simpler but less isolated. |

## Architecture Direction

Introduce a deeper lifecycle module with a small interface and concentrated policy:

```text
runtime_host              low-level runtime construction/start handles
runtime_manager           lifecycle mode, owner marker, start/stop/attach/detach decisions
CLI daemon/serve          command adapter
GUI proxy_control         UI adapter over manager semantics
TUI attached dashboard    read-only observer adapter
future Tauri backend      desktop/tray owner adapter
```

Terminology:

- **EphemeralConsole**: caller owns the proxy; UI/console exit stops it and restores client patch where applicable.
- **AttachedObserver**: caller observes/controls an existing proxy; normal UI exit detaches only.
- **ResidentDaemon**: explicit CLI daemon/supervisor-owned proxy.
- **DesktopOwned**: explicit desktop/tray-owned resident sidecar; tray Quit is the full shutdown affordance.

The main seam is not a pass-through wrapper around `ProxyService`; it should encode lifecycle policy so callers do not duplicate owner/stop/attach decisions.

## Closeout Condition

This lane can close when:

- lifecycle mode and owner metadata are first-class and tested,
- GUI/TUI/daemon behavior routes through the new semantics or a documented adapter shim,
- default simple behavior remains non-resident,
- explicit desktop/daemon owner behavior is representable and observable,
- evidence gates pass,
- docs reflect shipped behavior,
- and Tauri/full service work is either split or explicitly deferred.
