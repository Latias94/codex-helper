# Codex Responses WebSocket Relay — Evidence And Gates

Status: Historical (superseded by the canonical relay runtime on 2026-07-13)
Last updated: 2026-05-19

The commands and forecast/JSONL references below are retained as implementation history, not as current verification or accounting authority.

## Gate Set

### Targeted patch-surface gates

```powershell
cargo nextest run -p codex-helper-core responses_websocket
cargo nextest run -p codex-helper-core codex_client_patch_config_parses_responses_websocket_transport_option
```

### Targeted WebSocket relay gates

```powershell
cargo nextest run -p codex-helper-core responses_websocket
```

### Package gates

```powershell
cargo fmt --check
cargo check -p codex-helper
cargo nextest run -p codex-helper-core
cargo nextest run -p codex-helper-tui spend_forecast
cargo nextest run -p codex-helper-tui stats_kpis_show_spend_projection_only_when_sample_is_confident
```

### Broader closeout gate

```powershell
cargo nextest run --workspace
```

Use a narrower closeout gate if workspace runtime is too expensive and record the reason here.

## Fresh Evidence

### 2026-05-19 — CRW-010 Scope Freeze

Claim: correct first-class solution is helper-owned Responses WebSocket relay plus explicit opt-in
an explicit WebSocket transport switch, not direct Codex-to-sub2api wiring and not implicit WebSocket support on existing presets.

Evidence:

- `repo-ref/codex` uses `supports_websockets` to select Responses WebSocket v2.
- `repo-ref/sub2api` exposes Responses WebSocket v2 on `/responses`-style routes and uses the same
  beta header.
- `codex-helper` currently has no WebSocket upgrade relay in `crates/core/src/proxy/router_setup.rs`
  and therefore must keep existing official presets HTTP-only.

Commands: source inspection only.

### 2026-05-19 16:45 +08:00 — CRW-020/030/040/050 Implementation And Gates

Claim: `responses_websocket` is now an explicit transport switch, not another patch preset; helper owns
the first shippable Responses WebSocket relay vertical slice; operator docs describe the new switch.

Implemented:

- Removed public `official-ws-*` patch presets and compatibility aliases; the feature was never
  released, so stale switch-state should fail loudly instead of silently preserving preset sprawl.
- Added `CodexSwitchOptions.responses_websocket`, config parsing through
  `[codex.client_patch].responses_websocket`, and CLI `switch on --responses-websocket`.
- Kept official bridge presets HTTP-only by default (`supports_websockets = false`), and writes
  `supports_websockets = true` only when the transport switch is enabled.
- Added exact WebSocket routes for `/responses`, `/v1/responses`, and
  `/backend-api/codex/responses`, with non-WebSocket/non-GET traffic falling back to the normal HTTP
  proxy path.
- Added helper-owned relay behavior: read first `response.create`, apply request overrides,
  selected-upstream model mapping and request filtering, reuse routing/model-support checks,
  session route affinity, concurrency permit acquisition for route-graph endpoints, helper-side auth
  injection, `OpenAI-Beta: responses_websockets=2026-02-06`, and bidirectional frame relay.
- Updated `docs/CONFIGURATION.md` and `docs/CONFIGURATION.zh.md`.

Command evidence:

```powershell
cargo fmt --check
# pass

cargo check -p codex-helper
# pass

cargo nextest run -p codex-helper-core responses_websocket
# pass: 7 tests run, 7 passed

cargo nextest run -p codex-helper-core forecast
# pass: 4 tests run, 4 passed

cargo nextest run -p codex-helper-tui spend_forecast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper-tui stats_kpis_show_spend_projection_only_when_sample_is_confident
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper-core
# pass: 560 tests run, 560 passed
```

### 2026-05-19 17:20 +08:00 — Burn Forecast Ledger Sampling Repair

Claim: Stats burn rate no longer depends solely on the display-limited in-memory recent list, and
new request-ledger records include the model metadata needed for log replay pricing.

Findings:

- Local `requests.jsonl` tail showed high request volume: 124 requests in the latest 5 minutes,
  1349 in the latest 60 minutes, and 25000 in the inspected 24-hour tail.
- TUI burn forecast previously read only `snapshot.recent`, while `refresh_snapshot` requested 2000
  entries and `DashboardSnapshot` clamped the API result to 2000. Under high request volume this can
  under-sample the rolling forecast window.
- Recent request-log records had `usage` but no top-level `model`, no `route_decision`, and no
  retry-attempt model, so replaying the ledger could not price those records.

Implemented:

- TUI refresh now requests the configured `recent_finished_max` instead of a hard-coded 2000.
- Dashboard snapshot clamping now follows `ProxyState::recent_finished_max()`.
- Stats burn forecast uses a forecast-only merged source: in-memory recent plus
  `requests.jsonl` tail (`CODEX_HELPER_USAGE_FORECAST_LOG_TAIL_LINES`, default 20000), de-duplicated
  by trace id.
- Request ledger writes now include top-level `model` and structured `route_decision` where
  available, so future log-tail replay can estimate costs.
- Removed stale `official-ws-*` serde aliases entirely.

Command evidence:

```powershell
cargo fmt --check
# pass

cargo check -p codex-helper-core
# pass

cargo check -p codex-helper-tui
# pass

cargo check -p codex-helper
# pass

cargo nextest run -p codex-helper-core request_log request_model_reads_route_decision request_model_prefers_top_level_model_from_current_request_log_schema --no-fail-fast
# pass: 6 tests run, 6 passed

cargo nextest run -p codex-helper-tui spend_forecast merge_forecast_recent_requests --no-fail-fast
# pass: 3 tests run, 3 passed

cargo nextest run -p codex-helper-tui stats_kpis_show_spend_projection_only_when_sample_is_confident --no-fail-fast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper-core responses_websocket --no-fail-fast
# pass: 7 tests run, 7 passed

cargo nextest run -p codex-helper-core forecast --no-fail-fast
# pass: 4 tests run, 4 passed
```

Residual follow-ons:

- Parse usage from Responses WebSocket events if/when the upstream emits enough usage metadata.
- Add an optional live upstream smoke test against a real sub2api instance after operator
  acknowledgement, because the current integration test uses a local mock WebSocket upstream.
- Consider enabling permessage-deflate/custom TLS connector parity with Codex upstream if a real
  relay requires it; the current implementation supports native-root TLS via `rustls-tls-native-roots`.

### 2026-05-19 18:05 +08:00 бк Client Patch Preset Rename

Claim: user-facing client patch configuration now treats `mode` as a legacy spelling and writes the
new `preset` key everywhere helper owns config output.

Implemented:

- `[codex.client_patch].preset` is the primary config key.
- Legacy `[codex.client_patch].mode` is still accepted for existing users.
- If both `preset` and `mode` are present with different meanings, config loading fails instead of
  guessing.
- Config save/generation preserves `[codex.client_patch]` but normalizes valid legacy `mode` to
  `preset`.
- CLI now exposes `--preset`; legacy `--mode` remains an alias.
- Canonical preset names are `default`, `chatgpt-bridge`, `imagegen-bridge`, `official-relay`, and
  `official-imagegen`; old official `*-bridge` names remain accepted as aliases.
- Codex relay diagnostics API accepts request field `patch_preset` while preserving response
  `patch_mode` for compatibility.

Command evidence:

```powershell
cargo fmt --check
# pass

cargo check -p codex-helper
# pass

cargo nextest run -p codex-helper-core codex_client_patch --no-fail-fast
# pass: 7 tests run, 7 passed

cargo nextest run -p codex-helper-core bridge_ready_check --no-fail-fast
# pass: 4 tests run, 4 passed

cargo nextest run -p codex-helper-core codex_capabilities_api_reports_expected_observed_and_mismatches --no-fail-fast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper codex_relay_cli --no-fail-fast
# pass: 5 tests run, 5 passed

cargo nextest run -p codex-helper-tui codex_relay --no-fail-fast
# pass: 7 tests run, 7 passed

cargo nextest run -p codex-helper-tui startup --no-fail-fast
# pass: 4 tests run, 4 passed
```

### 2026-05-19 18:40 +08:00 бк Responses WebSocket Live Smoke

Claim: operators can explicitly test a selected upstream relay's Responses WebSocket v2 path before
turning on `responses_websocket` for normal Codex traffic.

Implemented:

- Added `CodexRelayLiveSmokeCase::ResponsesWebSocket`.
- Added CLI `codex-helper codex relay-live-smoke --websocket`.
- The WebSocket live smoke:
  - connects to the selected upstream's `/responses` WebSocket URL,
  - injects `OpenAI-Beta: responses_websockets=2026-02-06`,
  - injects helper-side upstream auth,
  - applies selected-upstream model mapping,
  - sends one minimal `response.create` frame,
  - reports pass when a `response.*` frame is received.
- Kept WebSocket smoke explicit-only; default live smoke remains compact-only.

Command evidence:

```powershell
cargo fmt --check
# pass

cargo check -p codex-helper
# pass

cargo nextest run -p codex-helper-core codex_relay_live_smoke_websocket_sends_response_create_with_beta_and_auth --no-fail-fast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper-core codex_relay_live_smoke --no-fail-fast
# pass: 9 tests run, 9 passed

cargo nextest run -p codex-helper codex_relay_cli --no-fail-fast
# pass: 6 tests run, 6 passed

cargo nextest run -p codex-helper live_smoke_cases --no-fail-fast
# pass: 4 tests run, 4 passed

cargo nextest run -p codex-helper-tui codex_relay --no-fail-fast
# pass: 7 tests run, 7 passed

cargo run -p codex-helper --bin codex-helper -- codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --websocket --json
# ran websocket-only; selected routing[0] https://input.9z1.me/v1; upstream rejected handshake with HTTP 429 DAILY_LIMIT_EXCEEDED

cargo run -p codex-helper --bin codex-helper -- routing explain --model gpt-5.5 --json
# selected provider_id=input; fallback candidates include provider_id=ciii base_url=https://codex.ciii.club/v1

cargo run -p codex-helper --bin codex-helper -- codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --station ciii --model gpt-5.5 --websocket --json
# fails before network IO: station 'ciii' not found, because live-smoke targeting still accepts legacy station names, not route-graph provider ids
```

### 2026-05-19 19:40 +08:00 бк CRW-070 Route-Graph Diagnostic Targeting

Claim: Codex relay capability diagnostics and live smoke can target route-graph provider endpoints
directly, so operators can run `--provider ciii` / `--provider input8` without changing normal
routing.

Implemented:

- Added API request fields `provider_id` and `endpoint_id` for both capability diagnostics and live
  smoke.
- Added CLI flags `--provider` and `--endpoint` for both `relay-capabilities` and
  `relay-live-smoke`.
- Extended target selection to resolve provider ids from compiled route-graph upstream tags, prefer
  `endpoint_id = default` when no endpoint is specified, and reject mixed provider/station
  targeting.
- Responses and evidence now include `provider_id`, `endpoint_id`, and `provider_endpoint_key` when
  available.

Command evidence:

```powershell
cargo nextest run -p codex-helper-core codex_relay_target --no-fail-fast
# pass: 2 tests run, 2 passed

cargo nextest run -p codex-helper-core codex_relay_live_smoke_targets_route_graph_provider_id --no-fail-fast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper-core codex_relay_capabilities_targets_route_graph_provider_id --no-fail-fast
# pass: 1 test run, 1 passed

cargo nextest run -p codex-helper codex_relay_cli --no-fail-fast
# pass: 6 tests run, 6 passed
```

Additional live-provider evidence:

```powershell
cargo run -p codex-helper --bin codex-helper -- codex relay-capabilities --model gpt-5.5 --provider input8 --json
# pass: target codex/input8/default; /models, /responses, /responses/compact supported by validation-only probes

cargo run -p codex-helper --bin codex-helper -- codex relay-capabilities --model gpt-5.5 --provider ciii --json
# pass: target codex/ciii/default; /models, /responses, /responses/compact supported by validation-only probes

cargo run -p codex-helper --bin codex-helper -- codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --provider input8 --websocket --json
# pass: target codex/input8/default; WebSocket handshake HTTP 101; accepted response.create and returned codex.rate_limits

cargo run -p codex-helper --bin codex-helper -- codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --provider ciii --websocket --json
# unknown: target codex/ciii/default; WebSocket handshake HTTP 101 then close code 1011 "upstream websocket proxy failed"
```
