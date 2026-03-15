use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use anyhow::bail;
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::config::{
    PersistedProviderSpec, PersistedStationProviderRef, PersistedStationSpec, ResolvedRetryConfig,
    RetryConfig,
};
use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1Snapshot, ControlProfileOption, HostLocalControlPlaneCapabilities,
    RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption, WindowStats,
};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, SessionIdentityCard,
    SessionManualOverrides, SessionStats, StationHealth, UsageRollupView,
};

use super::attached_discovery::{attached_management_candidates, resolve_api_v1_surface};
use super::{AttachedStatus, ProxyController, ProxyMode, send_admin_request};

#[derive(Debug, serde::Deserialize)]
struct RuntimeConfigStatus {
    loaded_at_ms: u64,
    #[serde(default)]
    source_mtime_ms: Option<u64>,
    #[serde(default)]
    retry: Option<ResolvedRetryConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct ProfilesResponse {
    default_profile: Option<String>,
    #[serde(default)]
    configured_default_profile: Option<String>,
    #[serde(default)]
    profiles: Vec<ControlProfileOption>,
}

#[derive(Debug, serde::Deserialize)]
struct RetryConfigResponse {
    configured: RetryConfig,
    resolved: ResolvedRetryConfig,
}

#[derive(Default, serde::Deserialize)]
struct AttachedSessionManualOverridesListResponse {
    #[serde(default)]
    sessions: HashMap<String, SessionManualOverrides>,
}

struct RefreshResult {
    management_base_url: String,
    api_version: Option<u32>,
    service_name: Option<String>,
    active: Vec<ActiveRequest>,
    recent: Vec<FinishedRequest>,
    session_cards: Vec<SessionIdentityCard>,
    global_station_override: Option<String>,
    configured_active_station: Option<String>,
    effective_active_station: Option<String>,
    configured_default_profile: Option<String>,
    default_profile: Option<String>,
    profiles: Vec<ControlProfileOption>,
    session_model: HashMap<String, String>,
    session_station: HashMap<String, String>,
    session_effort: HashMap<String, String>,
    session_service_tier: HashMap<String, String>,
    session_stats: HashMap<String, SessionStats>,
    stations: Vec<StationOption>,
    station_health: HashMap<String, StationHealth>,
    health_checks: HashMap<String, HealthCheckStatus>,
    usage_rollup: UsageRollupView,
    stats_5m: WindowStats,
    stats_1h: WindowStats,
    lb_view: HashMap<String, LbConfigView>,
    runtime_loaded_at_ms: Option<u64>,
    runtime_source_mtime_ms: Option<u64>,
    configured_retry: Option<RetryConfig>,
    resolved_retry: Option<ResolvedRetryConfig>,
    supports_retry_config_api: bool,
    persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    supports_provider_spec_api: bool,
    persisted_stations: BTreeMap<String, PersistedStationSpec>,
    persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    supports_station_spec_api: bool,
    supports_persisted_station_config: bool,
    supports_default_profile_override: bool,
    supports_station_runtime_override: bool,
    supports_session_override_reset: bool,
    supports_control_trace_api: bool,
    supports_station_api: bool,
    shared_capabilities: SharedControlPlaneCapabilities,
    host_local_capabilities: HostLocalControlPlaneCapabilities,
    remote_admin_access: RemoteAdminAccessCapabilities,
}

async fn get_json<T: DeserializeOwned>(
    client: &Client,
    url: String,
    timeout: Duration,
) -> anyhow::Result<T> {
    Ok(send_admin_request(client.get(url).timeout(timeout))
        .await?
        .json::<T>()
        .await?)
}

async fn get_v1_runtime_status(
    client: &Client,
    base: &str,
    req_timeout: Duration,
) -> anyhow::Result<RuntimeConfigStatus> {
    get_json::<RuntimeConfigStatus>(
        client,
        format!("{base}/__codex_helper/api/v1/runtime/status"),
        req_timeout,
    )
    .await
}

async fn get_v1_global_station_override(
    client: &Client,
    base: &str,
    req_timeout: Duration,
) -> anyhow::Result<Option<String>> {
    get_json::<Option<String>>(
        client,
        format!("{base}/__codex_helper/api/v1/overrides/global-station"),
        req_timeout,
    )
    .await
}

async fn get_v1_station_health(
    client: &Client,
    base: &str,
    req_timeout: Duration,
) -> anyhow::Result<HashMap<String, StationHealth>> {
    get_json::<HashMap<String, StationHealth>>(
        client,
        format!("{base}/__codex_helper/api/v1/status/station-health"),
        req_timeout,
    )
    .await
}

async fn refresh_from_base(
    client: &Client,
    base: &str,
    req_timeout: Duration,
) -> anyhow::Result<RefreshResult> {
    let caps = get_json::<ApiV1Capabilities>(
        client,
        format!("{base}/__codex_helper/api/v1/capabilities"),
        req_timeout,
    )
    .await?;
    if caps.api_version != 1 {
        bail!(
            "attached proxy reported unsupported api version: {}",
            caps.api_version
        );
    }

    let ApiV1Capabilities {
        api_version,
        service_name,
        endpoints,
        surface_capabilities,
        shared_capabilities,
        host_local_capabilities,
        remote_admin_access,
    } = caps;
    let resolved_surface = resolve_api_v1_surface(&surface_capabilities, endpoints.as_slice());
    let supports_snapshot = resolved_surface.snapshot;
    let supports_profiles = resolved_surface.profiles;
    let supports_retry_config_api = resolved_surface.retry_config;
    let supports_provider_spec_api = resolved_surface.provider_specs;
    let supports_station_spec_api = resolved_surface.station_specs;
    let supports_default_profile_override = resolved_surface.default_profile_override;
    let supports_session_override_reset = resolved_surface.session_override_reset;
    let supports_control_trace_api = resolved_surface.control_trace;
    let supports_session_override_aggregate = resolved_surface.session_override_aggregate;
    let supports_session_station = resolved_surface.session_station;
    let supports_session_effort = resolved_surface.session_reasoning_effort;
    let supports_persisted_station_config = resolved_surface.persisted_station_config;
    let supports_station_api = resolved_surface.station_api;
    let supports_station_runtime_override = resolved_surface.station_runtime;

    let configured_profiles = if supports_profiles {
        get_json::<ProfilesResponse>(
            client,
            format!("{base}/__codex_helper/api/v1/profiles"),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };
    let configured_retry = if supports_retry_config_api {
        get_json::<RetryConfigResponse>(
            client,
            format!("{base}/__codex_helper/api/v1/retry/config"),
            req_timeout,
        )
        .await
        .ok()
        .map(|response| (response.configured, response.resolved))
    } else {
        None
    };
    let persisted_station_catalog = if supports_station_spec_api {
        get_json::<crate::config::PersistedStationsCatalog>(
            client,
            format!("{base}/__codex_helper/api/v1/stations/specs"),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };
    let persisted_provider_catalog = if supports_provider_spec_api {
        get_json::<crate::config::PersistedProvidersCatalog>(
            client,
            format!("{base}/__codex_helper/api/v1/providers/specs"),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };

    if supports_snapshot {
        let api = get_json::<ApiV1Snapshot>(
            client,
            format!("{base}/__codex_helper/api/v1/snapshot?recent_limit=600&stats_days=21"),
            req_timeout,
        )
        .await?;
        let ApiV1Snapshot {
            api_version,
            service_name,
            runtime_loaded_at_ms,
            runtime_source_mtime_ms,
            stations,
            configured_active_station,
            effective_active_station,
            default_profile,
            profiles,
            snapshot,
        } = api;
        let configured_default_profile = configured_profiles
            .as_ref()
            .and_then(|response| response.configured_default_profile.clone())
            .or_else(|| {
                configured_profiles
                    .as_ref()
                    .and_then(|response| response.default_profile.clone())
            });
        let profiles = configured_profiles
            .as_ref()
            .map(|response| response.profiles.clone())
            .unwrap_or(profiles);
        let global_station_override = snapshot
            .effective_global_station_override()
            .map(str::to_owned);
        let station_health = snapshot.effective_station_health().clone();

        return Ok(RefreshResult {
            management_base_url: base.to_string(),
            api_version: Some(api_version),
            service_name: Some(service_name),
            active: snapshot.active,
            recent: snapshot.recent,
            session_cards: snapshot.session_cards,
            global_station_override,
            configured_active_station,
            effective_active_station,
            configured_default_profile,
            default_profile,
            profiles,
            session_model: snapshot.session_model_overrides,
            session_station: snapshot.session_station_overrides,
            session_effort: snapshot.session_effort_overrides,
            session_service_tier: snapshot.session_service_tier_overrides,
            session_stats: snapshot.session_stats,
            stations,
            station_health,
            health_checks: snapshot.health_checks,
            usage_rollup: snapshot.usage_rollup,
            stats_5m: snapshot.stats_5m,
            stats_1h: snapshot.stats_1h,
            lb_view: snapshot.lb_view,
            runtime_loaded_at_ms,
            runtime_source_mtime_ms,
            configured_retry: configured_retry
                .as_ref()
                .map(|(configured, _)| configured.clone()),
            resolved_retry: configured_retry
                .as_ref()
                .map(|(_, resolved)| resolved.clone()),
            supports_retry_config_api,
            persisted_providers: persisted_provider_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .providers
                        .iter()
                        .cloned()
                        .map(|provider| (provider.name.clone(), provider))
                        .collect()
                })
                .unwrap_or_default(),
            supports_provider_spec_api,
            persisted_stations: persisted_station_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .stations
                        .iter()
                        .cloned()
                        .map(|station| (station.name.clone(), station))
                        .collect()
                })
                .unwrap_or_default(),
            persisted_station_providers: persisted_station_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .providers
                        .iter()
                        .cloned()
                        .map(|provider| (provider.name.clone(), provider))
                        .collect()
                })
                .unwrap_or_default(),
            supports_station_spec_api,
            supports_persisted_station_config,
            supports_default_profile_override,
            supports_station_runtime_override,
            supports_session_override_reset,
            supports_control_trace_api,
            supports_station_api,
            shared_capabilities,
            host_local_capabilities,
            remote_admin_access,
        });
    }

    let (
        active,
        recent,
        runtime,
        global_station_override,
        stats,
        stations,
        station_health,
        health_checks,
    ) = tokio::try_join!(
        get_json::<Vec<ActiveRequest>>(
            client,
            format!("{base}/__codex_helper/api/v1/status/active"),
            req_timeout,
        ),
        get_json::<Vec<FinishedRequest>>(
            client,
            format!("{base}/__codex_helper/api/v1/status/recent?limit=200"),
            req_timeout,
        ),
        get_v1_runtime_status(client, base, req_timeout),
        get_v1_global_station_override(client, base, req_timeout),
        get_json::<HashMap<String, SessionStats>>(
            client,
            format!("{base}/__codex_helper/api/v1/status/session-stats"),
            req_timeout,
        ),
        get_json::<Vec<StationOption>>(
            client,
            format!("{base}/__codex_helper/api/v1/stations"),
            req_timeout,
        ),
        get_v1_station_health(client, base, req_timeout),
        get_json::<HashMap<String, HealthCheckStatus>>(
            client,
            format!("{base}/__codex_helper/api/v1/status/health-checks"),
            req_timeout,
        ),
    )?;

    let supports_session_model = resolved_surface.session_model;
    let supports_session_service_tier = resolved_surface.session_service_tier;
    let (session_station, session_effort, session_model, session_service_tier) =
        if supports_session_override_aggregate {
            let aggregate = get_json::<AttachedSessionManualOverridesListResponse>(
                client,
                format!("{base}/__codex_helper/api/v1/overrides/session"),
                req_timeout,
            )
            .await
            .ok()
            .unwrap_or_default();
            let mut session_station = HashMap::new();
            let mut session_effort = HashMap::new();
            let mut session_model = HashMap::new();
            let mut session_service_tier = HashMap::new();
            for (session_id, overrides) in aggregate.sessions {
                if let Some(station_name) = overrides.station_name {
                    session_station.insert(session_id.clone(), station_name);
                }
                if let Some(reasoning_effort) = overrides.reasoning_effort {
                    session_effort.insert(session_id.clone(), reasoning_effort);
                }
                if let Some(model) = overrides.model {
                    session_model.insert(session_id.clone(), model);
                }
                if let Some(service_tier) = overrides.service_tier {
                    session_service_tier.insert(session_id, service_tier);
                }
            }
            (
                session_station,
                session_effort,
                session_model,
                session_service_tier,
            )
        } else {
            let session_station = if supports_session_station {
                get_json::<HashMap<String, String>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/overrides/session/station"),
                    req_timeout,
                )
                .await
                .ok()
                .unwrap_or_default()
            } else {
                HashMap::new()
            };
            let session_effort = if supports_session_effort {
                get_json::<HashMap<String, String>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/overrides/session/effort"),
                    req_timeout,
                )
                .await
                .ok()
                .unwrap_or_default()
            } else {
                HashMap::new()
            };
            let session_model = if supports_session_model {
                get_json::<HashMap<String, String>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/overrides/session/model"),
                    req_timeout,
                )
                .await
                .ok()
                .unwrap_or_default()
            } else {
                HashMap::new()
            };
            let session_service_tier = if supports_session_service_tier {
                get_json::<HashMap<String, String>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/overrides/session/service-tier"),
                    req_timeout,
                )
                .await
                .ok()
                .unwrap_or_default()
            } else {
                HashMap::new()
            };
            (
                session_station,
                session_effort,
                session_model,
                session_service_tier,
            )
        };

    let (configured_default_profile, default_profile, profiles) = match configured_profiles {
        Some(response) => (
            response
                .configured_default_profile
                .clone()
                .or_else(|| response.default_profile.clone()),
            response.default_profile,
            response.profiles,
        ),
        None => (None, None, Vec::new()),
    };

    Ok(RefreshResult {
        management_base_url: base.to_string(),
        api_version: Some(api_version),
        service_name: Some(service_name),
        active,
        recent,
        session_cards: Vec::new(),
        global_station_override,
        configured_active_station: None,
        effective_active_station: None,
        configured_default_profile,
        default_profile,
        profiles,
        session_model,
        session_station,
        session_effort,
        session_service_tier,
        session_stats: stats,
        stations,
        station_health,
        health_checks,
        usage_rollup: UsageRollupView::default(),
        stats_5m: WindowStats::default(),
        stats_1h: WindowStats::default(),
        lb_view: HashMap::new(),
        runtime_loaded_at_ms: Some(runtime.loaded_at_ms),
        runtime_source_mtime_ms: runtime.source_mtime_ms,
        configured_retry: configured_retry
            .as_ref()
            .map(|(configured, _)| configured.clone()),
        resolved_retry: configured_retry
            .as_ref()
            .map(|(_, resolved)| resolved.clone())
            .or(runtime.retry),
        supports_retry_config_api,
        persisted_providers: persisted_provider_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .providers
                    .iter()
                    .cloned()
                    .map(|provider| (provider.name.clone(), provider))
                    .collect()
            })
            .unwrap_or_default(),
        supports_provider_spec_api,
        persisted_stations: persisted_station_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .stations
                    .iter()
                    .cloned()
                    .map(|station| (station.name.clone(), station))
                    .collect()
            })
            .unwrap_or_default(),
        persisted_station_providers: persisted_station_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .providers
                    .iter()
                    .cloned()
                    .map(|provider| (provider.name.clone(), provider))
                    .collect()
            })
            .unwrap_or_default(),
        supports_station_spec_api,
        supports_persisted_station_config,
        supports_default_profile_override,
        supports_station_runtime_override,
        supports_session_override_reset,
        supports_control_trace_api,
        supports_station_api,
        shared_capabilities,
        host_local_capabilities,
        remote_admin_access,
    })
}

fn apply_refresh_result(att: &mut AttachedStatus, result: RefreshResult) {
    att.last_error = None;
    att.admin_base_url = result.management_base_url;
    att.api_version = result.api_version;
    att.service_name = result.service_name;
    att.active = result.active;
    att.recent = result.recent;
    att.session_cards = result.session_cards;
    att.global_station_override = result.global_station_override;
    att.configured_active_station = result.configured_active_station;
    att.effective_active_station = result.effective_active_station;
    att.configured_default_profile = result.configured_default_profile;
    att.default_profile = result.default_profile;
    att.profiles = result.profiles;
    att.session_model_overrides = result.session_model;
    att.session_station_overrides = result.session_station;
    att.session_effort_overrides = result.session_effort;
    att.session_service_tier_overrides = result.session_service_tier;
    att.session_stats = result.session_stats;
    att.stations = result.stations;
    att.station_health = result.station_health;
    att.health_checks = result.health_checks;
    att.usage_rollup = result.usage_rollup;
    att.stats_5m = result.stats_5m;
    att.stats_1h = result.stats_1h;
    att.lb_view = result.lb_view;
    att.runtime_loaded_at_ms = result.runtime_loaded_at_ms;
    att.runtime_source_mtime_ms = result.runtime_source_mtime_ms;
    att.configured_retry = result.configured_retry;
    att.resolved_retry = result.resolved_retry;
    att.supports_retry_config_api = result.supports_retry_config_api;
    att.persisted_providers = result.persisted_providers;
    att.supports_provider_spec_api = result.supports_provider_spec_api;
    att.persisted_stations = result.persisted_stations;
    att.persisted_station_providers = result.persisted_station_providers;
    att.supports_station_spec_api = result.supports_station_spec_api;
    att.supports_persisted_station_config = result.supports_persisted_station_config;
    att.supports_default_profile_override = result.supports_default_profile_override;
    att.supports_station_runtime_override = result.supports_station_runtime_override;
    att.supports_session_override_reset = result.supports_session_override_reset;
    att.supports_control_trace_api = result.supports_control_trace_api;
    att.supports_station_api = result.supports_station_api;
    att.shared_capabilities = result.shared_capabilities;
    att.host_local_capabilities = result.host_local_capabilities;
    att.remote_admin_access = result.remote_admin_access;
}

impl ProxyController {
    pub fn refresh_attached_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        let refresh_every = refresh_every.max(Duration::from_secs(1));
        let base_candidates = match &mut self.mode {
            ProxyMode::Attached(att) => {
                if let Some(last_refresh) = att.last_refresh
                    && last_refresh.elapsed() < refresh_every
                {
                    return;
                }
                att.last_refresh = Some(Instant::now());
                attached_management_candidates(att)
            }
            _ => return,
        };

        let client = self.http_client.clone();
        let fut = async move {
            let req_timeout = Duration::from_millis(800);
            let mut last_err: Option<anyhow::Error> = None;
            for base in base_candidates {
                match refresh_from_base(&client, &base, req_timeout).await {
                    Ok(result) => return Ok::<_, anyhow::Error>(result),
                    Err(err) => last_err = Some(err),
                }
            }

            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("attach refresh failed")))
        };

        match rt.block_on(fut) {
            Ok(result) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    apply_refresh_result(att, result);
                }
            }
            Err(err) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    att.last_error = Some(err.to_string());
                }
            }
        }
    }
}
