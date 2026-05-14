use std::time::{Duration, Instant};

use futures_util::future::join_all;
use reqwest::Client;

use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ControlPlaneSurfaceCapabilities,
    HostLocalControlPlaneCapabilities, OperatorHealthSummary, OperatorRetrySummary,
    OperatorRuntimeSummary, OperatorSummaryCounts, RemoteAdminAccessCapabilities,
    SharedControlPlaneCapabilities,
};
use crate::proxy::{local_admin_base_url_for_proxy_port, local_proxy_base_url};

use super::{AttachedStatus, ProxyController, send_admin_request};

#[derive(Debug, Clone)]
pub struct DiscoveredProxy {
    pub port: u16,
    pub base_url: String,
    pub admin_base_url: String,
    pub api_version: Option<u32>,
    pub service_name: Option<String>,
    pub endpoints: Vec<String>,
    pub surface_capabilities: ControlPlaneSurfaceCapabilities,
    pub runtime_loaded_at_ms: Option<u64>,
    pub operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub operator_health_summary: Option<OperatorHealthSummary>,
    pub operator_counts: Option<OperatorSummaryCounts>,
    pub last_error: Option<String>,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
}

pub(super) fn attached_management_candidates(att: &AttachedStatus) -> Vec<String> {
    let mut out = vec![att.admin_base_url.clone()];
    if att.base_url != att.admin_base_url {
        out.push(att.base_url.clone());
    }
    out
}

pub(super) fn local_shared_control_plane_capabilities() -> SharedControlPlaneCapabilities {
    SharedControlPlaneCapabilities {
        session_observability: true,
        request_history: true,
    }
}

pub(super) fn local_host_local_control_plane_capabilities() -> HostLocalControlPlaneCapabilities {
    let host_local_history = crate::config::codex_sessions_dir().is_dir();
    HostLocalControlPlaneCapabilities {
        session_history: host_local_history,
        transcript_read: host_local_history,
        cwd_enrichment: host_local_history,
    }
}

pub(super) fn local_remote_admin_access_capabilities() -> RemoteAdminAccessCapabilities {
    RemoteAdminAccessCapabilities {
        loopback_without_token: true,
        remote_requires_token: true,
        remote_enabled: std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
            .ok()
            .is_some_and(|value| !value.trim().is_empty()),
        token_header: crate::proxy::ADMIN_TOKEN_HEADER.to_string(),
        token_env_var: crate::proxy::ADMIN_TOKEN_ENV_VAR.to_string(),
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ResolvedApiV1Surface {
    pub(super) snapshot: bool,
    pub(super) operator_summary: bool,
    pub(super) profiles: bool,
    pub(super) providers: bool,
    pub(super) retry_config: bool,
    pub(super) pricing_catalog: bool,
    pub(super) provider_balance_refresh: bool,
    pub(super) provider_specs: bool,
    pub(super) station_specs: bool,
    pub(super) default_profile_override: bool,
    pub(super) session_override_reset: bool,
    pub(super) control_trace: bool,
    pub(super) request_ledger_recent: bool,
    pub(super) request_ledger_summary: bool,
    pub(super) routing_explain: bool,
    pub(super) station_api: bool,
    pub(super) station_runtime: bool,
    pub(super) session_override_aggregate: bool,
    pub(super) global_station_override: bool,
    pub(super) global_route_override: bool,
    pub(super) session_station: bool,
    pub(super) session_route: bool,
    pub(super) session_reasoning_effort: bool,
    pub(super) session_model: bool,
    pub(super) session_service_tier: bool,
}

const API_V1_SNAPSHOT_ENDPOINT: &str = "/__codex_helper/api/v1/snapshot";
const API_V1_OPERATOR_SUMMARY_ENDPOINT: &str = "/__codex_helper/api/v1/operator/summary";
const API_V1_PROFILES_ENDPOINT: &str = "/__codex_helper/api/v1/profiles";
const API_V1_PROVIDERS_ENDPOINT: &str = "/__codex_helper/api/v1/providers";
const API_V1_PROVIDERS_RUNTIME_ENDPOINT: &str = "/__codex_helper/api/v1/providers/runtime";
const API_V1_PROVIDERS_BALANCES_REFRESH_ENDPOINT: &str =
    "/__codex_helper/api/v1/providers/balances/refresh";
const API_V1_RETRY_CONFIG_ENDPOINT: &str = "/__codex_helper/api/v1/retry/config";
const API_V1_PROVIDER_SPECS_ENDPOINT: &str = "/__codex_helper/api/v1/providers/specs";
const API_V1_STATIONS_ENDPOINT: &str = "/__codex_helper/api/v1/stations";
const API_V1_STATIONS_RUNTIME_ENDPOINT: &str = "/__codex_helper/api/v1/stations/runtime";
const API_V1_STATION_SPECS_ENDPOINT: &str = "/__codex_helper/api/v1/stations/specs";
const API_V1_DEFAULT_PROFILE_ENDPOINT: &str = "/__codex_helper/api/v1/profiles/default";
const API_V1_SESSION_OVERRIDE_RESET_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/reset";
const API_V1_CONTROL_TRACE_ENDPOINT: &str = "/__codex_helper/api/v1/control-trace";
const API_V1_REQUEST_LEDGER_RECENT_ENDPOINT: &str = "/__codex_helper/api/v1/request-ledger/recent";
const API_V1_REQUEST_LEDGER_SUMMARY_ENDPOINT: &str =
    "/__codex_helper/api/v1/request-ledger/summary";
const API_V1_ROUTING_EXPLAIN_ENDPOINT: &str = "/__codex_helper/api/v1/routing/explain";
const API_V1_PRICING_CATALOG_ENDPOINT: &str = "/__codex_helper/api/v1/pricing/catalog";
const API_V1_STATION_PROBE_ENDPOINT: &str = "/__codex_helper/api/v1/stations/probe";
const API_V1_SESSION_OVERRIDES_ENDPOINT: &str = "/__codex_helper/api/v1/overrides/session";
const API_V1_GLOBAL_STATION_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/global-station";
const API_V1_GLOBAL_ROUTE_OVERRIDE_ENDPOINT: &str = "/__codex_helper/api/v1/overrides/global-route";
const API_V1_SESSION_STATION_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/station";
const API_V1_SESSION_ROUTE_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/route";
const API_V1_SESSION_REASONING_EFFORT_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/effort";
const API_V1_SESSION_MODEL_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/model";
const API_V1_SESSION_SERVICE_TIER_OVERRIDE_ENDPOINT: &str =
    "/__codex_helper/api/v1/overrides/session/service-tier";

const API_V1_PROVIDER_SURFACE_ENDPOINTS: &[&str] =
    &[API_V1_PROVIDERS_ENDPOINT, API_V1_PROVIDERS_RUNTIME_ENDPOINT];
const API_V1_STATION_API_SURFACE_ENDPOINTS: &[&str] = &[
    API_V1_STATIONS_ENDPOINT,
    API_V1_STATIONS_RUNTIME_ENDPOINT,
    API_V1_STATION_PROBE_ENDPOINT,
];

fn supports_capability_flag(flag: bool, endpoints: &[String], endpoint: &str) -> bool {
    flag || endpoints.iter().any(|candidate| candidate == endpoint)
}

fn supports_any_capability_flag(
    flag: bool,
    endpoints: &[String],
    endpoint_candidates: &[&str],
) -> bool {
    flag || endpoint_candidates
        .iter()
        .any(|endpoint| supports_capability_flag(false, endpoints, endpoint))
}

pub(super) fn resolve_api_v1_surface(
    surface: &ControlPlaneSurfaceCapabilities,
    endpoints: &[String],
) -> ResolvedApiV1Surface {
    ResolvedApiV1Surface {
        snapshot: supports_capability_flag(surface.snapshot, endpoints, API_V1_SNAPSHOT_ENDPOINT),
        operator_summary: supports_capability_flag(
            surface.operator_summary,
            endpoints,
            API_V1_OPERATOR_SUMMARY_ENDPOINT,
        ),
        profiles: supports_capability_flag(surface.profiles, endpoints, API_V1_PROFILES_ENDPOINT),
        providers: supports_any_capability_flag(
            surface.providers || surface.provider_runtime,
            endpoints,
            API_V1_PROVIDER_SURFACE_ENDPOINTS,
        ),
        retry_config: supports_capability_flag(
            surface.retry_config,
            endpoints,
            API_V1_RETRY_CONFIG_ENDPOINT,
        ),
        pricing_catalog: supports_capability_flag(
            surface.pricing_catalog,
            endpoints,
            API_V1_PRICING_CATALOG_ENDPOINT,
        ),
        provider_balance_refresh: supports_capability_flag(
            surface.provider_balance_refresh,
            endpoints,
            API_V1_PROVIDERS_BALANCES_REFRESH_ENDPOINT,
        ),
        provider_specs: supports_capability_flag(
            surface.provider_specs,
            endpoints,
            API_V1_PROVIDER_SPECS_ENDPOINT,
        ),
        station_specs: supports_capability_flag(
            surface.station_specs,
            endpoints,
            API_V1_STATION_SPECS_ENDPOINT,
        ),
        default_profile_override: supports_capability_flag(
            surface.default_profile_override,
            endpoints,
            API_V1_DEFAULT_PROFILE_ENDPOINT,
        ),
        session_override_reset: supports_capability_flag(
            surface.session_override_reset,
            endpoints,
            API_V1_SESSION_OVERRIDE_RESET_ENDPOINT,
        ),
        control_trace: supports_capability_flag(
            surface.control_trace,
            endpoints,
            API_V1_CONTROL_TRACE_ENDPOINT,
        ),
        request_ledger_recent: supports_capability_flag(
            surface.request_ledger_recent,
            endpoints,
            API_V1_REQUEST_LEDGER_RECENT_ENDPOINT,
        ),
        request_ledger_summary: supports_capability_flag(
            surface.request_ledger_summary,
            endpoints,
            API_V1_REQUEST_LEDGER_SUMMARY_ENDPOINT,
        ),
        routing_explain: supports_capability_flag(
            surface.routing_explain,
            endpoints,
            API_V1_ROUTING_EXPLAIN_ENDPOINT,
        ),
        station_api: supports_any_capability_flag(
            surface.stations || surface.station_runtime || surface.station_probe,
            endpoints,
            API_V1_STATION_API_SURFACE_ENDPOINTS,
        ),
        station_runtime: supports_capability_flag(
            surface.station_runtime,
            endpoints,
            API_V1_STATIONS_RUNTIME_ENDPOINT,
        ),
        session_override_aggregate: supports_capability_flag(
            surface.session_overrides,
            endpoints,
            API_V1_SESSION_OVERRIDES_ENDPOINT,
        ),
        global_station_override: supports_capability_flag(
            surface.global_station_override,
            endpoints,
            API_V1_GLOBAL_STATION_OVERRIDE_ENDPOINT,
        ),
        global_route_override: supports_capability_flag(
            surface.global_route_override,
            endpoints,
            API_V1_GLOBAL_ROUTE_OVERRIDE_ENDPOINT,
        ),
        session_station: supports_capability_flag(
            surface.session_station_override,
            endpoints,
            API_V1_SESSION_STATION_OVERRIDE_ENDPOINT,
        ),
        session_route: supports_capability_flag(
            surface.session_route_override,
            endpoints,
            API_V1_SESSION_ROUTE_OVERRIDE_ENDPOINT,
        ),
        session_reasoning_effort: supports_capability_flag(
            surface.session_reasoning_effort_override,
            endpoints,
            API_V1_SESSION_REASONING_EFFORT_OVERRIDE_ENDPOINT,
        ),
        session_model: supports_capability_flag(
            surface.session_model_override,
            endpoints,
            API_V1_SESSION_MODEL_OVERRIDE_ENDPOINT,
        ),
        session_service_tier: supports_capability_flag(
            surface.session_service_tier_override,
            endpoints,
            API_V1_SESSION_SERVICE_TIER_OVERRIDE_ENDPOINT,
        ),
    }
}

fn apply_resolved_surface(attached: &mut AttachedStatus, resolved_surface: ResolvedApiV1Surface) {
    attached.supports_operator_summary_api = resolved_surface.operator_summary;
    attached.supports_retry_config_api = resolved_surface.retry_config;
    attached.supports_pricing_catalog_api = resolved_surface.pricing_catalog;
    attached.supports_provider_balance_refresh_api = resolved_surface.provider_balance_refresh;
    attached.supports_provider_spec_api = resolved_surface.provider_specs;
    attached.supports_station_spec_api = resolved_surface.station_specs;
    attached.supports_default_profile_override = resolved_surface.default_profile_override;
    attached.supports_station_runtime_override = resolved_surface.station_runtime;
    attached.supports_session_route_target_override = resolved_surface.session_route;
    attached.supports_global_route_target_override = resolved_surface.global_route_override;
    attached.supports_session_override_reset = resolved_surface.session_override_reset;
    attached.supports_control_trace_api = resolved_surface.control_trace;
    attached.supports_request_ledger_api = resolved_surface.request_ledger_recent;
    attached.supports_request_ledger_summary_api = resolved_surface.request_ledger_summary;
    attached.supports_routing_explain_api = resolved_surface.routing_explain;
    attached.supports_station_api = resolved_surface.station_api;
}

fn apply_discovered_proxy(attached: &mut AttachedStatus, discovered: &DiscoveredProxy) {
    let resolved_surface =
        resolve_api_v1_surface(&discovered.surface_capabilities, &discovered.endpoints);
    attached.base_url = discovered.base_url.clone();
    attached.admin_base_url = discovered.admin_base_url.clone();
    attached.api_version = discovered.api_version;
    attached.service_name = discovered.service_name.clone();
    attached.runtime_loaded_at_ms = discovered.runtime_loaded_at_ms;
    attached.operator_runtime_summary = discovered.operator_runtime_summary.clone();
    attached.operator_retry_summary = discovered.operator_retry_summary.clone();
    attached.operator_health_summary = discovered.operator_health_summary.clone();
    attached.operator_counts = discovered.operator_counts.clone();
    if let Some(runtime) = discovered.operator_runtime_summary.as_ref() {
        attached.runtime_source_mtime_ms = runtime.runtime_source_mtime_ms;
        attached.configured_active_station = runtime.configured_active_station.clone();
        attached.effective_active_station = runtime.effective_active_station.clone();
        attached.global_station_override = runtime.global_station_override.clone();
        attached.global_route_target_override = runtime.global_route_target_override.clone();
        attached.configured_default_profile = runtime.configured_default_profile.clone();
        attached.default_profile = runtime.default_profile.clone();
    }
    apply_resolved_surface(attached, resolved_surface);
    attached.shared_capabilities = discovered.shared_capabilities.clone();
    attached.host_local_capabilities = discovered.host_local_capabilities.clone();
    attached.remote_admin_access = discovered.remote_admin_access.clone();
}

fn discovery_surface_score(surface: &ControlPlaneSurfaceCapabilities) -> u32 {
    [
        surface.operator_summary,
        surface.retry_config,
        surface.stations,
        surface.station_runtime,
        surface.station_specs,
        surface.station_probe,
        surface.providers,
        surface.provider_runtime,
        surface.provider_specs,
        surface.profiles,
        surface.default_profile_override,
        surface.persisted_default_profile,
        surface.profile_mutation,
        surface.session_overrides,
        surface.session_profile_override,
        surface.session_model_override,
        surface.session_reasoning_effort_override,
        surface.session_station_override,
        surface.session_route_override,
        surface.session_service_tier_override,
        surface.session_override_reset,
        surface.global_station_override,
        surface.global_route_override,
        surface.control_trace,
        surface.request_ledger_recent,
        surface.request_ledger_summary,
        surface.routing_explain,
        surface.pricing_catalog,
        surface.runtime_reload,
        surface.healthcheck_start,
        surface.healthcheck_cancel,
    ]
    .into_iter()
    .filter(|flag| *flag)
    .count() as u32
}

fn discovery_catalog_score(proxy: &DiscoveredProxy) -> u32 {
    proxy
        .operator_counts
        .as_ref()
        .map(|counts| {
            (counts.active_requests as u32) * 1000
                + (counts.sessions as u32) * 100
                + (counts.stations as u32) * 10
                + (counts.profiles as u32) * 2
                + counts.providers as u32
        })
        .unwrap_or(0)
}

fn has_operator_home_summary(proxy: &DiscoveredProxy) -> bool {
    proxy.operator_runtime_summary.is_some()
        || proxy.operator_retry_summary.is_some()
        || proxy.operator_health_summary.is_some()
        || proxy.operator_counts.is_some()
}

fn sort_discovered_proxies(found: &mut [DiscoveredProxy]) {
    found.sort_by(|left, right| {
        has_operator_home_summary(right)
            .cmp(&has_operator_home_summary(left))
            .then_with(|| {
                right
                    .remote_admin_access
                    .remote_enabled
                    .cmp(&left.remote_admin_access.remote_enabled)
            })
            .then_with(|| {
                discovery_surface_score(&right.surface_capabilities)
                    .cmp(&discovery_surface_score(&left.surface_capabilities))
            })
            .then_with(|| discovery_catalog_score(right).cmp(&discovery_catalog_score(left)))
            .then_with(|| right.runtime_loaded_at_ms.cmp(&left.runtime_loaded_at_ms))
            .then_with(|| right.api_version.cmp(&left.api_version))
            .then_with(|| left.port.cmp(&right.port))
    });
}

impl ProxyController {
    pub fn request_attach_with_admin_base(&mut self, port: u16, admin_base_url: Option<String>) {
        self.clear_background_refresh();
        self.clear_provider_balance_refresh();
        let mut attached = AttachedStatus::new(port);
        if let Some(admin_base_url) = admin_base_url {
            attached.admin_base_url = admin_base_url.clone();
            if let Some(discovered) = self.discovered.iter().find(|candidate| {
                candidate.port == port && candidate.admin_base_url == admin_base_url
            }) {
                apply_discovered_proxy(&mut attached, discovered);
            }
        }
        self.mode = super::ProxyMode::Attached(attached);
        self.last_start_error = None;
        self.port_in_use_modal = None;
    }

    pub fn scan_local_proxies(
        &mut self,
        rt: &tokio::runtime::Runtime,
        ports: std::ops::RangeInclusive<u16>,
    ) -> anyhow::Result<()> {
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct RuntimeStatusResponse {
            #[serde(default)]
            runtime_source_path: Option<String>,
            #[serde(default)]
            config_path: Option<String>,
            loaded_at_ms: u64,
        }

        #[derive(Debug, serde::Deserialize)]
        struct AdminDiscovery {
            admin_base_url: String,
        }

        async fn get_json<T: serde::de::DeserializeOwned>(
            client: &Client,
            url: String,
            timeout: Duration,
        ) -> anyhow::Result<T> {
            Ok(send_admin_request(client.get(url).timeout(timeout))
                .await?
                .json::<T>()
                .await?)
        }

        async fn get_runtime_status(
            client: &Client,
            base: &str,
            timeout: Duration,
        ) -> anyhow::Result<RuntimeStatusResponse> {
            get_json::<RuntimeStatusResponse>(
                client,
                format!("{base}/__codex_helper/api/v1/runtime/status"),
                timeout,
            )
            .await
        }

        async fn get_operator_summary(
            client: &Client,
            base: &str,
            surface: &ControlPlaneSurfaceCapabilities,
            endpoints: &[String],
            timeout: Duration,
        ) -> Option<ApiV1OperatorSummary> {
            if !supports_capability_flag(
                surface.operator_summary,
                endpoints,
                API_V1_OPERATOR_SUMMARY_ENDPOINT,
            ) {
                return None;
            }

            get_json::<ApiV1OperatorSummary>(
                client,
                format!("{base}{API_V1_OPERATOR_SUMMARY_ENDPOINT}"),
                timeout,
            )
            .await
            .ok()
        }

        fn build_discovered_proxy(
            port: u16,
            base_url: String,
            admin_base_url: String,
            capabilities: ApiV1Capabilities,
            runtime: Option<RuntimeStatusResponse>,
            operator_summary: Option<ApiV1OperatorSummary>,
        ) -> DiscoveredProxy {
            let runtime_loaded_at_ms = operator_summary
                .as_ref()
                .and_then(|summary| summary.runtime.runtime_loaded_at_ms)
                .or_else(|| runtime.as_ref().map(|status| status.loaded_at_ms));

            DiscoveredProxy {
                port,
                base_url,
                admin_base_url,
                api_version: Some(capabilities.api_version),
                service_name: Some(capabilities.service_name),
                endpoints: capabilities.endpoints,
                surface_capabilities: capabilities.surface_capabilities,
                runtime_loaded_at_ms,
                operator_runtime_summary: operator_summary
                    .as_ref()
                    .map(|summary| summary.runtime.clone()),
                operator_retry_summary: operator_summary
                    .as_ref()
                    .map(|summary| summary.retry.clone()),
                operator_health_summary: operator_summary
                    .as_ref()
                    .and_then(|summary| summary.health.clone()),
                operator_counts: operator_summary
                    .as_ref()
                    .map(|summary| summary.counts.clone()),
                last_error: None,
                shared_capabilities: capabilities.shared_capabilities,
                host_local_capabilities: capabilities.host_local_capabilities,
                remote_admin_access: capabilities.remote_admin_access,
            }
        }

        async fn scan_port(client: Client, port: u16) -> Option<DiscoveredProxy> {
            let base_url = local_proxy_base_url(port);
            let admin_base_url = local_admin_base_url_for_proxy_port(port);
            let timeout = Duration::from_millis(250);

            let caps = get_json::<ApiV1Capabilities>(
                &client,
                format!("{admin_base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_runtime_status(&client, &admin_base_url, timeout)
                    .await
                    .ok();
                let operator_summary = get_operator_summary(
                    &client,
                    &admin_base_url,
                    &c.surface_capabilities,
                    &c.endpoints,
                    timeout,
                )
                .await;

                return Some(build_discovered_proxy(
                    port,
                    base_url,
                    admin_base_url,
                    c,
                    runtime,
                    operator_summary,
                ));
            }

            let caps = get_json::<ApiV1Capabilities>(
                &client,
                format!("{base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_runtime_status(&client, &base_url, timeout).await.ok();
                let operator_summary = get_operator_summary(
                    &client,
                    &base_url,
                    &c.surface_capabilities,
                    &c.endpoints,
                    timeout,
                )
                .await;

                return Some(build_discovered_proxy(
                    port,
                    base_url.clone(),
                    base_url,
                    c,
                    runtime,
                    operator_summary,
                ));
            }

            let discovered_admin_base = get_json::<AdminDiscovery>(
                &client,
                format!("{base_url}/.well-known/codex-helper-admin"),
                timeout,
            )
            .await
            .ok()
            .map(|discovery| discovery.admin_base_url.trim_end_matches('/').to_string());

            if let Some(discovered_admin_base) = discovered_admin_base
                && discovered_admin_base != admin_base_url
                && discovered_admin_base != base_url
            {
                let caps = get_json::<ApiV1Capabilities>(
                    &client,
                    format!("{discovered_admin_base}/__codex_helper/api/v1/capabilities"),
                    timeout,
                )
                .await;

                if let Ok(c) = caps {
                    let runtime = get_runtime_status(&client, &discovered_admin_base, timeout)
                        .await
                        .ok();
                    let operator_summary = get_operator_summary(
                        &client,
                        &discovered_admin_base,
                        &c.surface_capabilities,
                        &c.endpoints,
                        timeout,
                    )
                    .await;

                    return Some(build_discovered_proxy(
                        port,
                        base_url,
                        discovered_admin_base,
                        c,
                        runtime,
                        operator_summary,
                    ));
                }
            }

            None
        }

        let client = self.http_client.clone();
        let ports_vec = ports.collect::<Vec<_>>();
        let fut = async move {
            let tasks = ports_vec
                .into_iter()
                .map(|port| scan_port(client.clone(), port));
            let mut found = join_all(tasks)
                .await
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            sort_discovered_proxies(found.as_mut_slice());
            Ok::<_, anyhow::Error>(found)
        };

        let found = rt.block_on(fut)?;
        self.discovered = found;
        self.last_discovery_scan = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_proxy(port: u16) -> DiscoveredProxy {
        DiscoveredProxy {
            port,
            base_url: format!("http://127.0.0.1:{port}"),
            admin_base_url: format!("http://127.0.0.1:{}", port + 1000),
            api_version: Some(1),
            service_name: Some("codex".to_string()),
            endpoints: Vec::new(),
            surface_capabilities: ControlPlaneSurfaceCapabilities::default(),
            runtime_loaded_at_ms: None,
            operator_runtime_summary: None,
            operator_retry_summary: None,
            operator_health_summary: None,
            operator_counts: None,
            last_error: None,
            shared_capabilities: SharedControlPlaneCapabilities::default(),
            host_local_capabilities: HostLocalControlPlaneCapabilities::default(),
            remote_admin_access: RemoteAdminAccessCapabilities::default(),
        }
    }

    #[test]
    fn sort_discovered_proxies_prefers_operator_home_and_richer_surface() {
        let mut minimal = sample_proxy(3212);
        minimal.surface_capabilities.stations = true;

        let mut richer = sample_proxy(3211);
        richer.surface_capabilities.operator_summary = true;
        richer.surface_capabilities.retry_config = true;
        richer.surface_capabilities.station_specs = true;
        richer.remote_admin_access.remote_enabled = true;

        let mut richest = sample_proxy(3213);
        richest.surface_capabilities.operator_summary = true;
        richest.surface_capabilities.retry_config = true;
        richest.surface_capabilities.station_specs = true;
        richest.surface_capabilities.provider_specs = true;
        richest.operator_counts = Some(OperatorSummaryCounts {
            active_requests: 1,
            recent_requests: 2,
            sessions: 5,
            stations: 3,
            profiles: 4,
            providers: 6,
        });
        richest.operator_runtime_summary = Some(OperatorRuntimeSummary {
            runtime_loaded_at_ms: Some(800),
            runtime_source_mtime_ms: None,
            configured_active_station: None,
            effective_active_station: None,
            global_station_override: None,
            global_route_target_override: None,
            configured_default_profile: None,
            default_profile: None,
            default_profile_summary: None,
        });
        richest.runtime_loaded_at_ms = Some(800);

        let mut proxies = vec![minimal, richest, richer];
        sort_discovered_proxies(proxies.as_mut_slice());

        assert_eq!(proxies[0].port, 3213);
        assert_eq!(proxies[1].port, 3211);
        assert_eq!(proxies[2].port, 3212);
    }
}
