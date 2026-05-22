# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-050
- Owner: main
- Files:
  - `apps/desktop/src-tauri/Cargo.toml`
  - `apps/desktop/src-tauri/tauri.conf.json`
  - `apps/desktop/src-tauri/src/`
  - `apps/desktop/src/features/settings/`
- Validation:
  - Tauri compile/tests
  - honest UI copy; no working launch-at-login toggle unless the OS integration is real
- Status: READY
- Review: Launch-at-login must be either implemented through a real Tauri plugin or explicitly marked unsupported/deferred for the first replacement release.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.
- TDRP-040 is implemented with concerns: Windows NSIS packaging now includes a Tauri external binary sidecar. `pnpm tauri:build` produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`, and `7z l` confirmed both `codex-helper-desktop.exe` and bundled `codex-helper.exe`. Full live packaged lifecycle smoke remains TDRP-080 because the developer machine already had a live codex-helper runtime and must not be disturbed.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-050:

1. Decide whether to add `tauri-plugin-autostart` now or explicitly defer launch-at-login for the first replacement release.
2. If implemented, add Settings UI backed by real Tauri commands and compile/tests.
3. If deferred, show honest disabled UI copy and record the release limitation.
4. Do not run live runtime start/stop smoke against the developer machine's active codex-helper process.
