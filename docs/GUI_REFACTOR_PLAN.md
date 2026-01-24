# GUI Refactor Plan (egui / eframe)

> 中文速览（便于协作沟通）：  
> 目标是给 `codex-helper` 增加一个 **桌面 GUI（类似 Clash Verge 的信息架构）**，以 Windows 为主、跨平台尽力。GUI 能启动/托管本地代理、托盘管理、开机启动；当检测到已有代理占用端口时可选择 **Attach**，并支持“记住我的选择”。初期允许直接在 GUI 中编辑配置（表单优先，提供原始文本编辑作为高级模式）。

## Goals

- Provide a desktop GUI with feature parity to the current TUI:
  - Start/stop the proxy (Codex first; Claude best-effort).
  - Manage configs/providers: active selection, session pinned override, enable/disable, level, upstream health check.
  - Observe sessions/requests: active sessions, recent finished requests, retry chain, usage.
  - Browse Codex history sessions (`~/.codex/sessions`) and open transcript viewer with copy-to-clipboard.
  - Settings: language, refresh, runtime config reload, import/overwrite-from-codex (with confirmation).
- Windows-first UX (tray + autostart + single-instance), with cross-platform support where the ecosystem allows.
- Support "Attach to existing proxy" when port is already in use, with a remembered decision.
- Prefer GUI-based configuration editing (form-first), while still offering a raw editor as an advanced option.

## Non-goals (initially)

- A full daemon/service install flow (Windows Service / launchd / systemd) in the first iteration.
- Remote management over network. Attach is local-only (loopback / local IPC).
- Replacing the existing CLI/TUI. The GUI is an additional frontend.

## Current Baseline (what we already have)

- A single binary (`codex-helper` / `ch`) that can:
  - Run an Axum proxy on `127.0.0.1:<port>`.
  - Optionally launch an interactive TUI when running in a terminal.
  - Maintain runtime state in `ProxyState` (sessions, requests, overrides, usage rollups, health checks, LB states).
  - Expose a small local control/status API:
    - `GET/POST /__codex_helper/override/session` (effort override)
    - `GET /__codex_helper/status/active`
    - `GET /__codex_helper/status/recent`
    - `GET /__codex_helper/config/runtime`
    - `POST /__codex_helper/config/reload`
    - Versioned attach-friendly API (v1):
      - `GET /__codex_helper/api/v1/capabilities`
      - `GET /__codex_helper/api/v1/status/active`
      - `GET /__codex_helper/api/v1/status/recent?limit=...`
      - `GET /__codex_helper/api/v1/status/session-stats`
      - `GET /__codex_helper/api/v1/config/runtime`
      - `POST /__codex_helper/api/v1/config/reload`
      - `GET /__codex_helper/api/v1/configs`
      - `GET/POST /__codex_helper/api/v1/overrides/session/effort`
      - `GET/POST /__codex_helper/api/v1/overrides/session/config`
      - `GET/POST /__codex_helper/api/v1/overrides/global-config`

## Target UX / IA (Clash Verge-like)

Suggested top-level navigation (left sidebar or top tabs):

1) **Overview**
   - Proxy status (running / attached), listening address/port, current service (Codex/Claude)
   - Current active config and routing warnings
   - Quick actions: Start/Stop, Reload runtime config, Open logs folder

2) **Sessions**
   - Session list + filters (active-only / errors-only / overrides-only)
   - Session details panel (cwd/model/provider/config/usage/ttfb/last status)
   - Actions: set effort override, set pinned config (session), open transcript viewer

3) **Requests**
   - Recent finished requests table + filters (errors-only / scope session)
   - Request details: retry chain, upstream base_url, usage, TTFB, durations

4) **Providers / Configs**
   - Config list with level, enabled, active marker, upstream count, health status
   - Actions:
     - Set active (preferred but allow same-level failover)
     - Toggle enabled, adjust level
     - Run/cancel health check (selected / all)
     - Open config details (upstreams, auth source, supported models, mapping)

5) **History (Codex)**
   - Scan `~/.codex/sessions` for current directory; show session summaries
   - Open transcript viewer

6) **Settings**
   - UI prefs: language, refresh interval, remember decisions
   - Proxy control: default port, startup behavior
   - Integrations: import from Codex CLI, overwrite confirmation
   - Tray / autostart toggles

## Runtime Modes

The GUI supports two modes (with a clear indicator in the title bar and Overview page):

### Mode A: Integrated (GUI hosts proxy)

- `codex-helper-gui` starts the proxy server inside the same process (preferred initial mode).
- GUI reads/writes `ProxyState` directly (in-process calls), which minimizes refactor cost.
- Tray actions control the in-process proxy (Start/Stop/Show/Hide/Exit).

### Mode B: Attach (GUI connects to existing proxy)

- Triggered when the configured listen port is already in use.
- GUI prompts:
  - **Attach** (recommended)
  - **Start on another port**
  - **Exit**
  - plus "Remember my choice" (persisted to GUI config).
- Attach requires a sufficiently rich local API. We can do this incrementally:
  1) MVP: read-only attach (sessions/recent/runtime config) using existing endpoints.
  2) Full attach: extend API to support config changes, health checks, overrides, and history/transcript utilities.

## GUI Config (“remember my choice”)

We add a separate GUI config file to avoid disrupting existing proxy config semantics:

- Path (proposed): `~/.codex-helper/gui.toml`
- Contents (example):
  - `ui.language = "zh"|"en"`
  - `ui.refresh_ms = 500`
  - `proxy.default_service = "codex"|"claude"`
  - `proxy.default_port = 3211`
  - `attach.last_port = 3211` (optional; used by manual attach UI)
  - `attach.on_port_in_use = "ask"|"attach"|"start_new_port"|"exit"`
  - `attach.remember_choice = true|false`
  - `window.close_behavior = "minimize_to_tray"|"exit"` (default: `minimize_to_tray`)
  - `tray.enabled = true|false`
  - `autostart.enabled = true|false`

Notes:
- Proxy runtime config stays in `~/.codex-helper/config.toml` / `config.json`.
- GUI config must never store secrets.

## Refactor Strategy (keep UI logic out of core)

### Extract a UI-independent “dashboard core”

Today, some aggregation logic lives in TUI-specific modules (`src/tui/model.rs`).
We should extract shared logic into a new module, so both TUI and GUI can reuse it:

- New module (proposed): `src/dashboard_core/`
  - `snapshot.rs`: build snapshot from `ProxyState` (sessions, recent requests, overrides, usage rollup, health)
  - `types.rs`: `Snapshot`, `SessionRow`, `ProviderOption`, etc. (UI-neutral)
  - No `ratatui` types, no terminal-specific formatting.

TUI keeps its renderer + key handling, but consumes the shared snapshot/types.
GUI consumes the same snapshot/types, rendered with egui widgets.

### Expand local control API (for Attach)

Introduce a versioned API namespace:

- Implemented (v1):
  - `GET /__codex_helper/api/v1/capabilities` (API discovery)
  - `GET /__codex_helper/api/v1/status/active` / `.../recent` / `.../session-stats`
  - `GET /__codex_helper/api/v1/config/runtime` + `POST .../config/reload`
  - `GET /__codex_helper/api/v1/configs`
  - `GET/POST /__codex_helper/api/v1/overrides/session/effort`
  - `GET/POST /__codex_helper/api/v1/overrides/session/config`
  - `GET/POST /__codex_helper/api/v1/overrides/global-config`

- Planned (later):
  - `POST /__codex_helper/api/v1/config/meta_override` (enable/level without writing disk config)
  - `POST /__codex_helper/api/v1/healthcheck/start` / `POST /__codex_helper/api/v1/healthcheck/cancel`
  - `GET /__codex_helper/api/v1/history/list` + `GET /__codex_helper/api/v1/history/transcript`

Principles:
- Local-only by default (bind to `127.0.0.1`).
- Keep v1 simple (no token) but versioned for future hardening if needed.

## Desktop Tech Choices (Windows-first)

### GUI framework
- `eframe` (egui): fast iteration, single binary, good for dashboard-style apps.

### Localization (i18n)
- Support at least `zh`/`en` from day one, persisted in GUI config.
- Reuse the existing language selection semantics where possible (TUI already has `Language::Zh/En` and `pick(zh, en)` patterns).

### Tray
- `tray-icon` + `tao` (Windows-first but can be cross-platform).
- Tray menu items: Show/Hide, Start/Stop proxy, Reload config, Open config, Open logs, Quit.

### Autostart
- Use `auto-launch` (or platform-specific implementation) for “Run at login”.
- For Windows, registry-based autostart is sufficient for first iteration.

### Single-instance
- Use a lock file under `~/.codex-helper/locks/gui.lock` (or `single-instance` crate).
- If a second instance starts: focus existing window (best-effort) or show a message and exit.

## Milestones (high level)

1) **M0: Foundations**
   - Add new binary `codex-helper-gui`
   - Window shell + navigation skeleton (Overview/Sessions/Requests/Configs/History/Settings)
   - Read proxy config from disk and render it (no runtime yet)

2) **M1: Integrated proxy**
   - Start/stop proxy from GUI
   - Live sessions/recent requests panels (in-process `ProxyState`)

3) **M2: Config editing**
   - Form-based editor for configs/providers (active/enabled/level/upstreams)
   - Import/overwrite-from-codex flow with confirmation

4) **M3: Transcript + History**
   - History browser, transcript viewer, copy-to-clipboard

5) **M4: Tray + autostart**
   - Minimize-to-tray, autostart toggles, persisted GUI config

6) **M5: Attach**
   - Attach prompt + remembered decision
   - Expand local API to reach full manageability in attach mode

## Risks & Mitigations

- **API surface growth**: keep endpoints versioned (`/api/v1`), reuse existing validation rules, and limit to loopback.
- **State duplication**: avoid reimplementing aggregation in GUI; extract shared dashboard core.
- **Windows event loop complexity** (tray + egui): choose a known-good integration (tao) and keep async tasks behind a runtime boundary.
- **Config write races**: use existing `save_config`/`ServiceConfigManager` patterns; in attach mode, serialize writes on the proxy side.
