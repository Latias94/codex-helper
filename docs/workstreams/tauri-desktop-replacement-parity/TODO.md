# Tauri Desktop Replacement Parity — TODO

Status: Complete
Last updated: 2026-05-23

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

- [x] TDRP-050 [owner=main] [deps=TDRP-040] [scope=apps/desktop/src-tauri,apps/desktop/src]
  Goal: Implement launch-at-login setting or explicitly mark it unsupported for the first release with honest UI copy.
  Validation: DONE_WITH_CONCERNS — `tauri-plugin-autostart` is registered in the Tauri builder, Settings uses the real JS guest binding, frontend tests/build pass, and `cargo check -p codex-helper-desktop` plus `cargo nextest run -p codex-helper-desktop --lib` pass. Manual packaged OS verification remains part of TDRP-080.
  Review: DONE — UI now shows a working toggle only because the OS integration is real; startup-time proxy auto-start remains disabled with honest conservative copy.
  Evidence: `apps/desktop/src-tauri/src/lib.rs`; `apps/desktop/src/features/settings/SettingsPage.tsx`; `apps/desktop/src/app/App.test.tsx`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — plugin supports Windows/macOS/Linux desktop targets; Android/iOS are excluded by the plugin dependency. Packaged login-item smoke remains TDRP-080.

- [x] TDRP-060 [owner=planner-or-main] [deps=TDRP-040] [scope=release-docs,apps/desktop/src-tauri]
  Goal: Define signing, installer, and auto-update/release-channel posture; implement updater only if signing/artifact hosting decisions are ready.
  Validation: DONE_WITH_CONCERNS — release posture is documented; Settings shows a disabled honest update control; `pnpm test`, `pnpm build`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, `cargo nextest run -p codex-helper-desktop --lib`, and `git diff --check -- .` pass.
  Review: DONE — auto-update is intentionally deferred because there is no updater signing keypair, HTTPS endpoint, artifact hosting, or rollback process yet.
  Evidence: `docs/DESKTOP_RELEASE.md`; `apps/desktop/src/features/settings/SettingsPage.tsx`; `apps/desktop/src/app/App.test.tsx`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — first replacement release policy is manual GitHub Releases installer download; future updater work must complete signed artifact smoke before enabling the UI.

## M3 — Provider Edit Parity And Packaged Smoke

- [x] TDRP-070 [owner=main] [deps=TDRP-020] [scope=apps/desktop/src-tauri,apps/desktop/src/features/providers]
  Goal: Add common provider edit forms for single-endpoint providers with validation and advanced TOML preservation.
  Validation: DONE — `pnpm test`, `pnpm build`, `cargo check -p codex-helper-desktop`, `cargo nextest run -p codex-helper-desktop`, and `cargo fmt --check` pass. Final diff hygiene is recorded in EVIDENCE_AND_GATES.md.
  Review: Saving common fields must not silently drop unknown advanced provider fields.
  Evidence: `apps/desktop/src-tauri/src/commands/providers.rs`; `apps/desktop/src/features/providers/ProviderCard.tsx`; `apps/desktop/src/app/App.test.tsx`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE_WITH_CONCERNS — complex multi-endpoint editing stays advanced/raw TOML by design. Safe single-endpoint form editing is complete, but TDRP-080 still needs packaged smoke before any replacement claim.

- [x] TDRP-080 [owner=main] [deps=TDRP-040,TDRP-050,TDRP-060,TDRP-070] [scope=packaged-smoke,docs]
  Goal: Run and record full packaged desktop lifecycle smoke: close window, tray show/hide, Quit App, Detach, Stop Proxy, attach existing resident runtime, start packaged sidecar, second launch focus, config export/import.
  Validation: DONE — rebuilt the Windows NSIS installer, then `tdrp_080_packaged_smoke.ps1 -SkipDevToolsSmoke`, `tdrp_080_packaged_smoke.ps1`, and `tdrp_080_packaged_smoke.ps1 -RunAutostartSmoke` all passed against isolated install/config/Codex homes.
  Review: DONE_WITH_CONCERNS — Windows packaged parity is now proven, including real tray menu Show Window / Hide to Tray / Quit App via the native Shell notification icon callback. macOS/Linux packaged smoke remains a future platform expansion before claiming parity on those OSes.
  Evidence: `apps/desktop/src-tauri/src/lifecycle.rs`; `docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE — proceed to TDRP-090 user-facing replacement docs/release notes and egui deprecation/removal decision.

## M4 — Replacement Release And egui Deprecation/Removal

- [x] TDRP-090 [owner=planner] [deps=TDRP-080] [scope=README,CHANGELOG,release-notes,crates/gui-or-package-metadata]
  Goal: Update user-facing docs and release notes so Tauri is the GUI replacement path and egui is deprecated or removed behind verified gates.
  Validation: DONE — README/README_EN/CHANGELOG/docs/DESKTOP_RELEASE now describe Windows Tauri as the verified packaged replacement path, `codex-helper-gui`/egui as a deprecated legacy fallback, and auto-update/macOS/Linux packaged parity as follow-ons. `cargo fmt --check`, `cargo check --features gui --bin codex-helper-gui`, `python -m json.tool WORKSTREAM.json`, and `git diff --check -- .` pass.
  Review: DONE_WITH_CONCERNS — egui is deprecated but not removed because Windows packaged parity is green while macOS/Linux packaged parity and rollback policy still benefit from a retained fallback.
  Evidence: `README.md`; `README_EN.md`; `CHANGELOG.md`; `docs/DESKTOP_RELEASE.md`; `src/bin/codex-helper-gui.rs`; `EVIDENCE_AND_GATES.md`.
  Handoff: DONE — proceed to TDRP-100 final verification/review/closeout.

- [x] TDRP-100 [owner=planner] [deps=TDRP-090] [scope=docs/workstreams/tauri-desktop-replacement-parity]
  Goal: Close this lane only after the full objective is verified against current evidence.
  Validation: DONE — final closeout verification passed: `cargo fmt --check`, `cargo check -p codex-helper-desktop`, `cargo nextest run -p codex-helper-desktop --lib`, `cargo check --features gui --bin codex-helper-gui`, `python -m json.tool WORKSTREAM.json`, and `git diff --check -- .`.
  Review: DONE_WITH_CONCERNS — no blocking findings. Residual concerns are intentionally split follow-ons: macOS/Linux packaged smoke, signed updater release pipeline, and eventual egui removal after rollback policy is settled.
  Evidence: `EVIDENCE_AND_GATES.md`; `WORKSTREAM.json`; `HANDOFF.md`.
  Handoff: DONE — lane closed for Windows packaged replacement scope.
