# Tauri Desktop Replacement Parity — Handoff

Status: Complete
Last updated: 2026-05-23

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Closeout

- Closed task: TDRP-100
- Owner: main
- Status: DONE_WITH_CONCERNS
- Review: no blocking findings for the Windows packaged replacement scope.
- Evidence: `EVIDENCE_AND_GATES.md` records fresh closeout verification.
- Residual concerns:
  - macOS/Linux packaged lifecycle smoke is not yet proven.
  - signed updater release operations are not yet implemented.
  - `codex-helper-gui`/egui is deprecated but intentionally retained until rollback/non-Windows policy supports removal.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.
- TDRP-040 is implemented with concerns: Windows NSIS packaging now includes a Tauri external binary sidecar. `pnpm tauri:build` produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`, and `7z l` confirmed both `codex-helper-desktop.exe` and bundled `codex-helper.exe`. Full live packaged lifecycle smoke remains TDRP-080 because the developer machine already had a live codex-helper runtime and must not be disturbed.
- TDRP-050 is implemented with concerns: `tauri-plugin-autostart` is registered, Settings uses the real `@tauri-apps/plugin-autostart` guest binding, and frontend tests prove the switch calls the plugin. Manual packaged login-item smoke remains TDRP-080.
- TDRP-060 is implemented with concerns: the first replacement release uses manual GitHub Releases installer downloads; auto-update remains disabled until Tauri updater signing keys, HTTPS release endpoint, artifact hosting, and rollback operations are real. Settings shows disabled honest update copy.
- TDRP-070 is implemented with concerns: single-endpoint provider common edits are available through a validated form and `save_common_provider`; Rust config patch tests prove advanced TOML fields are preserved and multi-endpoint providers are rejected without overwriting. Complex provider editing remains raw TOML.
- TDRP-080 is implemented and verified on Windows: `scripts/tdrp_080_packaged_smoke.ps1` installs the NSIS artifact into a temporary directory, verifies the installed desktop exe plus bundled sidecar, starts the packaged app, proves native close hides to tray, proves second launch focuses/restores the existing packaged window, proves native tray menu Show Window / Hide to Tray / Quit App, drives Tauri commands through WebView2 DevTools/CDP, validates packaged known paths plus config export/import, starts the packaged desktop-managed sidecar without developer CLI env overrides, proves packaged Provider common edit UI writes alias/base URL/auth env changes to the isolated config, verifies packaged autostart enable/disable against the Windows HKCU Run key when `-RunAutostartSmoke` is supplied, hides/detaches while leaving the sidecar alive, explicitly stops the owned sidecar, restarts it, and verifies tray Quit App exits the desktop while leaving the sidecar running.
- The smoke script now auto-selects a free proxy/admin port pair when `-AdminUrl` is omitted and isolates both `CODEX_HELPER_HOME` and `CODEX_HOME`, so it does not mutate the user's active `~/.codex` files or fixed 3211/4211 runtime.
- Added `apps/desktop/src-tauri/capabilities/default.json` after packaged autostart commands were rejected by Tauri v2 ACL; the main window now has `autostart:default`, `dialog:default`, `opener:default`, and `core:default`.
- The desktop now retains the Tauri tray icon handle in managed state, and the smoke uses `Shell_NotifyIconGetRect` against the packaged desktop process plus the `tray-icon` 0.23 right-click callback to open the native tray menu without relying on localized Windows tray UI names.
- TDRP-090 is implemented: README/README_EN/CHANGELOG/docs/DESKTOP_RELEASE now present Tauri as the verified Windows packaged GUI replacement path, keep macOS/Linux packaged parity and signed auto-update as follow-ons, and mark `codex-helper-gui`/egui as a deprecated legacy fallback. The egui binary prints a deprecation warning when launched from a visible console.

## Blockers

- None yet.

## Follow-Ons

1. Open platform follow-ons for macOS/Linux packaged lifecycle smoke before cross-platform GUI replacement claims.
2. Build the signed updater release pipeline: key escrow, CI secrets, HTTPS metadata, signed artifacts, N-1 to N smoke, and rollback instructions.
3. Decide the removal window for `codex-helper-gui`/egui after fallback and rollback policy is settled.
