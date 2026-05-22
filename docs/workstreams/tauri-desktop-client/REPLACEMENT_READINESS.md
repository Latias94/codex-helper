# Tauri Desktop Client — Replacement Readiness

Status: Internal dogfood ready, not ready for egui removal
Last updated: 2026-05-22

## Decision

The Tauri desktop client is now the preferred replacement path for the existing
egui GUI.

It is ready to continue as the primary desktop-client development surface and
can be used for source-level/internal dogfooding. It is not yet ready to replace
the released `codex-helper-gui` entrypoint or delete `crates/gui`.

The reason is not the React/Tauri shell itself. The simplified product surface,
read-only admin wiring, safe mutations, and close-to-tray lifecycle boundary are
implemented. The remaining risk is in desktop replacement parity: packaged
sidecar behavior, installer/signing, single instance, launch-at-login,
auto-update, lightweight single-config import/export, and full OS-specific tray
smoke are not complete.

## Current Implemented Surface

| Area | Current state | Replacement implication |
| --- | --- | --- |
| Product shell | `apps/desktop` contains a Tauri v2 + React 19 + Tailwind CSS 4 + shadcn-style + TanStack client. | Good enough as the long-term frontend architecture. |
| Layout | Fixed desktop shell, fixed sidebar, fixed top strip, bounded main/table/panel scrolling. | Matches the user's desktop-client expectation better than a browser-page layout. |
| Pages | Dashboard, Providers, Usage, and Settings exist as the simplified MVP sitemap. | Covers the first replacement UI without exposing all control-plane internals as top-level pages. |
| Read-only data | Dashboard/Providers/Usage/Settings consume a Tauri-proxied admin read model with visible mock/fallback/disconnected/auth/stale states. | Good enough for operator visibility; still needs more form/edit parity before egui removal. |
| Safe mutations | Start/attach, switch on/off, reload, provider probe, balance refresh, route/session overrides, and explicit stop boundaries are wired. | Good enough to keep building local control-center workflows; dangerous actions are separated. |
| Lifecycle | Native close hides to tray; Quit App exits only the desktop process; Stop Proxy remains explicit. | Correct replacement direction for a resident desktop app. |
| Existing egui | `crates/gui` and `codex-helper-gui` remain intact. | Required fallback until packaging and parity gates pass. |

## Readiness Classification

| Claim | Status | Notes |
| --- | --- | --- |
| Source-level development surface | Ready | Use `apps/desktop` for future desktop UI work. |
| Internal dogfood build | Ready with concerns | Good for developers who can run from source and understand remaining desktop integration gaps. |
| Released GUI replacement | Not ready | Needs installer, signing, packaged sidecar, single instance, launch-at-login, and OS tray verification. |
| `crates/gui` removal | Not ready | Keep egui until Tauri has packaging parity and the replacement gates below pass. |
| Clash-like resident local control center | Partially ready | Tray/close semantics exist; local proxy switching and resident sidecar still need packaged validation. |

## Replacement Gates Before egui Removal

These gates must pass before claiming that Tauri has replaced the egui GUI:

1. **Source gates**
   - `pnpm test` in `apps/desktop`.
   - `pnpm build` in `apps/desktop`.
   - `cargo fmt --check`.
   - `cargo check -p codex-helper-desktop`.
   - `cargo nextest run -p codex-helper-desktop --lib`.
2. **Lifecycle gates**
   - Window close hides to tray and does not stop any proxy runtime.
   - Tray Show/Hide/Quit works through real OS menu interaction.
   - Quit App exits only the desktop process.
   - Detach and Quit App never call Stop Proxy.
   - Stop Proxy remains the only runtime shutdown path and keeps exact confirmation phrases.
3. **Packaged runtime gates**
   - Installed app can start a desktop-managed proxy without `CODEX_HELPER_CLI_PATH`.
   - Packaged sidecar lookup is deterministic and documented.
   - Windows installer behavior is verified; macOS notarization/signing decisions are recorded before macOS release claims.
4. **OS integration gates**
   - Single-instance behavior focuses/restores the existing window on second launch.
   - Launch-at-login setting is persisted through a Tauri command and manually verified per supported OS.
   - Tray behavior is manually verified on packaged Windows/macOS/Linux builds, not only dev builds.
5. **Release/update gates**
   - Signing and update-channel policy are defined.
   - Auto-update is either implemented with signed artifacts or explicitly deferred from the release promise.
6. **Config and path gates**
   - Lightweight single-config export/import is implemented with validation and backup.
   - Secret-containing exports warn the user clearly.
   - Config/log/cache path open actions work from Settings.
7. **UX parity gates**
   - Provider edit forms cover common single-endpoint provider changes without dropping advanced fields.
   - Advanced route/session/diagnostic entry points are discoverable but not promoted to top-level navigation.
   - A 1280 x 820 desktop smoke confirms the sidebar and title strip stay fixed while only intended panels scroll.

## Follow-on Split

These are outside the closed TDC-100/TDC-110 readiness lane and should become
separate workstreams or tasks:

| Follow-on | Scope | Acceptance signal |
| --- | --- | --- |
| TDC-FU-010 Packaged sidecar and installer | Decide bundle/sibling sidecar strategy, remove reliance on manual env setup for installed app, test Windows installer behavior. | Installed app starts and attaches to desktop-managed proxy from a clean user environment. |
| TDC-FU-020 Single instance | Add and verify single-instance behavior. | Second app launch focuses/restores the existing window and never starts a duplicate controller. |
| TDC-FU-030 Launch at login | Add OS autostart command, persisted setting, and UI state. | Enable/disable survives app restart and is manually verified on supported OSes. |
| TDC-FU-040 Signing and auto-update | Define release channels, signing key handling, updater hosting, and rollback posture. | Update check/apply works on signed artifacts or the release explicitly ships without auto-update. |
| TDC-FU-050 Lightweight config import/export | Export/import the single primary config file with validation, backup, and secret warning. | No heavy profile/workspace/config-catalog layer is introduced. |
| TDC-FU-060 Open paths | Add Tauri opener commands for config/log/cache paths. | Settings can open folders/files and reports missing paths safely. |
| TDC-FU-070 Provider edit parity | Implement common provider credential/config forms with schema validation. | Editing common single-endpoint providers works without losing advanced TOML fields. |
| TDC-FU-080 Full desktop lifecycle smoke | Manual packaged tray/window/quit/detach/stop matrix. | Evidence covers real OS tray menu clicks and packaged app behavior. |
| TDC-FU-090 egui deprecation/removal | Remove or deprecate `crates/gui` only after the above gates pass. | Release notes and README point users to Tauri, with rollback/deprecation notes. |

## Import/Export Boundary

Import/export must stay intentionally lightweight because codex-helper has one
primary config file.

Recommended behavior:

- export the current `~/.codex-helper/config.toml` to a user-selected path;
- import a selected TOML file only after parsing/validation;
- create a timestamped backup before replacing the active config;
- reload runtime state after import when a proxy is available;
- warn clearly when exported content may contain inline secrets.

Do not copy heavy profile/workspace/rule-management patterns from
`repo-ref/aio-coding-hub` or `repo-ref/cc-switch`.

## Non-goals For This Closeout

- No egui removal in this lane.
- No packaged desktop release claim.
- No auto-update claim.
- No multi-profile configuration manager.
- No SaaS billing authority UX; usage costs remain estimates.

