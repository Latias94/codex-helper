# Tauri Desktop Replacement Parity — TODO

Status: Draft
Last updated: 2026-05-22

## M0 — Scope And Evidence Freeze

- [x] TDRP-010 [owner=planner] [deps=none] [scope=docs/workstreams/tauri-desktop-replacement-parity]
  Goal: Open the replacement parity lane from the closed readiness report and user goal.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Evidence: `docs/workstreams/tauri-desktop-replacement-parity/DESIGN.md`
  Handoff: DONE — begin with TDRP-020 because it gives immediate desktop parity without waiting for installer work.

## M1 — Local Desktop Parity Primitives

- [x] TDRP-020 [owner=main] [deps=TDRP-010] [scope=apps/desktop/src-tauri,apps/desktop/src]
  Goal: Implement first-class Settings actions for opening config/log/cache paths and lightweight single-config export/import with parse validation, timestamped backup, and secret warnings.
  Validation: DONE — `pnpm test`, `pnpm build`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, and `cargo nextest run -p codex-helper-desktop --lib` pass; final diff hygiene is recorded in EVIDENCE_AND_GATES.md.
  Review: DONE — import/export is intentionally single-config only; no heavy profile/workspace/config catalog was introduced.
  Evidence: `apps/desktop/src-tauri/src/commands/paths.rs`; `apps/desktop/src/features/settings/SettingsPage.tsx`; `apps/desktop/src/app/App.test.tsx`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — UI copy warns that exported TOML may contain inline secrets. File picker UX now depends on `@tauri-apps/plugin-dialog`; packaged validation remains TDRP-040/TDRP-080.

- [x] TDRP-030 [owner=main] [deps=TDRP-010] [scope=apps/desktop/src-tauri]
  Goal: Add single-instance behavior so a second launch focuses/restores the existing main window instead of spawning another desktop controller.
  Validation: DONE — `cargo check -p codex-helper-desktop` and targeted lifecycle test passed; full gate recorded in EVIDENCE_AND_GATES.md.
  Review: DONE_WITH_CONCERNS — callback shows/unminimizes/focuses the main window and does not touch proxy lifecycle. Packaged second-launch smoke remains TDRP-080.
  Evidence: `apps/desktop/src-tauri/src/lib.rs`; `apps/desktop/src-tauri/src/lifecycle.rs`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — required code path is installed; packaged behavior still needs real smoke before egui replacement claim.

## M2 — Packaged Runtime And OS Integration

- [x] TDRP-040 [owner=main] [deps=TDRP-020,TDRP-030] [scope=apps/desktop/src-tauri,apps/desktop/src-tauri/tauri.conf.json,release-docs]
  Goal: Decide and implement packaged sidecar/installer strategy so installed Tauri can start or attach to codex-helper without manual `CODEX_HELPER_CLI_PATH`.
  Validation: DONE_WITH_CONCERNS — `pnpm tauri:build` produced the Windows NSIS installer and `7z l` confirmed `codex-helper-desktop.exe` plus bundled `codex-helper.exe`; full live packaged lifecycle smoke is deferred to TDRP-080 because the developer machine already had a running codex-helper instance and must not be disturbed.
  Review: DONE — packaged resource sidecar lookup is first, sibling development lookup is second, and `CODEX_HELPER_CLI_PATH`/legacy env lookup is only a developer fallback.
  Evidence: `apps/desktop/src-tauri/tauri.conf.json`; `apps/desktop/scripts/prepare-sidecar.mjs`; `apps/desktop/src-tauri/src/commands/control.rs`; `docs/DESKTOP_RELEASE.md`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — Windows packaging gate is green; installer signing/notarization and full interactive lifecycle smoke remain TDRP-060/TDRP-080.

- [ ] TDRP-050 [owner=main] [deps=TDRP-040] [scope=apps/desktop/src-tauri,apps/desktop/src]
  Goal: Implement launch-at-login setting or explicitly mark it unsupported for the first release with honest UI copy.
  Validation: Tauri command tests/compile plus manual packaged OS verification where supported.
  Review: UI must not show a working toggle until the OS integration is real.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: Record per-OS behavior.

- [ ] TDRP-060 [owner=planner-or-main] [deps=TDRP-040] [scope=release-docs,apps/desktop/src-tauri]
  Goal: Define signing, installer, and auto-update/release-channel posture; implement updater only if signing/artifact hosting decisions are ready.
  Validation: docs review; if implemented, updater smoke on signed/dev artifacts.
  Review: Do not ship auto-update copy without signature/private-key and rollback story.
  Evidence: release notes, EVIDENCE_AND_GATES.md.
  Handoff: May close as explicit deferral if the first replacement release intentionally excludes auto-update.

## M3 — Provider Edit Parity And Packaged Smoke

- [ ] TDRP-070 [owner=main] [deps=TDRP-020] [scope=apps/desktop/src-tauri,apps/desktop/src/features/providers]
  Goal: Add common provider edit forms for single-endpoint providers with validation and advanced TOML preservation.
  Validation: frontend form tests, Rust config patch tests, build/check gates.
  Review: Saving common fields must not silently drop unknown advanced provider fields.
  Evidence: tests and EVIDENCE_AND_GATES.md.
  Handoff: Complex multi-endpoint editing can stay advanced/raw TOML if documented.

- [ ] TDRP-080 [owner=main] [deps=TDRP-040,TDRP-050,TDRP-060,TDRP-070] [scope=packaged-smoke,docs]
  Goal: Run and record full packaged desktop lifecycle smoke: close window, tray show/hide, Quit App, Detach, Stop Proxy, attach existing resident runtime, start packaged sidecar, second launch focus, config export/import.
  Validation: Smoke evidence in EVIDENCE_AND_GATES.md.
  Review: Any failed smoke blocks egui removal.
  Evidence: packaged smoke logs/screenshots/manual notes.
  Handoff: Fix failures or split OS-specific follow-ons.

## M4 — Replacement Release And egui Deprecation/Removal

- [ ] TDRP-090 [owner=planner] [deps=TDRP-080] [scope=README,CHANGELOG,release-notes,crates/gui-or-package-metadata]
  Goal: Update user-facing docs and release notes so Tauri is the GUI replacement path and egui is deprecated or removed behind verified gates.
  Validation: docs review plus build/package gates for any binary/package metadata changes.
  Review: Do not remove egui unless TDRP-080 is green; otherwise document deprecation only.
  Evidence: README/CHANGELOG/release notes and final gate log.
  Handoff: Communicate rollback/fallback behavior.

- [ ] TDRP-100 [owner=planner] [deps=TDRP-090] [scope=docs/workstreams/tauri-desktop-replacement-parity]
  Goal: Close this lane only after the full objective is verified against current evidence.
  Validation: verify-rust-workstream records fresh final evidence.
  Review: review-workstream has no blocking findings.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json.
  Handoff: Summarize remaining risks, if any.
