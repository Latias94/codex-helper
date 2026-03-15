use std::time::{Duration, Instant};

use futures_util::future::join_all;
use reqwest::Client;

use crate::dashboard_core::{
    ApiV1Capabilities, ControlPlaneSurfaceCapabilities, HostLocalControlPlaneCapabilities,
    RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities,
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
    pub(super) profiles: bool,
    pub(super) retry_config: bool,
    pub(super) provider_specs: bool,
    pub(super) station_specs: bool,
    pub(super) persisted_station_config: bool,
    pub(super) default_profile_override: bool,
    pub(super) session_override_reset: bool,
    pub(super) control_trace: bool,
    pub(super) station_api: bool,
    pub(super) station_runtime: bool,
    pub(super) session_override_aggregate: bool,
    pub(super) session_station: bool,
    pub(super) session_reasoning_effort: bool,
    pub(super) session_model: bool,
    pub(super) session_service_tier: bool,
}

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
        snapshot: supports_capability_flag(
            surface.snapshot,
            endpoints,
            "/__codex_helper/api/v1/snapshot",
        ),
        profiles: supports_capability_flag(
            surface.profiles,
            endpoints,
            "/__codex_helper/api/v1/profiles",
        ),
        retry_config: supports_capability_flag(
            surface.retry_config,
            endpoints,
            "/__codex_helper/api/v1/retry/config",
        ),
        provider_specs: supports_capability_flag(
            surface.provider_specs,
            endpoints,
            "/__codex_helper/api/v1/providers/specs",
        ),
        station_specs: supports_capability_flag(
            surface.station_specs,
            endpoints,
            "/__codex_helper/api/v1/stations/specs",
        ),
        persisted_station_config: supports_any_capability_flag(
            surface.station_persisted_config,
            endpoints,
            &[
                "/__codex_helper/api/v1/stations/config-active",
                "/__codex_helper/api/v1/stations/{name}",
            ],
        ),
        default_profile_override: supports_capability_flag(
            surface.default_profile_override,
            endpoints,
            "/__codex_helper/api/v1/profiles/default",
        ),
        session_override_reset: supports_capability_flag(
            surface.session_override_reset,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/reset",
        ),
        control_trace: supports_capability_flag(
            surface.control_trace,
            endpoints,
            "/__codex_helper/api/v1/control-trace",
        ),
        station_api: supports_any_capability_flag(
            surface.stations || surface.station_runtime || surface.station_probe,
            endpoints,
            &[
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/stations/runtime",
                "/__codex_helper/api/v1/stations/probe",
            ],
        ),
        station_runtime: supports_capability_flag(
            surface.station_runtime,
            endpoints,
            "/__codex_helper/api/v1/stations/runtime",
        ),
        session_override_aggregate: supports_capability_flag(
            surface.session_overrides,
            endpoints,
            "/__codex_helper/api/v1/overrides/session",
        ),
        session_station: supports_capability_flag(
            surface.session_station_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/station",
        ),
        session_reasoning_effort: supports_capability_flag(
            surface.session_reasoning_effort_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/effort",
        ),
        session_model: supports_capability_flag(
            surface.session_model_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/model",
        ),
        session_service_tier: supports_capability_flag(
            surface.session_service_tier_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/service-tier",
        ),
    }
}

impl ProxyController {
    pub fn request_attach_with_admin_base(&mut self, port: u16, admin_base_url: Option<String>) {
        let mut attached = AttachedStatus::new(port);
        if let Some(admin_base_url) = admin_base_url {
            attached.admin_base_url = admin_base_url.clone();
            if let Some(discovered) = self.discovered.iter().find(|candidate| {
                candidate.port == port && candidate.admin_base_url == admin_base_url
            }) {
                let resolved_surface =
                    resolve_api_v1_surface(&discovered.surface_capabilities, &discovered.endpoints);
                attached.api_version = discovered.api_version;
                attached.service_name = discovered.service_name.clone();
                attached.supports_control_trace_api = resolved_surface.control_trace;
                attached.shared_capabilities = discovered.shared_capabilities.clone();
                attached.host_local_capabilities = discovered.host_local_capabilities.clone();
                attached.remote_admin_access = discovered.remote_admin_access.clone();
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
        struct RuntimeConfigStatus {
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
        ) -> anyhow::Result<RuntimeConfigStatus> {
            get_json::<RuntimeConfigStatus>(
                client,
                format!("{base}/__codex_helper/api/v1/runtime/status"),
                timeout,
            )
            .await
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

                return Some(DiscoveredProxy {
                    port,
                    base_url,
                    admin_base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    surface_capabilities: c.surface_capabilities,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
                    shared_capabilities: c.shared_capabilities,
                    host_local_capabilities: c.host_local_capabilities,
                    remote_admin_access: c.remote_admin_access,
                });
            }

            let caps = get_json::<ApiV1Capabilities>(
                &client,
                format!("{base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_runtime_status(&client, &base_url, timeout).await.ok();

                return Some(DiscoveredProxy {
                    port,
                    base_url: base_url.clone(),
                    admin_base_url: base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    surface_capabilities: c.surface_capabilities,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
                    shared_capabilities: c.shared_capabilities,
                    host_local_capabilities: c.host_local_capabilities,
                    remote_admin_access: c.remote_admin_access,
                });
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

                    return Some(DiscoveredProxy {
                        port,
                        base_url,
                        admin_base_url: discovered_admin_base,
                        api_version: Some(c.api_version),
                        service_name: Some(c.service_name),
                        endpoints: c.endpoints,
                        surface_capabilities: c.surface_capabilities,
                        runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                        last_error: None,
                        shared_capabilities: c.shared_capabilities,
                        host_local_capabilities: c.host_local_capabilities,
                        remote_admin_access: c.remote_admin_access,
                    });
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
            found.sort_by_key(|proxy| proxy.port);
            Ok::<_, anyhow::Error>(found)
        };

        let found = rt.block_on(fut)?;
        self.discovered = found;
        self.last_discovery_scan = Some(Instant::now());
        Ok(())
    }
}
