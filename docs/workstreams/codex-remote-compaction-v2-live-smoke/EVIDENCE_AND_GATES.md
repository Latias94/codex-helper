# Evidence And Gates

## 2026-05-26 - RCV2LS-010

Status: Complete.

Implemented:

- Added explicit-only `remote_compaction_v2` live-smoke case.
- The request sends `POST /responses`, `stream: true`, one `compaction_trigger` input item, and
  `x-codex-beta-features: remote_compaction_v2`.
- The classifier only passes when the stream has exactly one compaction output item event and a
  `response.completed` event. JSON-only compaction responses are recorded as accepted but not
  stream-proven.
- API/CLI docs include the case; CLI exposes `--compact-v2`.
- Result payloads now record `compaction_output_seen`, `compaction_output_items_seen`, and
  `response_completed_seen`.

Gates:

```powershell
cargo nextest run -p codex-helper-core codex_relay_live_smoke --no-fail-fast
cargo nextest run -p codex-helper live_smoke_cases --no-fail-fast
cargo fmt --all --check
```

Fresh results:

```powershell
cargo fmt --all --check
# passed

cargo nextest run -p codex-helper-core codex_relay_live_smoke --no-fail-fast
# 18 tests run: 18 passed

cargo nextest run -p codex-helper-core codex_live_smoke_api_runs_remote_compaction_v2_live_smoke codex_live_smoke_api_runs_compact_live_smoke --no-fail-fast
# 2 tests run: 2 passed

cargo nextest run -p codex-helper-core codex_relay_live_smoke codex_live_smoke_api codex_relay_evidence --no-fail-fast
# 25 tests run: 25 passed

cargo nextest run -p codex-helper-core --no-fail-fast
# 703 tests run: 703 passed

cargo nextest run -p codex-helper live_smoke_cases codex_relay_cli_parses_live_smoke --no-fail-fast
# 8 tests run: 8 passed

cargo nextest run -p codex-helper-tui codex_relay_live_smoke codex_relay_live_smoke_lines_show_confirmation_and_results --no-fail-fast
# 3 tests run: 3 passed
```

Manual paid relay smoke was not run during this lane. Operators can run it with:

```powershell
codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --provider <provider> --compact-v2 --json
```
