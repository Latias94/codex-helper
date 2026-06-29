# Desktop Release Packaging

This document records the current desktop packaging contract. It is intentionally
scoped to packaged desktop release mechanics: Windows packaged lifecycle smoke
has passed, while macOS/Linux packaged parity and signed auto-update operations
remain separate release follow-ups.

## Current target

- App shell: `apps/desktop`
- Tauri version: v2
- Frontend stack: React 19, Tailwind CSS 4, shadcn/ui-style components, TanStack Router/Query/Table
- First packaged target: Windows NSIS installer
- First public replacement channel: not enabled yet. The current tag release
  workflow intentionally publishes only the cargo-dist CLI artifacts; the
  desktop installer remains a local/manual validation artifact until the signed
  release channel and rollback process are ready.
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

## Signing and release-channel posture

TDRP-060 intentionally does **not** enable automatic updates yet. Tauri's
updater is a signed-artifact pipeline, not a plain "check an HTTP URL" feature.
Per the Tauri updater contract, a production updater needs:

1. updater signing keypair;
2. private key supplied only by CI/release secrets;
3. public key embedded in `tauri.conf.json`;
4. HTTPS updater endpoints that return signed metadata;
5. uploaded updater artifacts for each supported target;
6. rollback and revocation instructions for a bad release.

Current first replacement release policy:

- Do not ship Windows NSIS artifacts through the public GitHub tag release yet.
  `.github/workflows/release.yml` intentionally omits the Tauri installer job for
  v0.19.0, so the public release contains the cargo-dist CLI artifacts only.
- Keep `pnpm tauri:build` available for local/manual validation from
  `apps/desktop`, but do not upload those installers as release artifacts until
  signing, release-channel, and rollback operations are proven.
- Keep automatic update checks disabled in the app until the signing key,
  artifact hosting, release channel, and rollback process exist.
- Do not add `tauri-plugin-updater` or updater UI that implies a working update
  path before those release operations are real.
- The Settings page must show honest disabled copy instead of a clickable
  placeholder.

Minimum future implementation checklist:

1. Generate and escrow a Tauri updater keypair outside the repository.
2. Add only the public key to `apps/desktop/src-tauri/tauri.conf.json`.
3. Store `TAURI_SIGNING_PRIVATE_KEY` and, if needed,
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` as release CI secrets.
4. Add an HTTPS updater endpoint for stable channel metadata.
5. Configure CI to upload installer plus updater artifacts and signatures for
   every target.
6. Smoke a signed dev/staging release by installing N-1, checking update, applying
   update, verifying sidecar/startup behavior, and documenting rollback.

## OS integration

The desktop app registers the official Tauri autostart plugin for launch at
login. Settings uses the plugin's JavaScript guest binding to read, enable, and
disable the OS login item.

Current policy:

- Launch at login starts only the desktop companion.
- It does not automatically stop, restart, or seize an existing local proxy.
- Startup-time proxy auto-start remains explicit; the companion can start a
  desktop-managed sidecar, but proxy shutdown still requires the user-facing
  `Stop Proxy` action.
- The plugin supports Windows, macOS, and Linux desktop targets. Android/iOS are
  intentionally outside this desktop release target.

## Replacement posture

The Windows NSIS packaged Tauri app is the intended desktop GUI replacement
path, but it is not part of the v0.19.0 public release. The legacy
`codex-helper-gui` egui binary remains available as a deprecated fallback for
rollback and for platforms where packaged Tauri parity has not yet been smoked.

Do not claim cross-platform replacement until macOS/Linux packaged smoke has
covered the same lifecycle behavior. Do not enable or advertise automatic
updates until signed updater artifacts and rollback operations are proven.

## Verification status

Verified on Windows:

- `pnpm tauri:build` completes.
- The NSIS installer is produced under `target/release/bundle/nsis/`.
- The GitHub tag release workflow intentionally does not upload the Windows
  Tauri installer for v0.19.0; CLI artifacts remain the public release surface.
- `7z l target/release/bundle/nsis/codex-helper_<version>_x64-setup.exe` lists both
  `codex-helper-desktop.exe` and the bundled `codex-helper.exe` sidecar.
- Compile/test verification proves the launch-at-login plugin is registered and
  the Settings switch is wired to the real guest binding.
- Packaged lifecycle smoke runs in isolated `CODEX_HELPER_HOME` / `CODEX_HOME`
  directories and clears developer CLI overrides.
- The installed desktop app starts its bundled sidecar without
  `CODEX_HELPER_CLI_PATH`.
- Native close hides to tray, native tray menu Show Window / Hide to Tray / Quit
  App paths work, and Quit App exits only the desktop process.
- Detach, explicit Stop Proxy, second-launch focus/restore, config
  export/import, and Provider common edit UI pass in the packaged app.
- Launch-at-login enable/disable registers and unregisters the Windows HKCU Run
  entry during smoke cleanup.
- Release posture is defined: manual GitHub Releases for the first replacement
  release; auto-update is disabled until signing, endpoint, artifact hosting, and
  rollback are real.

Still required before broader release claims:

- Signing key escrow, updater endpoint, and signed update smoke before enabling
  automatic updates.
- macOS/Linux packaged lifecycle smoke before claiming cross-platform GUI
  replacement.
- A release rollback checklist before removing the legacy egui fallback entirely.
