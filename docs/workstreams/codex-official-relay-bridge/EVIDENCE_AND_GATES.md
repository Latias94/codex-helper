# Codex Official Relay Bridge — Evidence And Gates

Status: Complete
Last updated: 2026-05-18

## Smallest Current Repro

Historical local logs show compact traffic has not used the official compact endpoint:

```powershell
rg -n "responses/compact|compaction_trigger|compaction_summary|context_compaction" $env:USERPROFILE\.codex-helper\logs -S
```

Observed on 2026-05-18: no `/responses/compact`, `compaction_trigger`,
`compaction_summary`, or `context_compaction` hits in request/control/runtime logs.

## Gate Set

### Targeted Iteration Gate

```powershell
cargo nextest run -p codex-helper-core codex_integration
```

Proves Codex patch mode behavior.

```powershell
cargo nextest run -p codex-helper-core responses_compact
```

Proves helper proxy behavior for compact request paths if the implementation adds compact-specific
tests.

### Package Gate

```powershell
cargo nextest run -p codex-helper-core
```

Proves the core crate after compact bridge changes.

### Formatting Gate

```powershell
cargo fmt --check
```

Proves Rust formatting did not drift.

### Broader Closeout Gate

```powershell
cargo nextest run --workspace
```

Use a narrower closeout gate if the workspace is too expensive, and record the reason here.

### Review Gate

Run `review-workstream` before accepting task or lane completion. Record blocking findings, missing
gates, and residual risks here or link to the review note.

## Evidence Anchors

- `docs/workstreams/codex-official-relay-bridge/DESIGN.md`
- `docs/workstreams/codex-official-relay-bridge/TODO.md`
- `docs/workstreams/codex-official-relay-bridge/MILESTONES.md`
- `crates/core/src/codex_integration.rs`
- `crates/core/src/proxy/attempt_request.rs`
- `crates/core/src/request_ledger.rs`
- `docs/CONFIGURATION.md`
- `docs/CONFIGURATION.zh.md`

## Fresh Verification

### 2026-05-18 — CORB-020 Official Relay Compact V1 Slice

Claim verified: `official-relay-bridge` can make Codex-facing `codex_proxy` look like the official
OpenAI Responses provider for remote compaction v1 while helper continues to strip Codex client auth
and forwards `/responses/compact` to an upstream `/v1/responses/compact` endpoint.

Commands run from repo root:

```powershell
cargo fmt
```

Result: passed. Rust formatting applied before tests.

```powershell
cargo nextest run -p codex-helper-core codex_switch_on_official_relay_bridge_sets_openai_name_and_disables_websockets codex_switch_on_official_relay_bridge_records_mode_without_auth_json_patch codex_switch_status_infers_official_relay_bridge_without_state official_relay_bridge_ready_check_rejects_unresolved_upstream_env codex_client_patch_mode_parses_official_relay_bridge prepare_attempt_request_strips_client_auth_in_official_relay_bridge_without_upstream_secret proxy_forwards_responses_compact_to_upstream_v1_compact_path
```

Result: passed, 7 tests run. Proves TOML patch output, switch-state/status behavior, config parsing,
runtime upstream credential guard, Codex client auth stripping, and compact path forwarding/log
visibility.

```powershell
cargo check -p codex-helper
cargo check -p codex-helper-tui
cargo check -p codex-helper-gui
```

Result: all passed. Proves CLI/TUI/GUI entry points compile with the new mode.

```powershell
cargo nextest run -p codex-helper-core
```

Result: passed, 479 tests run. Proves the core crate after the official relay bridge changes.

Skipped broader gate:

- `cargo nextest run --workspace` was not run for CORB-020 because the task scope is core/client
  patch plus CLI/TUI/GUI entry compilation, and `codex-helper-core` full nextest plus package checks
  covered the changed behavioral surface. Run the workspace gate at M4 closeout if the lane is ready
  to merge.

### 2026-05-18 — CORB-030/CORB-040 Operator Diagnostics And Static Fallback

Claim verified: operators can opt into `official-relay-bridge`, distinguish remote compact v1
traffic from ordinary `/responses` fallback using a non-sensitive path filter, and rely on explicit
user-selected mode for the first release instead of helper-side active probing.

Additional source evidence checked on 2026-05-18:

- Codex `compact.rs` delegates remote compact selection to `provider.supports_remote_compaction()`.
- Codex `ModelProviderInfo::supports_remote_compaction()` returns true for `name = "OpenAI"` and
  Azure Responses providers.
- Codex remote compaction v1 uses the compact conversation client path that helper exposes as
  `/responses/compact`; remote compaction v2 uses ordinary `/responses` stream semantics with a
  compaction output shape and remains deferred.
- sub2api has a compact probe/test mode for `/responses/compact` and a separate WebSocket upgrade
  handler/forwarder, so helper should not advertise WebSocket support until it owns an upgrade path.

Commands run from repo root:

```powershell
cargo fmt --check
```

Result: passed. Proves Rust formatting did not drift after diagnostics changes.

```powershell
cargo nextest run -p codex-helper-core request_ledger
```

Result: passed, 9 tests run. Proves request-ledger filtering still works and now matches compact
request paths through `--path responses/compact`.

```powershell
cargo nextest run -p codex-helper-core capabilities
```

Result: passed, 6 tests run. Proves admin API request-ledger filtering still works with the new
path filter parameter.

```powershell
cargo nextest run -p codex-helper-gui request_ledger
```

Result: passed, 6 tests run. Proves GUI/attached request-ledger query construction still compiles
and forwards path filters where applicable.

```powershell
cargo check -p codex-helper
cargo check -p codex-helper-tui
cargo check -p codex-helper-gui
```

Result: all passed. Proves CLI/TUI/GUI entry points compile after the diagnostics surface changes.

```powershell
cargo nextest run -p codex-helper-core codex_switch_on_official_relay_bridge_sets_openai_name_and_disables_websockets codex_switch_on_official_relay_bridge_records_mode_without_auth_json_patch codex_switch_status_infers_official_relay_bridge_without_state official_relay_bridge_ready_check_rejects_unresolved_upstream_env codex_client_patch_mode_parses_official_relay_bridge prepare_attempt_request_strips_client_auth_in_official_relay_bridge_without_upstream_secret proxy_forwards_responses_compact_to_upstream_v1_compact_path proxy_records_responses_compact_unsupported_status_for_fallback_diagnostics filters_match_compact_request_path
```

Result: passed, 9 tests run. Re-proves official relay bridge behavior plus compact path
diagnostics, including visible unsupported-relay 404 status for fallback decisions.

```powershell
cargo nextest run -p codex-helper-core responses_compact
```

Result: passed, 2 tests run. Proves supported `/responses/compact` forwarding and unsupported
compact status visibility.

```powershell
cargo nextest run -p codex-helper-core
```

Result: passed, 481 tests run. Proves the core crate after bridge and diagnostics changes.

```powershell
cargo run -q --bin codex-helper -- usage find --path responses/compact --limit 20
```

Result: command passed and reported no matching local log records. This proves the new CLI filter is
accepted and confirms the historical local logs still have no `/responses/compact` traffic.

Decision: first release does not add active compact probing or per-provider compact capability
hints. The mode is explicit and reversible: unsupported relays should fail visibly on
`/responses/compact`, and operators can switch back to `default`. Active probing can be split into a
follow-on if operators need automatic relay classification.

### 2026-05-18 — CORB-050 Closeout

Claim verified: the workstream is ready to close with remote compact v1 bridge behavior,
operator diagnostics, explicit unsupported-relay fallback, and deferred WebSocket/v2/probing
follow-ons.

Commands run from repo root:

```powershell
cargo nextest run --workspace
```

Result: passed, 745 tests run. Proves the workspace after the official relay bridge and diagnostics
changes.

Review result: `docs/workstreams/codex-official-relay-bridge/REVIEW.md` records no blocking,
important, or minor findings. Residual risks are documented as follow-ons.

## Notes

Fresh verification is required before marking a task, Codex goal, or lane complete.
