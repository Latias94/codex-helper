# Codex Responses WebSocket Relay Journal

## 2026-05-19 18:05 +08:00

- Renamed the user-facing client patch concept from `mode` to `preset` without breaking existing
  users: config reads old `mode`, CLI accepts old `--mode`, and helper-owned config writes `preset`.
- Kept the lower-level `CodexPatchMode` type for now to avoid mixing the preset rename with the
  larger provider-identity/auth-profile axis refactor.
- Added canonical short official preset names: `official-relay` and `official-imagegen`.
- Updated docs, TUI/CLI/operator copy, and targeted tests.

## 2026-05-19 18:40 +08:00

- Added `responses_websocket` to relay live smoke cases.
- CLI now supports `codex-helper codex relay-live-smoke --websocket`.
- The WebSocket smoke opens the selected upstream's `/responses` WebSocket endpoint, injects the
  official beta header, applies helper-side upstream auth and model mapping, sends one minimal
  `response.create`, and passes when a `response.*` frame is received.
- Kept the case explicit-only so normal live smoke still defaults to compact-only.

## 2026-05-19 19:05 +08:00

- Fixed the live-smoke wire name to the documented `responses_websocket` spelling while accepting
  the accidental `responses_web_socket` spelling as a read alias.
- Made CLI optional smoke flags explicit-only: no flag still runs compact; `--websocket` now runs
  only WebSocket; `--image` now runs only hosted image generation. This avoids spending an extra
  compact request when the operator only wants to probe an optional capability.
- Ran the real `--websocket` smoke. The selected `routing[0]` target reached the upstream but the
  handshake was rejected with HTTP 429 `DAILY_LIMIT_EXCEEDED`, so this run did not prove WebSocket
  support.
- `routing explain` shows a fallback provider `ciii`, but `relay-live-smoke --station ciii` cannot
  target it because the diagnostic command still accepts legacy station names rather than
  route-graph provider ids. Split this as the next clean follow-on.

## 2026-05-19 19:40 +08:00

- Added route-graph provider targeting for Codex relay diagnostics:
  - API request fields: `provider_id`, `endpoint_id`.
  - CLI flags: `--provider`, `--endpoint`.
- Capabilities and live-smoke responses now include `provider_id`, `endpoint_id`, and
  `provider_endpoint_key` when the selected upstream has route-graph identity tags.
- Provider targeting is mutually exclusive with legacy station/upstream targeting; `endpoint_id`
  requires `provider_id`.
- Added focused tests proving `ciii`/`input8`-style provider selection through the compiled
  `routing` compatibility station.
- Real capability probes show both `input8` and `ciii` support `/models`, `/responses`, and
  `/responses/compact`.
- Real WebSocket smoke:
  - `input8` succeeds through HTTP 101 and returns `codex.rate_limits` after the `response.create`
    frame, which matches Codex's WebSocket protocol stream and proves the path is accepted.
  - `ciii` upgrades to HTTP 101 but closes with code 1011 `upstream websocket proxy failed`, so its
    HTTP endpoints are fine but its WebSocket upstream/proxy path is not currently usable.
