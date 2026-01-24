# GUI Work Tracking (egui / eframe)

> 中文速览：这是 GUI 改造的阶段性 TODO 清单与进度跟踪文件；建议每完成一块就勾选并在本文件追加简短变更记录，避免“做了但忘记同步”。

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Decisions (locked)

- Platform: **Windows-first**, cross-platform best-effort.
- On port-in-use: prompt user, support **Attach**, support “remember my choice”.
- Config editing: start with **GUI editing** (form-first); optionally add raw editor as advanced mode.
- Default close behavior: **minimize to tray**.
- Attach security (v1): **no token**, keep API extensible for future hardening.

## Open Questions (need confirmation before M4/M5)

- `[ ]` Should GUI be able to stop/replace an existing proxy process (or never)?

## Milestones

### M0 — Foundations (new GUI binary + navigation)

- [x] GUI-001 Add `codex-helper-gui` binary (Cargo `[[bin]]`)
- [x] GUI-002 Add `eframe/egui` dependencies behind a `gui` feature (optional)
- [x] GUI-003 Create app shell + navigation: Overview / Sessions / Requests / Configs / History / Settings
- [x] GUI-004 Define GUI config file (`~/.codex-helper/gui.toml`) schema + load/save
- [x] GUI-005 Basic logging + error panel (user-facing, no secrets)
- [x] GUI-006 i18n: language toggle + persisted choice (Zh/En)

**Definition of done**
- Launches a window, navigation works, GUI config persists language/refresh/default port.

### M1 — Integrated Proxy (GUI hosts proxy)

- [x] GUI-101 Start proxy from GUI (Codex first): choose port, show bind errors nicely
- [x] GUI-102 Stop proxy gracefully (Ctrl+C equivalent): ensure config restore behavior remains correct
- [x] GUI-103 Show runtime status: listening addr/port, current active config, model routing warnings
- [x] GUI-104 Sessions view backed by in-process `ProxyState` snapshot refresh
- [x] GUI-105 Requests view backed by in-process `ProxyState` snapshot refresh

**Definition of done**
- GUI can start/stop proxy, and live data (sessions/recent) updates.

### M2 — Config Editing (form-first)

- [x] GUI-201 Config list + details (active/enabled/level/upstreams, health summary)
- [ ] GUI-202 Actions: set active / clear active (auto)
- [x] GUI-203 Actions: set/clear session pinned config, set/clear effort override
- [x] GUI-207 Actions: set/clear global pinned config override
- [x] GUI-204 Actions: toggle enabled, adjust level (persisted)
- [ ] GUI-205 Import from Codex CLI (sync auth env keys best-effort) + overwrite confirmation flow
- [x] GUI-206 Optional: advanced raw editor (TOML) with validation and “apply” button

**Definition of done**
- GUI can fully replace TUI config management actions in integrated mode.

### M3 — History + Transcript

- [x] GUI-301 History list for current dir (`~/.codex/sessions`) + refresh
- [~] GUI-302 Transcript viewer: tail/all toggle, paging/scrolling, copy-to-clipboard
- [~] GUI-303 “Open in explorer/finder” for session file/logs (Windows-first)

**Definition of done**
- GUI can browse history and read/copy transcripts like TUI.

### M4 — Tray + Autostart (Windows-first)

- [x] GUI-401 Add tray icon + menu: Show/Hide, Start/Stop, Quit
- [x] GUI-402 Close-to-tray behavior (configurable)
- [x] GUI-403 Autostart toggle (Windows registry; best-effort other platforms)
- [x] GUI-404 Single-instance guard + focus existing instance

**Definition of done**
- GUI behaves like a desktop companion: tray + autostart works on Windows.

### M5 — Attach Mode (connect to existing proxy)

- [x] GUI-501 Port-in-use detection UX: prompt + remember choice
- [x] GUI-502 MVP attach (read-only) using existing endpoints:
  - `/__codex_helper/status/active`, `/status/recent`, `/config/runtime`
- [x] GUI-503 Extend proxy API to `/__codex_helper/api/v1/...` for full control
- [~] GUI-504 Full attach: config actions, overrides, health checks, history/transcript via API
- [ ] GUI-505 Optional: attach to a non-default port / discovery UI

**Definition of done**
- If the proxy is already running, GUI can attach and manage it (or at least observe, in MVP).

## Refactor Workstream (shared core)

- [ ] CORE-001 Extract `Snapshot` building logic into UI-neutral module (shared by TUI + GUI)
- [ ] CORE-002 Keep rendering-only code in TUI; remove `ratatui` types from shared core
- [ ] CORE-003 Add unit tests for snapshot aggregation (Windows path handling already exists; extend as needed)
- [ ] CORE-004 Versioned API layer for attach mode (see M5)

## QA / Release

- [ ] QA-001 Manual test checklist (Windows): start/stop, tray, autostart, config edit, health check, history/transcript
- [ ] QA-002 Automated tests (where feasible): config schema, snapshot aggregation, API handlers
- [ ] REL-001 Update `README.md` and `README_EN.md` with GUI usage
- [ ] REL-002 cargo-dist packaging notes for GUI binary (Windows `.zip` asset layout)

## Change Log (append-only)

- 2026-01-24: Finished GUI-001..GUI-006 (GUI binary + `gui` feature, shell/nav, GUI config, zh/en toggle, raw config editor stub).
- 2026-01-24: Finished GUI-101/102, GUI-501/502 (integrated start/stop, port-in-use prompt+remember, attach read-only refresh).
- 2026-01-24: Finished GUI-104/105 (+ GUI-203): sessions/requests pages backed by in-process `ProxyState`, with session overrides editing (pinned config + effort).
- 2026-01-24: Finished GUI-401/402/403 (tray icon+menu, close-to-tray, Windows autostart toggle).
- 2026-01-24: Finished GUI-103, GUI-404 (overview runtime info; single-instance guard + notify existing instance to show window).
- 2026-01-24: Finished GUI-201/204/206 (config form view for active/enabled/level + keep raw editor).
- 2026-01-24: Finished GUI-301 (history list + transcript viewer tail); tray adds Reload/Open Config/Open Logs.
- 2026-01-24: Finished GUI-503; GUI attach upgraded to read full runtime snapshot and write overrides (effort + session config + global override via API v1).
- 2026-01-24: Finished GUI-207: global pinned config override UI (integrated + attach v1).
