# Desktop Release Packaging

This document records the current desktop packaging contract. It is intentionally
separate from the final replacement announcement: the Tauri app must still pass
the full packaged lifecycle smoke before it can replace the egui GUI.

## Current target

- App shell: `apps/desktop`
- Tauri version: v2
- Frontend stack: React 19, Tailwind CSS 4, shadcn/ui-style components, TanStack Router/Query/Table
- First packaged target: Windows NSIS installer
- Build command:

```powershell
cd apps/desktop
pnpm tauri:build
```

## Sidecar strategy

The packaged desktop app ships the `codex-helper` CLI as a Tauri external
binary sidecar.

Build flow:

1. `tauri.conf.json` runs `pnpm tauri:build:assets` before packaging.
2. `pnpm tauri:build:assets` runs the frontend build and then `pnpm prepare:sidecar`.
3. `scripts/prepare-sidecar.mjs` runs `cargo build --release --bin codex-helper`.
4. The script copies the release CLI to
   `apps/desktop/src-tauri/sidecars/codex-helper-$TARGET_TRIPLE(.exe)`.
5. Tauri `bundle.externalBin = ["sidecars/codex-helper"]` copies that binary into
   the packaged resource directory as `codex-helper(.exe)`.

The generated sidecar binaries are ignored by Git. Commit only the sidecar
preparation script and `src-tauri/sidecars/.gitignore`.

Environment escape hatches:

- `TAURI_ENV_TARGET_TRIPLE`, `CARGO_BUILD_TARGET`, or `TARGET` can override the
  inferred sidecar target triple.
- `CODEX_HELPER_DESKTOP_SKIP_CLI_BUILD=1` skips rebuilding the release CLI only
  when an existing release binary is already present.
- `CODEX_HELPER_DESKTOP_ALLOW_DEBUG_SIDECAR=1` allows a debug CLI fallback when
  no release CLI exists. Do not use this for release artifacts.

## Runtime lookup order

`start_desktop_proxy` resolves the CLI in this deterministic order:

1. packaged resource directory sidecar: `codex-helper(.exe)`;
2. sibling binary next to the current desktop executable, including Cargo `deps`
   parent fallback for development builds;
3. developer environment override: `CODEX_HELPER_CLI_PATH` or legacy
   `CODEX_HELPER_CLI`.

The environment override is only a development fallback. A packaged app should
not require shell setup to start a desktop-managed proxy.

## Verification status

Verified on Windows:

- `pnpm tauri:build` completes.
- The NSIS installer is produced under `target/release/bundle/nsis/`.
- `7z l target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe` lists both
  `codex-helper-desktop.exe` and the bundled `codex-helper.exe` sidecar.

Still required before egui replacement:

- Full packaged lifecycle smoke in an isolated environment:
  - start packaged app;
  - start desktop-managed proxy without `CODEX_HELPER_CLI_PATH`;
  - close-to-tray/show/hide/quit behavior;
  - detach and explicit stop behavior;
  - second launch focus;
  - config export/import.
- Signing and updater posture.
- Launch-at-login behavior.
- Provider edit parity for common single-endpoint providers.
