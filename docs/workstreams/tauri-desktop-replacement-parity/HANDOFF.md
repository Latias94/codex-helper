# Tauri Desktop Replacement Parity — Handoff

Status: Draft
Last updated: 2026-05-22

## Current State

This lane is newly opened from the closed `tauri-desktop-client` readiness lane. The goal is no longer just to document readiness; it is to close the parity gaps required for Tauri to replace egui.

The parent readiness report says Tauri is source-preview/internal-dogfood ready, but not egui-removal ready. This lane owns the remaining replacement gates.

## Active Task

- Task ID: TDRP-040
- Owner: main
- Files:
  - `apps/desktop/src-tauri/Cargo.toml`
  - `apps/desktop/src-tauri/tauri.conf.json`
  - packaged smoke/release docs
- Validation:
  - `pnpm tauri:build` or documented packaging blocker
  - packaged app smoke on Windows at minimum
- Status: READY
- Review: Sidecar lookup must be deterministic and documented; no hidden dependence on developer shell env.
- Evidence: update `EVIDENCE_AND_GATES.md`.

## Decisions Since Last Update

- Start with local desktop parity primitives because they are useful immediately and reduce replacement risk without waiting for installer work.
- Packaged sidecar remains the highest-risk release gate and follows after the first primitives.
- TDRP-020 is implemented: Settings can open known paths and can export/import the single primary config with validation, backup, and secret warning. It uses `tauri-plugin-dialog` for file pickers and `tauri-plugin-opener` for opening paths.
- TDRP-030 is implemented: `tauri-plugin-single-instance` is registered and second launch focuses/restores the existing main window without touching proxy lifecycle.

## Blockers

- None yet.

## Next Recommended Action

Implement TDRP-040:

1. Decide whether the bundled app ships `codex-helper` as a Tauri sidecar or requires a documented sibling CLI installation.
2. Make `start_desktop_proxy` use the packaged strategy before falling back to developer env lookup.
3. Run `pnpm tauri:build` or document/fix the exact packaging blocker.
4. Record packaged smoke evidence before claiming replacement parity.
