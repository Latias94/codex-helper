# 2026-05-23 — TDRP-080 tray menu completion

## Scope

- Finish the last open TDRP-080 packaged lifecycle gap: real tray menu Show Window / Hide to Tray / Quit App.
- Keep the smoke isolated from the user's active `codex-helper` runtime and real Codex home.

## Diagnosis

- The existing expanded packaged smoke already proved installer install, sidecar presence, close-to-tray, second-launch restore, DevTools command bridge, known paths, config export/import, Provider common edit UI, autostart registration, detach, Stop Proxy, and command-driven Quit App.
- The remaining gap was the native tray menu.
- Initial Windows tray enumeration could not find `codex-helper` by localized taskbar/overflow UI text.
- `Shell_NotifyIconGetRect` did find the packaged app's notification icon for the desktop process, proving the Shell icon existed even when Windows UI Automation did not expose a stable name.
- `tray-icon` 0.23 opens its Windows tray menu on the right-button-up callback, not the older right-button-down path.

## Changes

- Retained the Tauri `TrayIcon` handle in managed state in `apps/desktop/src-tauri/src/lifecycle.rs`.
- Extended `scripts/tdrp_080_packaged_smoke.ps1` to:
  - locate the packaged notification icon by process id through `Shell_NotifyIconGetRect`;
  - use the `tray-icon` 0.23 Shell callback path for the native tray menu;
  - use keyboard menu navigation as a fallback when localized native menus do not expose UIA names;
  - assert Show Window, Hide to Tray, and Quit App post-conditions.

## Verification

- `cargo fmt --check`: PASS.
- `cargo check -p codex-helper-desktop`: PASS.
- `cargo nextest run -p codex-helper-desktop --lib`: PASS, 19 tests.
- `pnpm tauri:build` from `apps/desktop`: PASS.
- `tdrp_080_packaged_smoke.ps1 -SkipDevToolsSmoke`: PASS.
- `tdrp_080_packaged_smoke.ps1`: PASS.
- `tdrp_080_packaged_smoke.ps1 -RunAutostartSmoke`: PASS.
- `python -m json.tool docs\workstreams\tauri-desktop-replacement-parity\WORKSTREAM.json`: PASS.
- `git diff --check -- .`: PASS, with only Windows LF/CRLF warnings.

## Result

- TDRP-080 is DONE_WITH_CONCERNS for the Windows packaged replacement gate.
- The concern is platform scope: macOS/Linux packaged parity still needs dedicated follow-ons before cross-platform replacement claims.
- Next task is TDRP-090: user-facing replacement docs/release notes and egui deprecation/removal decision.
