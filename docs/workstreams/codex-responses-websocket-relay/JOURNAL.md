# Codex Responses WebSocket Relay Journal

## 2026-05-19 18:05 +08:00

- Renamed the user-facing client patch concept from `mode` to `preset` without breaking existing
  users: config reads old `mode`, CLI accepts old `--mode`, and helper-owned config writes `preset`.
- Kept the lower-level `CodexPatchMode` type for now to avoid mixing the preset rename with the
  larger provider-identity/auth-profile axis refactor.
- Added canonical short official preset names: `official-relay` and `official-imagegen`.
- Updated docs, TUI/CLI/operator copy, and targeted tests.
