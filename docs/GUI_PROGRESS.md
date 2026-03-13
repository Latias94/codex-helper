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
- Attach security (v1): **loopback-default; remote non-loopback admin requires shared token**.

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
- [x] GUI-106 Sessions list UX: stable ordering + active partitioning + search + “lock order”

**Definition of done**
- GUI can start/stop proxy, and live data (sessions/recent) updates.

### M2 — Config Editing (form-first)

- [x] GUI-201 Config list + details (active/enabled/level/upstreams, health summary)
- [x] GUI-202 Actions: set active / clear active (auto)
- [x] GUI-203 Actions: set/clear session pinned config, set/clear effort override
- [x] GUI-207 Actions: set/clear global pinned config override
- [x] GUI-204 Actions: toggle enabled, adjust level (persisted)
- [x] GUI-208 Health checks: start/cancel + status + upstream results (running + attach v1)
- [x] GUI-205 Import from Codex CLI (sync auth env keys best-effort) + overwrite confirmation flow
- [x] GUI-206 Optional: advanced raw editor (TOML) with validation and “apply” button

**Definition of done**
- GUI can fully replace TUI config management actions in integrated mode.

### M3 — History + Transcript

- [x] GUI-301 History list for current dir (`~/.codex/sessions`) + refresh
- [x] GUI-302 Transcript viewer: tail/all toggle, scrolling, copy-to-clipboard
- [x] GUI-303 “Open in explorer/finder” for session file/logs (Windows-first)
- [x] GUI-304 Open transcript from Sessions (jump to History + auto load by session_id)
- [x] GUI-305 Global recent history: list recent sessions by mtime (default 12h) + copy `root id` + open `codex resume` in Windows Terminal (`wt`)
- [x] GUI-306 All history by date: browse all Codex sessions grouped by day, with transcript preview and copy (tool calls hidden by default)

**Definition of done**
- GUI can browse history and read/copy transcripts like TUI.

### M4 — Tray + Autostart (Windows-first)

- [x] GUI-401 Add tray icon + menu: Show/Hide, Start/Stop, Quit
- [x] GUI-402 Close-to-tray behavior (configurable)
- [x] GUI-403 Autostart toggle (Windows registry; best-effort other platforms)
- [x] GUI-404 Single-instance guard + focus existing instance
- [x] GUI-405 Startup behavior: start minimized / minimize-to-tray (configurable)

**Definition of done**
- GUI behaves like a desktop companion: tray + autostart works on Windows.

### M5 — Attach Mode (connect to existing proxy)

- [x] GUI-501 Port-in-use detection UX: prompt + remember choice
- [x] GUI-502 MVP attach (read-only) using control-plane endpoints:
  - `/__codex_helper/api/v1/status/active`, `/status/recent`, `/runtime/status`
- [x] GUI-503 Extend proxy API to `/__codex_helper/api/v1/...` for full control
- [~] GUI-504 Full attach via API v1 (Clash Verge style)
  - [x] Config actions: active/enabled/level
  - [x] Provider structure: alias/enabled/auth env refs/endpoints CRUD
  - [x] Retry/failover: persisted retry profile + cooldown policy editor
  - [x] Overrides: session station/effort + global station
  - [x] Health checks: start/cancel + status
  - [x] History/transcript: list + tail/all + copy + open file
  - [x] Discovery UI: scan/list existing proxy instances (default 3210-3220)
  - [x] Auto attach-or-start: probe configured port, fallback scan, then start
- [x] GUI-505 Manual attach to a specified port (no discovery UI yet)

**Definition of done**
- If the proxy is already running, GUI can attach and manage it (or at least observe, in MVP).

## Refactor Workstream (shared core)

- [x] CORE-001 Extract `Snapshot` building logic into UI-neutral module (shared by TUI + GUI)
- [ ] CORE-002 Keep rendering-only code in TUI; remove `ratatui` types from shared core
- [ ] CORE-003 Add unit tests for snapshot aggregation (Windows path handling already exists; extend as needed)
- [ ] CORE-004 Versioned API layer for attach mode (see M5)

## QA / Release

- [ ] QA-001 Manual test checklist (Windows): start/stop, tray, autostart, config edit, health check, history/transcript
- [ ] QA-002 Automated tests (where feasible): config schema, snapshot aggregation, API handlers
- [ ] REL-001 Update `README.md` and `README_EN.md` with GUI usage
- [ ] REL-002 cargo-dist packaging notes for GUI binary (Windows `.zip` asset layout)

## Change Log (append-only)

- 2026-02-01: Added GUI Stats page; introduced `dashboard_core` snapshot/window stats; added API v1 `/snapshot` for attach mode.
- 2026-01-24: Finished GUI-001..GUI-006 (GUI binary + `gui` feature, shell/nav, GUI config, zh/en toggle, raw config editor stub).
- 2026-01-24: Finished GUI-101/102, GUI-501/502 (integrated start/stop, port-in-use prompt+remember, attach read-only refresh).
- 2026-01-24: Finished GUI-104/105 (+ GUI-203): sessions/requests pages backed by in-process `ProxyState`, with session overrides editing (pinned config + effort).
- 2026-01-24: Finished GUI-401/402/403 (tray icon+menu, close-to-tray, Windows autostart toggle).
- 2026-01-24: Finished GUI-103, GUI-404 (overview runtime info; single-instance guard + notify existing instance to show window).
- 2026-01-24: Finished GUI-201/204/206 (config form view for active/enabled/level + keep raw editor).
- 2026-01-24: Finished GUI-301 (history list + transcript viewer tail); tray adds Reload/Open Config/Open Logs.
- 2026-01-24: Finished GUI-503; GUI attach upgraded to read full runtime snapshot and write overrides (effort + session config + global override via API v1).
- 2026-01-24: Finished GUI-207: global pinned config override UI (integrated + attach v1).
- 2026-01-24: Finished GUI-302/303 and GUI-505: transcript all/tail + copy + open file; manual attach by port.
- 2026-01-24: Finished GUI-202: set/clear active (auto) in config form view.
- 2026-01-24: Finished GUI-205: import/sync providers from Codex CLI (preview + apply & save).
- 2026-01-24: Finished GUI-208: health check controls + attach v1 endpoints for status/start/cancel.
- 2026-01-24: Finished GUI-405: startup behavior setting (show/minimized/to tray) and tray hide uses `Visible=false`.
- 2026-01-24: Finished GUI-504 discovery: scan local ports 3210-3220 and attach with one click.
- 2026-01-24: Finished GUI-106: sessions list stable ordering + search + lock order (avoid jitter with multiple CLIs).
- 2026-01-24: Finished GUI-304: open transcript directly from Sessions (auto-navigate to History).
- 2026-03-11: Finished CP-305 GUI migration: removed legacy GUI routing presets from `gui.toml`, Overview, tray, and auto-apply flow; proxy config profiles are now the only formal profile entry point.
- 2026-03-11: Config v2 Profiles section now uses control-plane profile CRUD when the selected service matches the running/attached proxy; attach mode no longer treats that section as local-file-only.
- 2026-03-11: Config v2 common station fields (`active_station`, `enabled`, `level`) now use persisted station control-plane APIs when the selected service matches the running/attached proxy; attached mode no longer writes those actions to the local file by mistake.
- 2026-03-11: Stations page is now remote-first for persisted station controls too: runtime quick switch remains runtime-only, while configured `active_station` / `enabled` / `level` write through the current proxy when the target exposes station config APIs.
- 2026-03-11: Added persisted retry/failover control-plane support to GUI: running/attached snapshots now carry configured+resolved retry policy, the Stations page can edit retry profile plus cooldown/backoff fields, and attach mode stays read-only when the remote proxy does not expose `/api/v1/retry/config`.
- 2026-03-11: Config v2 station editor now supports structure-level station management too: local mode can add/delete/edit station alias+members in form view, and attached mode can do the same when the proxy exposes `/api/v1/stations/specs` without exposing provider secrets.
- 2026-03-11: Config v2 provider editor now supports structure-level provider management: local mode can add/delete/edit alias, env auth refs, and endpoints while preserving advanced tags/model mappings; attached mode can do the same when the proxy exposes `/api/v1/providers/specs`, otherwise the section stays read-only.
- 2026-03-11: Config v2 Profiles now include a linked route preview: while editing a profile, GUI shows the resolved station source (`profile.station` / `active_station` / auto), visible member/provider routes, and capability mismatches for model / fast mode / reasoning when that data is available.
- 2026-03-12: Continued GUI page split: moved the Settings page into `pages/settings.rs`, moved Stations profile/retry operator panels into `pages/stations.rs`, and cleaned remaining legacy GUI labels like `Configs` / `No config selected` toward station-first wording.
- 2026-03-12: Continued GUI page split again: moved Config v2 station/provider/profile helper panels and spec builders into `pages/config_v2.rs`, reducing `pages/mod.rs` further while keeping GUI tests green.
- 2026-03-12: Continued GUI page split once more: moved the advanced raw config editor into `pages/config_raw.rs`, so config-page rendering logic is less concentrated in `pages/mod.rs` while `cargo check -p codex-helper-gui` and `cargo nextest run -p codex-helper-gui` remain green.
- 2026-03-12: Continued GUI page split again: moved the main Stations page renderer into `pages/stations.rs`, switched the page entrypoint in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while shrinking `pages/mod.rs` by another large chunk.
- 2026-03-12: Continued GUI page split again: moved the full Config v2 form renderer into `pages/config_v2.rs`, switched the Config form branch in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while cutting `pages/mod.rs` down to the remaining shared helpers plus legacy config form.
- 2026-03-13: Continued GUI page split again: moved the Overview page renderer and its station-summary helper into `pages/overview.rs`, switched the Overview page entrypoint in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while shrinking `pages/mod.rs` further toward shared helpers plus legacy config form.
- 2026-03-13: Continued GUI page split again: moved the legacy Config form renderer into `pages/config_legacy.rs`, switched the Config form branch in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while removing the largest remaining page function from `pages/mod.rs`.
- 2026-03-13: Continued GUI page split again: moved the Stats page renderer into `pages/stats.rs`, switched the Stats page entrypoint in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while pushing `pages/mod.rs` closer to a shared-helper shell.
- 2026-03-13: Continued GUI page split again: moved the Setup page renderer into `pages/setup.rs`, switched the Setup page entrypoint in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while removing the last large onboarding page from `pages/mod.rs`.
- 2026-03-13: Continued GUI page split again: moved the Doctor page renderer into `pages/doctor.rs`, switched the Doctor page entrypoint in `pages/mod.rs` to the module renderer, and kept `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` green while leaving `pages/mod.rs` primarily as navigation plus shared helper code.
- 2026-03-13: Continued GUI shared-helper split: moved runtime station health/capability helpers into `pages/runtime_station.rs`, then moved profile route preview builders/catalog helpers into `pages/profile_preview.rs`; `pages/mod.rs` dropped to about 66 KB and `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` stayed green (`40/40`).
- 2026-03-13: Continued GUI shared-helper split again: moved retry editor helpers into `pages/retry_editor.rs`, moved profile preview rendering/value helpers fully into `pages/profile_preview.rs`, and moved config parse/save/sync glue into `pages/config_document.rs`; `pages/mod.rs` dropped further to about 50 KB and `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` stayed green (`40/40`).
- 2026-03-13: Continued GUI shared-helper split once more: moved remote attach/admin/token/loopback helpers into `pages/remote_attach.rs`, moved time/label/usage formatting helpers into `pages/formatting.rs`, and moved history/workdir/Windows Terminal batching helpers into `pages/history_tools.rs`; `pages/mod.rs` dropped further to about 35 KB and `cargo check -p codex-helper-gui` plus `cargo nextest run -p codex-helper-gui` stayed green (`40/40`).
- 2026-03-13: Continued GUI shell split: moved grouped navigation definitions/rendering into `pages/navigation.rs` and moved the Config page shell into `pages/config_shell.rs`; `pages/mod.rs` dropped further to about 31 KB and is now mostly view-state/type definitions plus page dispatch, while `cargo check -p codex-helper-gui` and `cargo nextest run -p codex-helper-gui` remain green (`40/40`).
- 2026-03-13: Continued GUI shell split to near-closeout: moved remaining page/view state definitions into `pages/view_state.rs`; `pages/mod.rs` dropped further to about 26 KB and now acts mainly as the shared page index, public shell entry, and test host while `cargo check -p codex-helper-gui` remains clean and `cargo nextest run -p codex-helper-gui` stays green (`40/40`).
