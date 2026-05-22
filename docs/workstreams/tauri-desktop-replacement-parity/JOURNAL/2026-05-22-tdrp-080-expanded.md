# 2026-05-22 — TDRP-080 expanded packaged smoke automation

## Scope

- Continue TDRP-080 packaged lifecycle smoke after the first partial Win32 window smoke.
- Diagnose why `start_desktop_proxy` reported that the admin API was not reachable within 5 seconds.
- Strengthen automation while keeping the user's active local helper runtime safe.

## Diagnosis

- Built a deterministic repro by manually starting the packaged `codex-helper.exe` from a temporary NSIS install.
- The first failure was not a slow startup: the smoke config was invalid because route node `relay` conflicted with provider `relay`.
- A second failure came from fixed smoke ports colliding with a leftover temporary runtime on `5211/6211`.
- During manual diagnosis, a temporary sidecar briefly inherited the real Codex home and switched `~/.codex/config.toml` to port `5211`. I stopped only that temporary sidecar and restored the real Codex proxy config to the user's active `3211` runtime.

## Changes

- Updated `scripts/tdrp_080_packaged_smoke.ps1` to:
  - choose a free proxy/admin port pair when `-AdminUrl` is omitted;
  - keep `-AdminUrl` available for explicit targeted smoke;
  - keep OS login-item mutation behind explicit `-RunAutostartSmoke`;
  - isolate both `CODEX_HELPER_HOME` and `CODEX_HOME`;
  - write a valid `version = 5` smoke config using provider `relay` plus route node `main`;
  - keep developer CLI env overrides cleared;
  - assert Tauri command results using camelCase field names from the packaged invoke bridge;
  - drive the packaged Provider common edit form through WebView2 DevTools/CDP and verify the isolated `config.toml` is updated.
- Added `apps/desktop/src-tauri/capabilities/default.json` after the packaged app rejected autostart commands with `Command plugin:autostart|is_enabled not allowed by ACL`.

## Verification

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\workstreams\tauri-desktop-replacement-parity\scripts\tdrp_080_packaged_smoke.ps1
```

Result: PASS.

Packaging command:

```powershell
cd apps/desktop
pnpm tauri:build
```

Result: PASS. This rebuilt the NSIS installer from the current frontend before the final smoke, so the packaged Provider edit UI was present in the installed artifact.

Autostart command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File docs\workstreams\tauri-desktop-replacement-parity\scripts\tdrp_080_packaged_smoke.ps1 -RunAutostartSmoke
```

Result: PASS. The packaged app registered `codex-helper` in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for the temporary install path, `is_enabled` returned true, disable returned the state to false, and cleanup removed the temporary Run entry.

Passed checks:

- NSIS install into a temporary install directory.
- Installed directory contains `codex-helper-desktop.exe` and bundled `codex-helper.exe`.
- Packaged window starts.
- `WM_CLOSE` hides to tray instead of exiting the desktop process.
- Second launch focuses/restores the existing packaged window.
- DevTools/CDP bridge can call Tauri commands.
- `get_known_paths` points at the isolated helper home.
- `export_config` creates the export and reports `secretWarning`.
- `import_config` validates TOML, creates a timestamped backup, and reports `secretWarning`.
- `start_desktop_proxy` starts the packaged sidecar without developer CLI env overrides and reaches desktop-owned mode.
- Provider edit UI opens for the single-endpoint `Relay Smoke` provider, saves alias/base URL/auth env changes, shows the success banner, and the isolated config contains the edited values.
- Launch-at-login uses the real packaged autostart plugin path and Windows login item registration. The smoke restores the Run key afterward.
- `hide_main_window` detaches while the sidecar stays reachable.
- Another launch restores the hidden desktop while the sidecar remains reachable.
- `stop_proxy` with `STOP OWNED PROXY` stops the owned sidecar.
- `quit_app` exits only the desktop process while leaving the restarted sidecar reachable.

## Safety check

- After the passing smoke, only the user's existing `ch.exe` runtime remained.
- `3211/4211` were still owned by that existing `ch.exe`.
- Real `~/.codex/config.toml` was restored to `base_url = "http://127.0.0.1:3211"`.

## Concerns / follow-up

- TDRP-080 still remains open.
- Not yet proven:
  - real tray menu Show Window / Hide to Tray / Quit App click paths;
- Avoid broad Windows UI Automation tree scans; they can hang on this machine.
