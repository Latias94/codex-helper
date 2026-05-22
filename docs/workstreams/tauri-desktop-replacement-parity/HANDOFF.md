# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-080
- Owner: main
- Files:
  - `apps/desktop/src-tauri/`
  - `apps/desktop/src/`
  - `docs/workstreams/tauri-desktop-replacement-parity/`
- Validation:
  - packaged lifecycle smoke in an isolated environment
  - desktop build/check gates if fixes are needed
  - evidence updates
- Status: READY
- Review: Any failed smoke blocks egui removal. Do not disturb the developer machine's active codex-helper runtime.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.
- TDRP-040 is implemented with concerns: Windows NSIS packaging now includes a Tauri external binary sidecar. `pnpm tauri:build` produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`, and `7z l` confirmed both `codex-helper-desktop.exe` and bundled `codex-helper.exe`. Full live packaged lifecycle smoke remains TDRP-080 because the developer machine already had a live codex-helper runtime and must not be disturbed.
- TDRP-050 is implemented with concerns: `tauri-plugin-autostart` is registered, Settings uses the real `@tauri-apps/plugin-autostart` guest binding, and frontend tests prove the switch calls the plugin. Manual packaged login-item smoke remains TDRP-080.
- TDRP-060 is implemented with concerns: the first replacement release uses manual GitHub Releases installer downloads; auto-update remains disabled until Tauri updater signing keys, HTTPS release endpoint, artifact hosting, and rollback operations are real. Settings shows disabled honest update copy.
- TDRP-070 is implemented with concerns: single-endpoint provider common edits are available through a validated form and `save_common_provider`; Rust config patch tests prove advanced TOML fields are preserved and multi-endpoint providers are rejected without overwriting. Complex provider editing remains raw TOML.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-080:

1. Prepare an isolated packaged-smoke environment so the user's active local `codex-helper` runtime is not stopped, detached, reconfigured, or otherwise disturbed.
2. Rebuild/package only if current artifacts are stale.
3. Smoke close-to-tray, tray show/hide, Quit App, Detach, explicit Stop Proxy, attach existing runtime, packaged sidecar start, second-launch focus, Settings path/config actions, launch-at-login, and provider edit UI.
4. Record pass/fail evidence in `EVIDENCE_AND_GATES.md`.
5. Fix blocking failures or split OS-specific follow-ons before moving to docs/release/egui deprecation tasks.
