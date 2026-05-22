# Tauri Desktop Replacement Parity — Milestones

Status: Draft
Last updated: 2026-05-22

## M0 — Scope And Evidence Freeze

Exit criteria:

- Replacement parity problem and target state are explicit.
- Non-goals are explicit, especially no heavy config manager and no egui removal before evidence.
- Readiness lane and current Tauri app are linked.
- First proof slice is chosen.

Primary evidence:

- `docs/workstreams/tauri-desktop-replacement-parity/DESIGN.md`
- `docs/workstreams/tauri-desktop-replacement-parity/TODO.md`

Status: Complete.

## M1 — Local Desktop Parity Primitives

Exit criteria:

- Settings can open config/log/cache paths through real Tauri commands.
- Settings can export/import the single primary config with validation, backup, and secret warning.
- Single instance restores/focuses the existing window.

Primary gates:

- `pnpm test`
- `pnpm build`
- `cargo check -p codex-helper-desktop`
- `cargo nextest run -p codex-helper-desktop --lib`

## M2 — Packaged Runtime And OS Integration

Exit criteria:

- Installed app can start/attach through a deterministic sidecar strategy.
- Launch-at-login is implemented or explicitly excluded from release copy.
- Signing, installer, and update posture are documented and, where possible, implemented.

Primary gates:

- `pnpm tauri:build` or documented platform-specific packaging fallback.
- Manual packaged smoke on Windows at minimum.

## M3 — Provider Edit Parity And Packaged Smoke

Exit criteria:

- Common provider edits work without dropping advanced config.
- Full packaged lifecycle smoke is recorded.
- Any OS-specific failures are fixed or split before replacement claims.

Primary gates:

- Frontend/Rust tests for provider forms and config patching.
- Packaged smoke evidence.

## M4 — Replacement Release And egui Deprecation/Removal

Exit criteria:

- Tauri replacement claim is reflected in README/CHANGELOG/release notes.
- egui is deprecated or removed only if packaged parity evidence supports it.
- Final gate evidence is fresh.
- Workstream status is updated.
