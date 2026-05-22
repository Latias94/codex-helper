# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-070
- Owner: main
- Files:
  - `apps/desktop/src-tauri/src/commands/`
  - `apps/desktop/src/features/providers/`
  - `apps/desktop/src/lib/api/`
  - `apps/desktop/src/app/App.test.tsx`
- Validation:
  - frontend form tests
  - Rust config patch tests
  - desktop build/check gates
- Status: READY
- Review: Provider save must preserve unknown advanced TOML fields and avoid pretending complex multi-endpoint editing is solved.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.
- TDRP-040 is implemented with concerns: Windows NSIS packaging now includes a Tauri external binary sidecar. `pnpm tauri:build` produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`, and `7z l` confirmed both `codex-helper-desktop.exe` and bundled `codex-helper.exe`. Full live packaged lifecycle smoke remains TDRP-080 because the developer machine already had a live codex-helper runtime and must not be disturbed.
- TDRP-050 is implemented with concerns: `tauri-plugin-autostart` is registered, Settings uses the real `@tauri-apps/plugin-autostart` guest binding, and frontend tests prove the switch calls the plugin. Manual packaged login-item smoke remains TDRP-080.
- TDRP-060 is implemented with concerns: the first replacement release uses manual GitHub Releases installer downloads; auto-update remains disabled until Tauri updater signing keys, HTTPS release endpoint, artifact hosting, and rollback operations are real. Settings shows disabled honest update copy.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-070:

1. Inspect the current provider read model, config schema, and existing provider UI.
2. Add common single-endpoint provider edit forms for safe fields only.
3. Implement Rust config patching so unknown advanced provider fields are preserved.
4. Keep complex multi-endpoint/raw TOML editing explicitly advanced/deferred.
5. Do not run live runtime start/stop smoke against the developer machine's active codex-helper process.
