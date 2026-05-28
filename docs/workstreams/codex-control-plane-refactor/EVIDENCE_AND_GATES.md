# Evidence and Gates: Codex Control Plane

This file records fresh validation evidence for bounded refactor tasks in this workstream.

## 2026-05-28 - CP-002 / CP-401 station/config semantic closeout

Scope:

- Runtime/operator/API/GUI/TUI surfaces should be station-first.
- `config` remains only for persisted config documents, explicit legacy/v2 migration compatibility, tests, or historical examples.
- Canonical `operator/summary` should reject legacy config-shaped home payload fields, links, and capability keys.
- Old users and old config files keep migration/read compatibility through existing legacy/v2 loaders.

Evidence:

- `rg` scan for operator-facing stale active-station labels, legacy route-attempt labels, hidden config-path claims, and stale closeout phrases returned no matches in the touched runtime/operator/API/GUI/TUI/workstream surfaces.
- `crates/core/src/proxy/tests/api_admin/runtime_overrides.rs` now asserts:
  - `session_cards[*].effective_station` is present
  - `effective_config`, `last_config_name`, and `override_config_name` are absent from session cards
  - `station_persisted_config` is absent from `surface_capabilities`
  - `config_active` and `/stations/config-active` are absent from `links`
- GUI/TUI/tray/request-detail copy now uses default/effective/last station wording instead of user-facing `active_station`, `legacy`, or `config` labels.
- `CONFIG_V2_MIGRATION.md` now states that legacy `config` input is a persisted-file migration concern and that legacy `config` API path aliases are not advertised.

Commands:

- `cargo fmt --check` - passed
- `cargo nextest run -p codex-helper-core --no-fail-fast proxy_api_v1_operator_summary_reports_runtime_target_and_retry` - passed, 1 test
- `cargo check -p codex-helper-core -p codex-helper-gui -p codex-helper-tui` - passed
- `cargo nextest run -p codex-helper-gui -p codex-helper-tui --no-fail-fast` - passed, 278 tests
- `git diff --check` - passed

Residual risks:

- `active_station` remains a persisted schema/API field where it is the documented canonical config key or snapshot field.
- Legacy `configs` examples remain in migration/history docs by design.
- Post-output cross-station failover and long-horizon route provenance are separate product decisions, not CP-002 / CP-401 naming blockers.
