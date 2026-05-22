# Tauri Desktop Replacement Parity

Status: Draft
Last updated: 2026-05-22

## Why This Lane Exists

The earlier `tauri-desktop-client` lane proved that the Tauri client is the preferred long-term replacement path for the existing egui GUI, but it intentionally stopped before release/parity work. The user now wants to keep going until Tauri actually replaces egui.

This lane exists to turn that readiness report into shipped desktop parity: installed-app sidecar behavior, resident tray semantics, single instance, launch at login, update/signing posture, lightweight config import/export, path actions, provider edit parity, packaged smoke evidence, and only then egui deprecation/removal.

## Relevant Authority

- Parent readiness lane:
  - `docs/workstreams/tauri-desktop-client/REPLACEMENT_READINESS.md`
  - `docs/workstreams/tauri-desktop-client/IMPLEMENTATION_BRIEF.md`
  - `docs/workstreams/tauri-desktop-client/EVIDENCE_AND_GATES.md`
- Current Tauri app:
  - `apps/desktop/`
  - `apps/desktop/src-tauri/`
- Current egui fallback:
  - `crates/gui/`
  - `codex-helper-gui` release entrypoint
- Root user-facing docs:
  - `README.md`
  - `README_EN.md`
  - `CHANGELOG.md`

## Problem

The Tauri desktop client has the right product direction and core shell, but it is not yet a safe release replacement for egui. The missing pieces are mostly desktop-app parity and migration safety:

- installed app must start or attach to the local proxy without manual env setup;
- only one desktop controller should exist at a time;
- tray/window behavior must be proven in packaged builds;
- launch-at-login and update/signing behavior need explicit product decisions;
- simple config backup/restore and path open actions need real Tauri commands;
- common provider edits need forms that do not destroy advanced TOML;
- egui must not be removed until the Tauri replacement is verified.

## Target State

When this workstream closes:

- Tauri is the documented and released GUI path for codex-helper.
- An installed Tauri app can start/attach to the helper proxy through a deterministic sidecar strategy.
- The app is single-instance and restores/focuses the existing window on second launch.
- Closing the main window, tray show/hide, Quit App, Detach, and Stop Proxy are verified in packaged smoke.
- Launch-at-login is implemented or explicitly excluded from the release promise with clear UI copy.
- Auto-update and signing/release-channel posture are implemented or explicitly deferred with safe release notes.
- Settings can open config/log/cache paths and can export/import the single primary config with parse validation, backup, and secret warnings.
- Providers can edit common single-endpoint provider settings without silently dropping advanced fields.
- README/CHANGELOG/release notes point users to Tauri as the GUI replacement.
- egui is either removed or clearly deprecated behind a documented fallback boundary after parity gates pass.

## In Scope

- Tauri v2 desktop integration plugins or equivalent native commands.
- Packaged sidecar strategy and installer behavior for the codex-helper CLI/runtime.
- Single-instance behavior.
- Tray/window packaged smoke scripts or manual evidence.
- Launch-at-login implementation and UI state.
- Updater/signing/release-channel design and implementation where practical.
- Lightweight single-config import/export only.
- Config/log/cache path opener actions.
- Common provider edit forms with validation and advanced-field preservation.
- README/CHANGELOG/release note updates for replacement status.
- egui deprecation/removal once evidence gates prove replacement safety.

## Out Of Scope

- Heavy multi-profile/workspace/config-catalog management.
- Recreating aio-coding-hub or cc-switch configuration managers.
- SaaS billing, login, subscription purchase, affiliate, or promo-code UX.
- Making every advanced route/session/diagnostic control top-level.
- Removing egui before packaged parity evidence exists.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Tauri remains the desired long-term GUI replacement. | High | User explicitly asked to do all follow-ons with the goal of replacing egui. | Keep egui as long-term parallel UI instead of deprecating it. |
| Import/export should remain lightweight and single-config oriented. | High | User clarified codex-helper has one primary config and should not copy heavy config managers. | Need a broader config model and more product design before implementation. |
| Tauri v2 plugins are acceptable if they reduce OS-specific maintenance. | Medium | Existing app already uses Tauri v2 and `tauri-plugin-opener`. | Implement equivalent native commands instead. |
| Packaged sidecar behavior is the highest-risk replacement gate. | High | Readiness closeout named it as a hard gate before egui removal. | Prioritize another parity slice first if packaging constraints change. |

## Architecture Direction

Use the existing boundary from the Tauri client lane:

- Admin API owns runtime/product data.
- Tauri commands own host-local desktop capabilities.
- Desktop lifecycle and packaging behavior live in `apps/desktop/src-tauri`.
- Frontend Settings is the primary UI for desktop behavior, paths, config backup/restore, update checks, and dangerous lifecycle actions.
- Provider forms should patch known config fields surgically and preserve unknown/advanced TOML instead of regenerating whole provider sections.

Prefer vertical slices that make replacement more true:

1. OS/app lifecycle primitives first: single instance, path actions, config backup/restore.
2. Packaged sidecar and installer strategy next, because release replacement depends on it.
3. Launch-at-login and updater/signing decisions before public replacement claims.
4. Provider edit parity and packaged smoke before egui deprecation/removal.

## Closeout Condition

This lane can close only when:

- every replacement gate in `REPLACEMENT_READINESS.md` is either implemented and verified or explicitly excluded from the release promise;
- current command evidence is recorded in `EVIDENCE_AND_GATES.md`;
- packaged desktop smoke evidence exists for the replacement claim;
- README/CHANGELOG/release notes match the shipped behavior;
- and egui removal/deprecation is supported by evidence rather than intent.
