# Tauri Desktop Replacement Parity — Evidence And Gates

Status: Draft
Last updated: 2026-05-22

## Gate Set

### Desktop Frontend Gate

```powershell
cd apps/desktop
pnpm test
pnpm build
```

Proves the React/Tauri frontend compiles and targeted UI behavior still passes.

### Desktop Rust Gate

```powershell
cargo fmt --check
cargo check -p codex-helper-desktop
cargo nextest run -p codex-helper-desktop --lib
```

Proves the Tauri Rust command/lifecycle crate formats, compiles, and passes targeted command/lifecycle tests.

### Diff Hygiene Gate

```powershell
git diff --check -- .
```

Proves no whitespace errors in the current diff. Windows LF/CRLF warnings are acceptable if there are no whitespace errors.

### Packaging Gate

```powershell
cd apps/desktop
pnpm tauri:build
```

Run when packaging behavior is part of the claim. If packaging fails because signing, installer, or platform dependencies are not yet configured, record the failure and either fix it or split the packaging blocker before replacement claims.

### Packaged Lifecycle Smoke Gate

Manual or scripted packaged app evidence must cover:

- start packaged app;
- start desktop-managed proxy without `CODEX_HELPER_CLI_PATH`;
- close main window hides to tray;
- tray show/hide works;
- Quit App exits desktop only;
- Detach does not stop proxy;
- Stop Proxy requires confirmation and stops only by explicit action;
- second launch focuses existing window;
- config export/import creates backup and validates TOML.

## Evidence Anchors

- `docs/workstreams/tauri-desktop-client/REPLACEMENT_READINESS.md`
- `docs/workstreams/tauri-desktop-replacement-parity/DESIGN.md`
- `docs/workstreams/tauri-desktop-replacement-parity/TODO.md`
- `apps/desktop/src-tauri/src/`
- `apps/desktop/src/features/settings/`
- `apps/desktop/src/features/providers/`

## Evidence Log

### 2026-05-22 — Workstream opened

Evidence:

- User asked to continue all replacement follow-ons with the goal of replacing egui.
- Created `docs/workstreams/tauri-desktop-replacement-parity/` with DESIGN, TODO, MILESTONES, EVIDENCE_AND_GATES, HANDOFF, WORKSTREAM.json, and JOURNAL.
- First executable task is TDRP-020: Settings path actions plus lightweight single-config export/import.

Result:

- PASS — scope and task split are explicit. Implementation work may begin.

### 2026-05-22 — TDRP-020 path actions and lightweight config import/export

Evidence:

- Added Tauri path/config commands in `apps/desktop/src-tauri/src/commands/paths.rs`:
  - `open_known_path` for home/config/logs/cache via `tauri-plugin-opener`;
  - `export_config` for copying the single active `config.toml` to a user-selected file;
  - `import_config` for reading a selected `.toml`, validating it as `ProxyConfigV4`, backing up the current config with a timestamped `.bak`, and replacing the active `config.toml`.
- Added `tauri-plugin-dialog` and frontend file picker integration for export/import.
- Updated Settings:
  - About/paths rows have open buttons;
  - config/log/cache path actions are real Tauri commands;
  - export/import buttons are wired;
  - UI warns that exported TOML may contain inline secrets;
  - import success reports backup path when a previous config existed.
- Added tests:
  - Rust tests prove export copies the single config and carries a secret warning;
  - Rust tests prove import validates TOML, creates a timestamped backup, and does not overwrite current config when validation fails;
  - frontend route test proves Settings path/export/import buttons invoke the correct Tauri commands and report backup status.

Review:

- Workstream compliance: PASS — this implements TDRP-020 without introducing profile/workspace/config-catalog management.
- Code quality: PASS_WITH_CONCERNS — validation accepts supported TOML config shapes and intentionally excludes legacy JSON import from the lightweight desktop UI flow.
- Missing gates: final `git diff --check -- .` still needs to be run after evidence/doc updates.

Verification:

- Command: `cargo nextest run -p codex-helper-desktop --lib paths::tests`
- Scope: TDRP-020 Rust path/config command tests.
- Result: PASS — 3 tests.

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 12 tests.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Packaged file picker/open-path smoke remains part of TDRP-040/TDRP-080.
- Runtime reload after import is still a user action; future polish can offer "Import and reload" after packaged lifecycle gates are stable.

Result:

- DONE_WITH_CONCERNS — TDRP-020 is implemented and verified at command/build/test level; packaged smoke remains a later gate.

### 2026-05-22 — TDRP-030 single instance

Evidence:

- Added `tauri-plugin-single-instance`.
- Registered the plugin during Tauri builder setup.
- The second-instance callback calls the existing lifecycle path that shows, unminimizes, and focuses the main window.
- Added a lifecycle assertion that second-instance launch leaves any proxy runtime running and does not request Stop Proxy or app shutdown.

Review:

- Workstream compliance: PASS_WITH_CONCERNS — code-level single-instance behavior is wired. The remaining concern is packaged second-launch smoke, which belongs to TDRP-080.
- Code quality: PASS — reuse of `lifecycle::show_main_window` keeps behavior consistent with tray Show Window and avoids duplicate focus logic.
- Missing gates: packaged smoke is not expected for this code slice but remains required before egui replacement.

Verification:

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib lifecycle::tests::second_instance_launch_never_stops_proxy_runtime`
- Scope: targeted lifecycle policy.
- Result: PASS — 1 test.

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 13 tests.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Result:

- DONE_WITH_CONCERNS — single-instance plugin is wired and the second-launch callback focuses the existing main window without touching proxy lifecycle. Packaged second-launch smoke remains a replacement gate.

### 2026-05-22 — TDRP-040 packaged sidecar and Windows NSIS build

Evidence:

- Chose a first-class Tauri external binary sidecar instead of requiring a documented sibling CLI install.
- Added `apps/desktop/scripts/prepare-sidecar.mjs`:
  - builds `cargo build --release --bin codex-helper`;
  - infers or accepts the Tauri target triple;
  - copies the CLI to `apps/desktop/src-tauri/sidecars/codex-helper-$TARGET_TRIPLE(.exe)`.
- Added `apps/desktop/src-tauri/sidecars/.gitignore` so generated sidecar binaries are not committed.
- Updated `apps/desktop/src-tauri/tauri.conf.json`:
  - `bundle.active = true`;
  - Windows target is `nsis`;
  - `bundle.externalBin = ["sidecars/codex-helper"]`;
  - `beforeBuildCommand = "pnpm tauri:build:assets"`.
- Updated `start_desktop_proxy` sidecar lookup:
  - packaged resource directory sidecar first;
  - sibling development binary second;
  - `CODEX_HELPER_CLI_PATH` / legacy `CODEX_HELPER_CLI` only as developer fallback.
- Added Rust tests proving packaged sidecar lookup wins over env overrides and env lookup remains a final fallback.
- Added `docs/DESKTOP_RELEASE.md` with packaging contract, sidecar lookup order, and remaining release gates.

Verification:

- Command: `pnpm prepare:sidecar`
- Scope: `apps/desktop`
- Result: PASS — release CLI was built and copied to `apps/desktop/src-tauri/sidecars/codex-helper-x86_64-pc-windows-msvc.exe`.

- Command: `pnpm tauri:build`
- Scope: `apps/desktop`
- Result: PASS — produced `target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe`.

- Command: `7z l target\release\bundle\nsis\codex-helper_0.16.0_x64-setup.exe`
- Scope: Windows NSIS installer contents.
- Result: PASS — installer lists `codex-helper-desktop.exe` and bundled `codex-helper.exe`.

- Command: `cargo nextest run -p codex-helper-desktop --lib control::tests::cli_resolution`
- Scope: deterministic sidecar resolution.
- Result: PASS — 2 tests.

- Command: `pnpm test`
- Scope: `apps/desktop`.
- Result: PASS — 5 files, 23 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`.
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: Tauri desktop crate.
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: Tauri desktop crate.
- Result: PASS — 15 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace.
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff.
- Result: PASS — no whitespace errors; only Windows LF/CRLF warnings for edited text files.

Deferred:

- Full live packaged lifecycle smoke is still TDRP-080. It was not completed in this slice because the developer machine already had a live codex-helper runtime; replacement smoke must run in an isolated environment and must not stop or mutate the user's active local helper.
- Signing/notarization and updater posture remain TDRP-060.
- Launch-at-login remains TDRP-050.

Result:

- DONE_WITH_CONCERNS — Windows packaged sidecar/installer build is deterministic and verified at artifact/content level. Full interactive packaged runtime smoke remains required before any egui replacement claim.
