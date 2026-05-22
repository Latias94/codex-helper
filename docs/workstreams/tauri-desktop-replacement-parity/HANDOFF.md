# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-060
- Owner: main
- Files:
  - `apps/desktop/src-tauri/tauri.conf.json`
  - `docs/DESKTOP_RELEASE.md`
  - `README.md`
  - `README_EN.md`
  - `CHANGELOG.md`
- Validation:
  - docs review
  - if updater is implemented, signed/dev artifact updater smoke
- Status: READY
- Review: Do not ship auto-update copy without signature/private-key, release artifact hosting, and rollback posture.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.
- TDRP-040 is implemented with concerns: Windows NSIS packaging now includes a Tauri external binary sidecar. `pnpm tauri:build` produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`, and `7z l` confirmed both `codex-helper-desktop.exe` and bundled `codex-helper.exe`. Full live packaged lifecycle smoke remains TDRP-080 because the developer machine already had a live codex-helper runtime and must not be disturbed.
- TDRP-050 is implemented with concerns: `tauri-plugin-autostart` is registered, Settings uses the real `@tauri-apps/plugin-autostart` guest binding, and frontend tests prove the switch calls the plugin. Manual packaged login-item smoke remains TDRP-080.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-060:

1. Define signing, installer, release channel, and auto-update posture in `docs/DESKTOP_RELEASE.md`.
2. Implement updater only if signing/private-key and artifact hosting decisions are ready; otherwise explicitly defer auto-update for the first replacement release with honest UI copy.
3. Update README/CHANGELOG wording so users do not expect auto-update until the signing/update gate is real.
4. Do not run live runtime start/stop smoke against the developer machine's active codex-helper process.
