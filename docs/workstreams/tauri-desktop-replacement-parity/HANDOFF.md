# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-030
- Owner: main
- Files:
  - `apps/desktop/src-tauri/Cargo.toml`
  - `apps/desktop/src-tauri/src/lib.rs`
  - `apps/desktop/src-tauri/src/lifecycle.rs`
- Validation:
  - `cargo fmt --check`
  - `cargo check -p codex-helper-desktop`
  - `cargo nextest run -p codex-helper-desktop --lib`
  - documented dev/manual smoke for second launch behavior if the plugin cannot be unit-tested
- Status: READY
- Review: Single-instance callback must show/focus the main window and must not start a duplicate proxy.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-030:

1. Add Tauri single-instance plugin or equivalent native guard.
2. On second launch, show/unminimize/focus the existing main window.
3. Ensure the callback does not start or stop proxy runtimes.
4. Record fresh evidence and commit if clean.
