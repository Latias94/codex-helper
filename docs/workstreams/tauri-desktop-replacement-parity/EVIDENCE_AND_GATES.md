# Tauri Desktop Replacement Parity — Evidence And Gates

Status: Complete
Last updated: 2026-05-23

Historical note (superseded 2026-07-13): references below to config import and `ProxyConfigV4` describe the retired implementation. Current startup accepts only the canonical version 5 config and uses semantic unversioned runtime types.

## Gate Set

### Desktop Frontend Gate

```powershell
cd apps/desktop
pnpm test
pnpm build
```

Proves the React/Tauri frontend compiles and targeted UI behavior still passes.

### Desktop Rust Gate

```powershell
cargo fmt --check
cargo check -p codex-helper-desktop
cargo nextest run -p codex-helper-desktop --lib
```

Proves the Tauri Rust command/lifecycle crate formats, compiles, and passes targeted command/lifecycle tests.

### Diff Hygiene Gate

```powershell
git diff --check -- .
```

Proves no whitespace errors in the current diff. Windows LF/CRLF warnings are acceptable if there are no whitespace errors.

### Packaging Gate

```powershell
cd apps/desktop
pnpm tauri:build
```

Run when packaging behavior is part of the claim. If packaging fails because signing, installer, or platform dependencies are not yet configured, record the failure and either fix it or split the packaging blocker before replacement claims.

### Packaged Lifecycle Smoke Gate

Manual or scripted packaged app evidence must cover:

- start packaged app;
- start desktop-managed proxy without `CODEX_HELPER_CLI_PATH`;
- close main window hides to tray;
- tray show/hide works;
- Quit App exits desktop only;
- Detach does not stop proxy;
- Stop Proxy requires confirmation and stops only by explicit action;
- second launch focuses existing window;
- config export/import creates backup and validates TOML.

## Evidence Anchors

- `docs/workstreams/tauri-desktop-client/REPLACEMENT_READINESS.md`
- `docs/workstreams/tauri-desktop-replacement-parity/DESIGN.md`
- `docs/workstreams/tauri-desktop-replacement-parity/TODO.md`
- `apps/desktop/src-tauri/src/`
- `apps/desktop/src/features/settings/`
- `apps/desktop/src/features/providers/`

## Evidence Log

### 2026-05-22 — Workstream opened

Evidence:

- User asked to continue all replacement follow-ons with the goal of replacing egui.
- Created `docs/workstreams/tauri-desktop-replacement-parity/` with DESIGN, TODO, MILESTONES, EVIDENCE_AND_GATES, HANDOFF, WORKSTREAM.json, and JOURNAL.
- First executable task is TDRP-020: Settings path actions plus lightweight single-config export/import.

Result:

- PASS — scope and task split are explicit. Implementation work may begin.

### 2026-05-22 — TDRP-020 path actions and lightweight config import/export

Evidence:

- Added Tauri path/config commands in `apps/desktop/src-tauri/src/commands/paths.rs`:
  - `open_known_path` for home/config/logs/cache via `tauri-plugin-opener`;
  - `export_config` for copying the single active `config.toml` to a user-selected file;
  - `import_config` for reading a selected `.toml`, validating it as `ProxyConfigV4`, backing up the current config with a timestamped `.bak`, and replacing the active `config.toml`.
- Added `tauri-plugin-dialog` and frontend file picker integration for export/import.
- Updated Settings:
  - About/paths rows have open buttons;
  - config/log/cache path actions are real Tauri commands;
  - export/import buttons are wired;
  - UI warns that exported TOML may contain inline secrets;
  - import success reports backup path when a previous config existed.
- Added tests:
  - Rust tests prove export copies the single config and carries a secret warning;
  - Rust tests prove import validates TOML, creates a timestamped backup, and does not overwrite current config when validation fails;
  - frontend route test proves Settings path/export/import buttons invoke the correct Tauri commands and report backup status.

Review:

- Workstream compliance: PASS — this implements TDRP-020 without introducing profile/workspace/config-catalog management.
- Code quality: PASS_WITH_CONCERNS — validation accepts supported TOML config shapes and intentionally excludes legacy JSON import from the lightweight desktop UI flow.
- Missing gates: final `git diff --check -- .` still needs to be run after evidence/doc updates.

Verification:

- Command: `cargo nextest run -p codex-helper-desktop --lib paths::tests`
- Scope: TDRP-020 Rust path/config command tests.
- Result: PASS — 3 tests.

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 12 tests.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Packaged file picker/open-path smoke remains part of TDRP-040/TDRP-080.
- Runtime reload after import is still a user action; future polish can offer "Import and reload" after packaged lifecycle gates are stable.

Result:

- DONE_WITH_CONCERNS — TDRP-020 is implemented and verified at command/build/test level; packaged smoke remains a later gate.

### 2026-05-22 — TDRP-030 single instance

Evidence:

- Added `tauri-plugin-single-instance`.
- Registered the plugin during Tauri builder setup.
- The second-instance callback calls the existing lifecycle path that shows, unminimizes, and focuses the main window.
- Added a lifecycle assertion that second-instance launch leaves any proxy runtime running and does not request Stop Proxy or app shutdown.

Review:

- Workstream compliance: PASS_WITH_CONCERNS — code-level single-instance behavior is wired. The remaining concern is packaged second-launch smoke, which belongs to TDRP-080.
- Code quality: PASS — reuse of `lifecycle::show_main_window` keeps behavior consistent with tray Show Window and avoids duplicate focus logic.
- Missing gates: packaged smoke is not expected for this code slice but remains required before egui replacement.

Verification:

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib lifecycle::tests::second_instance_launch_never_stops_proxy_runtime`
- Scope: targeted lifecycle policy.
- Result: PASS — 1 test.

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 13 tests.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Result:

- DONE_WITH_CONCERNS — single-instance plugin is wired and the second-launch callback focuses the existing main window without touching proxy lifecycle. Packaged second-launch smoke remains a replacement gate.

### 2026-05-22 — TDRP-040 packaged sidecar and Windows NSIS build

Evidence:

- Chose a first-class Tauri external binary sidecar instead of requiring a documented sibling CLI install.
- Added `apps/desktop/scripts/prepare-sidecar.mjs`:
  - builds `cargo build --release --bin codex-helper`;
  - infers or accepts the Tauri target triple;
  - copies the CLI to `apps/desktop/src-tauri/sidecars/codex-helper-$TARGET_TRIPLE(.exe)`.
- Added `apps/desktop/src-tauri/sidecars/.gitignore` so generated sidecar binaries are not committed.
- Updated `apps/desktop/src-tauri/tauri.conf.json`:
  - `bundle.active = true`;
  - Windows target is `nsis`;
  - `bundle.externalBin = ["sidecars/codex-helper"]`;
  - `beforeBuildCommand = "pnpm tauri:build:assets"`.
- Updated `start_desktop_proxy` sidecar lookup:
  - packaged resource directory sidecar first;
  - sibling development binary second;
  - `CODEX_HELPER_CLI_PATH` / legacy `CODEX_HELPER_CLI` only as developer fallback.
- Added Rust tests proving packaged sidecar lookup wins over env overrides and env lookup remains a final fallback.
- Added `docs/DESKTOP_RELEASE.md` with packaging contract, sidecar lookup order, and remaining release gates.

Verification:

- Command: `pnpm prepare:sidecar`
- Scope: `apps/desktop`
- Result: PASS — release CLI was built and copied to `apps/desktop/src-tauri/sidecars/codex-helper-x86_64-pc-windows-msvc.exe`.

- Command: `pnpm tauri:build`
- Scope: `apps/desktop`
- Result: PASS — produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`.

- Command: `7z l target\release\bundle\nsis\codex-helper_0.16.0_x64-setup.exe`
- Scope: Windows NSIS installer contents.
- Result: PASS — installer lists `codex-helper-desktop.exe` and bundled `codex-helper.exe`.

- Command: `cargo nextest run -p codex-helper-desktop --lib control::tests::cli_resolution`
- Scope: deterministic sidecar resolution.
- Result: PASS — 2 tests.

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 15 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Full live packaged lifecycle smoke is still TDRP-080. It was not completed in this slice because the developer machine already had a live codex-helper runtime; replacement smoke must run in an isolated environment and must not stop or mutate the user's active local helper.
- Signing/notarization and updater posture remain TDRP-060.
- Launch-at-login remains TDRP-050.

Result:

- DONE_WITH_CONCERNS — Windows packaged sidecar/installer build is deterministic and verified at artifact/content level. Full interactive packaged runtime smoke remains required before any egui replacement claim.

### 2026-05-22 — TDRP-050 launch at login

Evidence:

- Added the official Tauri autostart plugin:
  - Rust: `tauri-plugin-autostart` registered in `apps/desktop/src-tauri/src/lib.rs`;
  - frontend: `@tauri-apps/plugin-autostart` guest binding used by `apps/desktop/src/lib/tauri/commands.ts`.
- Added a TanStack Query-backed Settings hook for reading and changing launch-at-login state.
- Replaced the previous inert Settings "开机启动" row with a real switch that calls the autostart plugin and reports success or failure through the existing desktop action banner.
- Kept "启动时自动启动本地代理" disabled with explicit conservative copy. Launch-at-login starts the desktop companion only; it does not automatically stop, restart, or seize an already-running local proxy.
- Added a frontend test proving the Settings switch calls the real autostart guest binding.

Review:

- Workstream compliance: PASS_WITH_CONCERNS — the UI no longer advertises a fake launch-at-login toggle. The OS integration is real for Tauri desktop platforms supported by the plugin. Manual packaged login-item verification still belongs to TDRP-080.
- Code quality: PASS — autostart state is managed through the existing React Query pattern and isolated in `settings/hooks.ts` plus Tauri command wrappers.
- Safety: PASS — validation did not start, stop, detach, or mutate the developer machine's active codex-helper runtime. Unit tests mock the plugin guest binding and do not touch OS login items.

Verification:

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 24 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 15 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Manual packaged OS verification remains TDRP-080. The expected smoke is: enable launch-at-login in the packaged app, confirm the OS login entry is created, restart/login in an isolated environment, confirm the desktop companion starts without auto-stopping an existing proxy, then disable and confirm the OS login entry is removed.

Result:

- DONE_WITH_CONCERNS — launch-at-login is implemented through a real Tauri plugin and verified at compile/test level. Packaged OS smoke remains required before egui replacement.

### 2026-05-22 — TDRP-060 signing, installer, and update posture

Evidence:

- Reviewed the Tauri updater requirements and chose not to enable the updater plugin for this slice.
- Documented the release policy in `docs/DESKTOP_RELEASE.md`:
  - first replacement release uses GitHub Releases with manual installer download;
  - updater remains disabled until a signing keypair, CI-held private key, embedded public key, HTTPS endpoint, updater artifacts, and rollback/revocation story exist;
  - future implementation checklist records the minimum safe path to enable updates.
- Updated Settings "关于与路径" so update checking is not a fake clickable action:
  - the button is disabled;
  - UI copy says auto-update is not enabled and lists the missing release prerequisites.
- Updated README/README_EN/CHANGELOG to avoid promising automatic updates for the first replacement release.
- Added a frontend route test proving the Settings update control is disabled and carries honest signing/release-hosting copy.

Review:

- Workstream compliance: PASS_WITH_CONCERNS — TDRP-060 allows an explicit deferral when signing/artifact hosting decisions are not ready. This slice defines the posture and removes misleading update UI.
- Code quality: PASS — no updater dependency or placeholder command was added, so the desktop app cannot claim an unverified update path.
- Safety: PASS — validation did not start, stop, detach, install, or mutate the developer machine's active codex-helper runtime.

Verification:

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 25 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 15 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Real auto-update implementation remains a future release-operations task. It must include key generation/escrow, CI secret setup, public key config, HTTPS endpoint metadata, updater artifacts/signatures for every target, and signed update smoke from N-1 to N before the Settings button can be enabled.

Result:

- DONE_WITH_CONCERNS — signing/update posture is explicit and safe for the first replacement release. Auto-update remains disabled until the signed release pipeline exists.

### 2026-05-22 — TDRP-070 provider common edit parity

Evidence:

- Added `apps/desktop/src-tauri/src/commands/providers.rs` with a `save_common_provider` Tauri command.
- The command edits only safe common fields:
  - `alias`;
  - single-endpoint `base_url`;
  - `enabled`;
  - optional `auth_token_env` and `api_key_env` env var names.
- The command rejects unsupported provider shapes instead of flattening them:
  - v5 route graph config is required;
  - providers with multiple endpoints remain raw TOML only;
  - providers that mix provider-level `base_url` with endpoint tables are treated as advanced TOML.
- The command writes a timestamped backup before replacing `config.toml`.
- Added Rust tests proving:
  - single-provider common edits update the intended fields;
  - advanced provider fields such as `supported_models`, `tags`, and `limits` are preserved;
  - multi-endpoint providers are rejected without overwriting the config;
  - non-http(s) base URLs are rejected.
- Extended provider read-model mapping so cards carry `id`, `baseUrl`, `enabled`, endpoint count, endpoint name, editability, and explicit raw-TOML blocking copy.
- Added a Provider card inline edit form with Zod validation.
- Added frontend route tests proving:
  - a single-endpoint provider invokes `save_common_provider` with the safe field payload;
  - multi-endpoint providers show raw TOML copy and disable the common edit action.

Review:

- Workstream compliance: PASS_WITH_CONCERNS — TDRP-070 is complete for safe single-endpoint provider edits. Complex multi-endpoint editing is intentionally not claimed and remains raw TOML.
- Code quality: PASS — the Rust patch path mutates the parsed TOML value surgically and validates the resulting v5 config before writing. It avoids regenerating provider sections from the UI shape.
- Safety: PASS — validation did not start, stop, detach, reload, or mutate the developer machine's active codex-helper runtime. File-writing behavior is covered by isolated temp-file tests only.

Verification:

- Command: `cargo nextest run -p codex-helper-desktop commands::providers`
- Scope: TDRP-070 Rust provider config patch tests.
- Result: PASS — 4 tests.

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 27 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS — 18 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

Deferred:

- Provider creation, deletion, multi-endpoint editing, route graph editing, inline secret editing, and rich model-mapping editors remain raw TOML or future tasks. This is intentional so the desktop app does not become a heavy config manager.
- Packaged UI smoke for provider editing remains part of TDRP-080.

Result:

- DONE_WITH_CONCERNS — common provider edit parity is implemented and verified for single-endpoint providers while preserving advanced TOML fields. Multi-endpoint providers remain explicitly advanced/raw TOML.

### 2026-05-22 — TDRP-080 packaged lifecycle smoke partial automation

Evidence:

- Added a repeatable Windows smoke runner:
  - `docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1`
  - The script installs the NSIS artifact into a temporary directory.
  - It uses an isolated `CODEX_HELPER_HOME` and `CODEX_HELPER_DESKTOP_ADMIN_URL=http://127.0.0.1:6211`.
  - It clears `CODEX_HELPER_CLI_PATH` and legacy `CODEX_HELPER_CLI` so the packaged installation cannot rely on developer env CLI overrides.
  - It only stops the desktop process it started from the temporary install directory.
- The first UI Automation approach was abandoned because broad Windows UIA/WebView/system-tray tree scans can hang on this machine.
- The smoke runner was changed to a Win32 window-message strategy for stable non-invasive verification.

Verification:

- Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1
```

- Result: PASS for the automated subset.
- Output:

```json
{
  "timestamp": "2026-05-22T14:22:17.6069332+08:00",
  "installer": "D:\\Projects\\rust\\codex-helper\\target\\release\\bundle\\nsis\\codex-helper_0.16.0_x64-setup.exe",
  "install_dir": "C:\\Users\\Administrator\\AppData\\Local\\Temp\\codex-helper-tdrp-080-install",
  "smoke_home": "C:\\Users\\ADMINI~1\\AppData\\Local\\Temp\\codex-helper-tdrp-080-home-15e2dafc52074ece9f8ed8c53267e541",
  "admin_url": "http://127.0.0.1:6211",
  "results": [
    {
      "name": "nsis-install",
      "passed": true,
      "detail": "exit=0; install=C:\\Users\\Administrator\\AppData\\Local\\Temp\\codex-helper-tdrp-080-install"
    },
    {
      "name": "packaged-files",
      "passed": true,
      "detail": "desktop=True; sidecar=True"
    },
    {
      "name": "packaged-window-start",
      "passed": true,
      "detail": "pid=24144; hwnd=987176; visible=True"
    },
    {
      "name": "close-hides-to-tray",
      "passed": true,
      "detail": "alive=True; hwnd_after_close=987176; visible_after_close=False"
    },
    {
      "name": "second-launch-focuses-existing-window",
      "passed": true,
      "detail": "second_pid=53940; second_exited=True; first_alive=True; hwnd=987176; visible=True"
    }
  ],
  "passed": true
}
```

Claims proven by this run:

- The Windows NSIS installer can install into an isolated temporary directory.
- The installed directory contains both `codex-helper-desktop.exe` and bundled `codex-helper.exe`.
- The installed desktop app starts and creates the main native window.
- A native `WM_CLOSE` request hides the packaged main window instead of exiting the desktop process.
- A second launch exits/focuses the already-running packaged instance and makes the hidden main window visible again.
- The smoke used a separate `CODEX_HELPER_HOME` and admin URL and did not touch the user's active `ch.exe` process.

Not yet proven by this partial automation:

- Real tray menu click paths: Show Window, Hide to Tray, Quit App.
- Start Proxy through the packaged UI and desktop-managed sidecar startup without developer CLI env overrides.
- Detach and explicit Stop Proxy behavior in the packaged UI.
- Packaged config export/import file-dialog behavior.
- Packaged provider edit UI behavior.
- Launch-at-login enable/disable and login/restart behavior.

Result:

- PARTIAL — TDRP-080 is not complete. The automated Windows smoke now proves the installer, installed sidecar presence, close-to-tray native window behavior, and packaged single-instance focus path. Remaining TDRP-080 items still need stronger UI automation or manual smoke evidence before egui replacement claims.

### 2026-05-22 — TDRP-080 expanded packaged smoke automation

Evidence:

- Reworked `docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1` to:
  - pick a free proxy/admin port pair when no explicit `AdminUrl` is provided;
  - isolate both `CODEX_HELPER_HOME` and `CODEX_HOME` into temporary directories;
  - write a valid `version = 5` smoke config with a standalone provider and `manual-sticky` route target;
  - clear `CODEX_HELPER_CLI_PATH` and legacy `CODEX_HELPER_CLI` so packaged smoke cannot rely on development CLI overrides;
  - run packaged CDP smoke for `get_app_metadata`, `get_known_paths`, optional OS autostart registration, `export_config`, `import_config`, `start_desktop_proxy`, packaged Provider edit UI, `hide_main_window`, second-launch focus, `stop_proxy`, and `quit_app`.
- The expanded smoke now runs against a temporary install and a temporary Codex home without colliding with the developer machine's existing 3211/4211 runtime.
- The exported/imported config assertions now read the Tauri command results using their actual camelCase field names (`secretWarning`, `connectionMode`, `adminBaseUrl`).
- Added `apps/desktop/src-tauri/capabilities/default.json` because the packaged autostart plugin commands were otherwise rejected by Tauri v2 ACL. The capability grants the main window `core:default`, `autostart:default`, `dialog:default`, and `opener:default`.

Verification:

- Command: `pnpm tauri:build` from `apps/desktop`
- Result: PASS — rebuilt the Windows NSIS installer from the current desktop frontend and packaged sidecar.

- Command: `powershell -NoProfile -ExecutionPolicy Bypass -File docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1`
- Result: PASS — the full expanded automated packaged smoke passed.

- Command: `powershell -NoProfile -ExecutionPolicy Bypass -File docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1 -RunAutostartSmoke`
- Result: PASS — the packaged app enabled the Windows HKCU Run login item for the temporary install, `is_enabled` reported true, disable removed it, and cleanup left no codex-helper Run entry behind.

Claims proven by this run:

- the installed NSIS app can start a desktop-managed sidecar from the packaged binary without `CODEX_HELPER_CLI_PATH`;
- config export/import keeps the secret warning and timestamped backup behavior intact;
- packaged detach/stop/quit lifecycle commands work in the installed app;
- packaged Provider common edit UI can open the Relay Smoke edit form, save alias/base URL/auth env changes through the UI, and the isolated `config.toml` contains the edited values;
- packaged launch-at-login can register and unregister the installed desktop executable through the real autostart plugin and Windows HKCU Run key;
- the smoke no longer depends on the developer machine's fixed port assignments or live Codex home.

Not yet proven:

- real tray menu click paths Show Window / Hide to Tray / Quit App;

Result:

- PARTIAL — the automated smoke is much stronger now, but TDRP-080 still remains open until the real tray click paths are proven.

### 2026-05-23 — TDRP-080 packaged lifecycle smoke complete

Evidence:

- Fixed the packaged tray lifetime by retaining the Tauri `TrayIcon` handle in managed state.
- Extended `docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1` to prove the native tray menu paths:
  - locate the packaged app's Windows notification icon by desktop process id and `Shell_NotifyIconGetRect`;
  - open the same `tray-icon` 0.23 right-click callback path used by the Shell;
  - select native tray menu items by keyboard navigation when the localized Windows menu does not expose UIA item names;
  - assert the authoritative post-conditions for Show Window, Hide to Tray, and Quit App.
- Rebuilt the Windows NSIS installer after the tray retention fix, so the smoke ran against the current packaged binary.

Verification:

- Command: `cargo fmt --check`
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Result: PASS — 19 tests.

- Command: `pnpm tauri:build` from `apps/desktop`
- Result: PASS — rebuilt `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe` from the current desktop frontend, Tauri crate, and packaged sidecar.

- Command: `powershell -NoProfile -ExecutionPolicy Bypass -File docs\workstreams\tauri-desktop-replacement-parity\scripts\tdrp_080_packaged_smoke.ps1 -SkipDevToolsSmoke`
- Result: PASS — the isolated installed app proved native close-to-tray, second launch restore, tray Show Window, and tray Hide to Tray.

- Command: `powershell -NoProfile -ExecutionPolicy Bypass -File docs\workstreams\tauri-desktop-replacement-parity\scripts\tdrp_080_packaged_smoke.ps1`
- Result: PASS — full packaged smoke passed, including installed sidecar startup, known paths, config export/import, Provider edit UI, detach, second-launch restore, owned Stop Proxy, and tray Quit App leaving the sidecar running.

- Command: `powershell -NoProfile -ExecutionPolicy Bypass -File docs\workstreams\tauri-desktop-replacement-parity\scripts\tdrp_080_packaged_smoke.ps1 -RunAutostartSmoke`
- Result: PASS — full packaged smoke plus real Windows HKCU Run registration/unregistration passed and cleanup restored the temporary login item.

- Command: `python -m json.tool docs\workstreams\tauri-desktop-replacement-parity\WORKSTREAM.json`
- Result: PASS.

- Command: `git diff --check -- .`
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Claims proven:

- Windows NSIS packaged Tauri can replace the egui GUI for the covered Windows desktop lifecycle gates.
- The packaged app starts its bundled sidecar without development CLI overrides.
- Native close hides to tray; tray Show Window, Hide to Tray, and Quit App are proven through the installed app's native notification icon/menu path.
- Quit App exits only the desktop process and leaves the restarted sidecar reachable.
- Detach hides the desktop without stopping the sidecar, and explicit Stop Proxy stops only after the owned confirmation path.
- Config export/import, known path resolution, Provider common edit UI, and launch-at-login registration work in an isolated packaged environment.

Remaining concerns:

- The smoke is Windows-specific. macOS/Linux packaged parity should be split into platform follow-ons before making cross-platform replacement claims.
- TDRP-090 still needs README/CHANGELOG/release-note updates and the egui deprecation/removal decision.

Result:

- DONE_WITH_CONCERNS — TDRP-080 is complete for the Windows replacement gate. Proceed to TDRP-090.

### 2026-05-23 — TDRP-090 replacement docs and egui deprecation

Evidence:

- Updated `README.md` and `README_EN.md` so user-facing install/runtime/GUI sections describe the Windows NSIS packaged Tauri app as the verified desktop GUI replacement path.
- Updated `CHANGELOG.md` Unreleased notes so the release summary no longer calls the Tauri desktop only source-preview/internal dogfood, while still limiting the replacement claim to Windows packaged evidence.
- Updated `docs/DESKTOP_RELEASE.md` to move full packaged lifecycle smoke from "still required before egui replacement" into the Windows verified list, and to keep macOS/Linux packaged parity plus signed auto-update as follow-ons.
- Chose deprecation rather than removal for `codex-helper-gui`/egui:
  - Windows packaged Tauri parity is green.
  - macOS/Linux packaged parity is not yet proven.
  - keeping egui preserves rollback and non-Windows fallback behavior.
- Added a visible-console runtime warning to `src/bin/codex-helper-gui.rs` marking egui as deprecated and pointing Windows users at the Tauri desktop replacement path.

Verification:

- Command: `cargo fmt --check`
- Result: PASS.

- Command: `cargo check --features gui --bin codex-helper-gui`
- Result: PASS.

- Command: `python -m json.tool docs\workstreams\tauri-desktop-replacement-parity\WORKSTREAM.json`
- Result: PASS.

- Command: `git diff --check -- .`
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Claims proven:

- The user-facing docs and release notes now match the TDRP-080 evidence: Windows packaged Tauri is the verified desktop GUI replacement path.
- `codex-helper-gui`/egui is explicitly deprecated but retained as a legacy fallback.
- The docs do not claim macOS/Linux packaged replacement parity or automatic updates before evidence exists.

Remaining concerns:

- TDRP-100 still needs final review/verification closeout across the whole lane.
- macOS/Linux packaged lifecycle smoke remains a future platform follow-on.
- Signed updater release operations remain a future release follow-on before enabling auto-update.

Result:

- DONE_WITH_CONCERNS — TDRP-090 is complete. Proceed to TDRP-100 final closeout.

### 2026-05-23 — TDRP-100 final closeout

Review:

- Workstream compliance: PASS_WITH_CONCERNS.
  - All TODO tasks are complete.
  - The Windows packaged replacement claim is backed by TDRP-080 smoke evidence.
  - TDRP-090 docs match the evidence and do not overclaim macOS/Linux or auto-update readiness.
  - Residual concerns are follow-ons rather than blockers for the Windows packaged replacement scope.
- Code quality: PASS.
  - The tray fix retains the `TrayIcon` handle in Tauri managed state and does not change proxy lifecycle semantics.
  - The smoke script runs in isolated homes, clears developer CLI overrides, uses free ports by default, and validates post-conditions instead of assuming UI clicks succeeded.
  - The egui change is a bounded deprecation warning and does not alter GUI startup/error behavior.
- Missing gates: NONE for the Windows packaged replacement scope.

Verification:

- Command: `cargo fmt --check`
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Result: PASS — 19 tests. Nextest run id: `266a86a3-250b-4ac1-a09f-e4d0b5861e15`.

- Command: `cargo check --features gui --bin codex-helper-gui`
- Result: PASS.

- Command: `python -m json.tool docs\workstreams\tauri-desktop-replacement-parity\WORKSTREAM.json`
- Result: PASS.

- Command: `git diff --check -- .`
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Gates not rerun:

- `pnpm tauri:build` and full packaged smoke were not rerun during TDRP-100 because no Tauri runtime/frontend code changed after the TDRP-080 packaged smoke pass. The current closeout reused the same-day TDRP-080 packaged evidence and added fresh Rust/docs verification for the final diff.
- `pnpm test` / `pnpm build` were not rerun because TDRP-090/TDRP-100 did not change frontend source files.

Final status:

- CLOSED_WITH_CONCERNS for the Windows packaged replacement scope.
- Follow-ons:
  - macOS/Linux packaged lifecycle smoke before cross-platform GUI replacement claims;
  - signed updater release pipeline before enabling auto-update;
  - eventual `codex-helper-gui`/egui removal after rollback and non-Windows fallback policy is settled.
