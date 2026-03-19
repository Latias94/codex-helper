# Operator Summary Contract

> 中文速览：这份文档把 `GET /__codex_helper/api/v1/operator/summary` 固定成未来 GUI/WebUI/外部客户端的“首页读侧 contract”。它回答的是“当前我在控制什么、运行态是什么、有哪些可见目录与能力”，而不是拿来替代深层 CRUD 或请求审计接口。

## Purpose

This document defines the read-side home-payload contract for:

```text
GET /__codex_helper/api/v1/operator/summary
```

The goal is to give future GUI/WebUI/external clients one stable top-level entry point for:

- current runtime target
- current retry/failover posture
- lightweight station health posture
- session identity cards
- station/profile/provider catalogs
- capability disclosure for remote-safe vs host-local behavior

This endpoint is intentionally **read-side first**.
It is not a replacement for normalized CRUD, deep request observability, or control-trace export surfaces.

## Discovery Rule

Clients should discover and use this payload in the following order:

1. fetch `GET /__codex_helper/api/v1/capabilities`
2. check `surface_capabilities.operator_summary`
3. if supported, fetch `GET /__codex_helper/api/v1/operator/summary` as the top-level home payload
4. use deeper endpoints only for editing, detail views, or deeper observability

Clients should not infer full write support from legacy aliases or partial endpoint fallback alone.

## Canonical Role

`operator/summary` is the top-level answer to:

- what runtime target is currently active
- what the operator can safely control from this client
- which sessions/stations/profiles/providers are currently visible
- what the current retry/health/failover posture looks like at a lightweight level

It should be treated as the backend-facing home payload for:

- attached GUI clients
- a future WebUI
- future non-GUI operator tools

It should **not** be treated as:

- a write surface
- a full replacement for station/provider/profile spec CRUD
- a full request history API
- a long-horizon audit/export API

## Canonical Top-level Shape

The payload is station-first and currently exposes these top-level fields:

| Field | Required | Purpose |
| --- | --- | --- |
| `api_version` | yes | API contract version |
| `service_name` | yes | current service (`codex`, `claude`, etc.) |
| `runtime` | yes | current runtime target/default summary |
| `counts` | yes | lightweight counts for requests/sessions/stations/profiles/providers |
| `retry` | yes | resolved retry/failover posture |
| `health` | yes in current implementation; clients should still tolerate absence | lightweight aggregated station health posture |
| `session_cards` | yes | session identity catalog |
| `stations` | yes | runtime station catalog |
| `profiles` | yes | profile catalog |
| `providers` | yes | provider catalog |
| `links` | yes in current implementation; clients should still tolerate absence on older v1 servers | semantic follow-up links for deeper read/write surfaces |
| `surface_capabilities` | yes | endpoint/write-surface discovery |
| `shared_capabilities` | yes | shared relay-visible capability disclosure |
| `host_local_capabilities` | yes | host-local-only capability disclosure |
| `remote_admin_access` | yes | remote token/access posture disclosure |

## `runtime` Sub-object

The `runtime` object is the compact answer to "what default runtime target is currently in effect?"

Canonical fields:

| Field | Meaning |
| --- | --- |
| `runtime_loaded_at_ms` | when the runtime config was loaded |
| `runtime_source_mtime_ms` | source config mtime if known |
| `configured_active_station` | persisted default station |
| `effective_active_station` | currently effective station after runtime state/fallback |
| `global_station_override` | current runtime-wide station override, if any |
| `configured_default_profile` | persisted default profile |
| `default_profile` | currently effective default profile |
| `default_profile_summary` | compact profile summary for the effective default profile |

`default_profile_summary.fast_mode` is a presentation alias for `service_tier = "priority"`.
It is not a separate persisted control dimension.

## `links` Sub-object

The `links` object exists so future clients do not need to rebuild a semantic endpoint map from flat manifest strings.

It currently points at the main follow-up surfaces used after the home payload:

| Field | Purpose |
| --- | --- |
| `snapshot` / `status_active` | richer runtime snapshot and active-request follow-up reads |
| `runtime_status` / `runtime_reload` | runtime status and reload actions |
| `status_recent` / `status_session_stats` / `status_health_checks` / `status_station_health` | deeper runtime/status reads |
| `control_trace` | deeper routing/retry audit reads |
| `retry_config` | retry/failover config read/write surface |
| `sessions` / `session_by_id_template` | session list/detail drill-down |
| `session_overrides` / `global_station_override` | runtime control surfaces |
| `stations` / `station_by_name_template` / `station_specs` / `station_spec_by_name_template` / `station_probe` / `healthcheck_start` / `healthcheck_cancel` | station overview, probe, and healthcheck control surfaces |
| `providers` / `provider_specs` / `provider_spec_by_name_template` | provider overview and spec surfaces |
| `profiles` / `profile_by_name_template` / `default_profile` / `persisted_default_profile` | profile overview and default-profile control surfaces |

These links are canonical follow-up paths, not hidden compatibility aliases.

## Layering Rules for Clients

Recommended client layering:

1. Use `operator/summary` for the first screen/home card.
2. Use `session_cards` for lightweight session identity navigation.
3. Use `stations` / `profiles` / `providers` as catalogs for overview/navigation only.
4. Use `surface_capabilities` to decide which deeper endpoints can be edited safely.
5. Use `links` to resolve the canonical follow-up endpoint paths by purpose.
6. Use deeper endpoints for actual mutation or detail views, for example:
   - station/provider spec CRUD
   - profile mutation
   - retry config mutation
   - session override mutation
   - control-trace / recent request reads

This avoids making every client rebuild the same runtime explanation logic independently.

## Compatibility and Evolution Rules

- Canonical naming is station-first.
- `links` should expose canonical follow-up paths, not compatibility-only aliases.
- Clients must prefer canonical fields such as:
  - `configured_active_station`
  - `effective_active_station`
  - `station_persisted_settings`
- Legacy aliases remain compatibility-only and must not be treated as part of the home payload contract.
- Hidden compatibility routes such as `/__codex_helper/api/v1/stations/config-active` are not part of this contract.
- Deserialize-only aliases such as `station_persisted_config` are not canonical emitted fields for new clients.
- Clients should ignore unknown additive fields so the payload can grow without reintroducing schema churn.

The payload should not emit legacy runtime terminology such as:

- `configs`
- `active_config`
- `configured_active_config`
- `effective_active_config`

## Remote-safe Interpretation Rules

Clients must treat these fields as authoritative for remote-safe behavior:

- `shared_capabilities`
- `host_local_capabilities`
- `remote_admin_access`

In particular:

- host-local transcript/history access is optional and not implied by session visibility
- remote clients can still control shared routing/session surfaces without inheriting host-local file access

## Regression Coverage

The backend regression suite now pins the canonical home payload shape in:

- `crates/core/src/proxy/tests/api_admin/runtime_overrides.rs`
  - `proxy_api_v1_operator_summary_reports_runtime_target_and_retry`

That test checks:

- canonical top-level home fields are present
- canonical runtime fields are present
- canonical semantic follow-up links are present
- station-first capability naming is emitted
- representative legacy `config` fields are absent from the home payload

## Closeout Note

This document closes the "first API payload sketches are stable enough for GUI consumption" part of the workstream.

What still remains is narrower:

- finish the last compatibility-only terminology/export cleanup
- keep future clients on top of this home-payload layering contract
- only add new backend surfaces when they remove real client complexity
