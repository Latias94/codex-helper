use super::*;

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct RuntimeStatusResponse {
    #[serde(default)]
    runtime_source_path: Option<String>,
    #[serde(default)]
    config_path: Option<String>,
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

pub(super) struct RefreshResult {
    pub management_base_url: String,
    pub api_version: Option<u32>,
    pub service_name: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub session_cards: Vec<SessionIdentityCard>,
    pub global_station_override: Option<String>,
    pub configured_active_station: Option<String>,
    pub effective_active_station: Option<String>,
    pub configured_default_profile: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub providers: Vec<ProviderOption>,
    pub session_model: HashMap<String, String>,
    pub session_station: HashMap<String, String>,
    pub session_effort: HashMap<String, String>,
    pub session_service_tier: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub station_health: HashMap<String, StationHealth>,
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub pricing_catalog: ModelPriceCatalogSnapshot,
    pub lb_view: HashMap<String, LbConfigView>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub operator_health_summary: Option<OperatorHealthSummary>,
    pub operator_counts: Option<OperatorSummaryCounts>,
    pub operator_summary_links: Option<OperatorSummaryLinks>,
    pub supports_operator_summary_api: bool,
    pub supports_pricing_catalog_api: bool,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_retry_config_api: bool,
    pub persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    pub supports_provider_spec_api: bool,
    pub persisted_stations: BTreeMap<String, PersistedStationSpec>,
    pub persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    pub supports_station_spec_api: bool,
    pub supports_persisted_station_settings: bool,
    pub supports_default_profile_override: bool,
    pub supports_station_runtime_override: bool,
    pub supports_session_override_reset: bool,
    pub supports_control_trace_api: bool,
    pub supports_station_api: bool,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
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

fn linked_url(
    base: &str,
    links: Option<&OperatorSummaryLinks>,
    select: impl FnOnce(&OperatorSummaryLinks) -> &str,
    fallback: &str,
) -> String {
    let path = links.map(select).unwrap_or(fallback);
    format!("{base}{path}")
}

async fn get_v1_runtime_status(
    client: &Client,
    base: &str,
    links: Option<&OperatorSummaryLinks>,
    req_timeout: Duration,
) -> anyhow::Result<RuntimeStatusResponse> {
    get_json::<RuntimeStatusResponse>(
        client,
        linked_url(
            base,
            links,
            |summary_links| summary_links.runtime_status.as_str(),
            "/__codex_helper/api/v1/runtime/status",
        ),
        req_timeout,
    )
    .await
}

async fn get_v1_global_station_override(
    client: &Client,
    base: &str,
    links: Option<&OperatorSummaryLinks>,
    req_timeout: Duration,
) -> anyhow::Result<Option<String>> {
    get_json::<Option<String>>(
        client,
        linked_url(
            base,
            links,
            |summary_links| summary_links.global_station_override.as_str(),
            "/__codex_helper/api/v1/overrides/global-station",
        ),
        req_timeout,
    )
    .await
}

async fn get_v1_station_health(
    client: &Client,
    base: &str,
    links: Option<&OperatorSummaryLinks>,
    req_timeout: Duration,
) -> anyhow::Result<HashMap<String, StationHealth>> {
    get_json::<HashMap<String, StationHealth>>(
        client,
        linked_url(
            base,
            links,
            |summary_links| summary_links.status_station_health.as_str(),
            "/__codex_helper/api/v1/status/station-health",
        ),
        req_timeout,
    )
    .await
}

pub(super) async fn refresh_from_base(
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
    let supports_operator_summary_api = resolved_surface.operator_summary;
    let supports_profiles = resolved_surface.profiles;
    let supports_providers = resolved_surface.providers;
    let supports_retry_config_api = resolved_surface.retry_config;
    let supports_pricing_catalog_api = resolved_surface.pricing_catalog;
    let supports_provider_spec_api = resolved_surface.provider_specs;
    let supports_station_spec_api = resolved_surface.station_specs;
    let supports_default_profile_override = resolved_surface.default_profile_override;
    let supports_session_override_reset = resolved_surface.session_override_reset;
    let supports_control_trace_api = resolved_surface.control_trace;
    let supports_session_override_aggregate = resolved_surface.session_override_aggregate;
    let supports_global_station_override = resolved_surface.global_station_override;
    let supports_session_station = resolved_surface.session_station;
    let supports_session_effort = resolved_surface.session_reasoning_effort;
    let supports_persisted_station_settings = resolved_surface.persisted_station_settings;
    let supports_station_api = resolved_surface.station_api;
    let supports_station_runtime_override = resolved_surface.station_runtime;

    let operator_summary = if supports_operator_summary_api {
        get_json::<ApiV1OperatorSummary>(
            client,
            format!("{base}/__codex_helper/api/v1/operator/summary"),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };
    let operator_runtime_summary = operator_summary
        .as_ref()
        .map(|summary| summary.runtime.clone());
    let operator_retry_summary = operator_summary
        .as_ref()
        .map(|summary| summary.retry.clone());
    let operator_health_summary = operator_summary
        .as_ref()
        .and_then(|summary| summary.health.clone());
    let operator_counts = operator_summary
        .as_ref()
        .map(|summary| summary.counts.clone());
    let operator_session_cards = operator_summary
        .as_ref()
        .map(|summary| summary.session_cards.clone());
    let operator_profiles = operator_summary
        .as_ref()
        .map(|summary| summary.profiles.clone());
    let operator_providers = operator_summary
        .as_ref()
        .map(|summary| summary.providers.clone());
    let operator_stations = operator_summary
        .as_ref()
        .map(|summary| summary.stations.clone());
    let operator_summary_links = operator_summary
        .as_ref()
        .and_then(|summary| summary.links.clone());
    let operator_summary_links_ref = operator_summary_links.as_ref();
    let pricing_catalog = if supports_pricing_catalog_api {
        get_json::<ModelPriceCatalogSnapshot>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.pricing_catalog.as_str(),
                "/__codex_helper/api/v1/pricing/catalog",
            ),
            req_timeout,
        )
        .await
        .ok()
        .unwrap_or_else(bundled_model_price_catalog_snapshot)
    } else {
        bundled_model_price_catalog_snapshot()
    };
    let configured_profiles = if supports_profiles {
        get_json::<ProfilesResponse>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.profiles.as_str(),
                "/__codex_helper/api/v1/profiles",
            ),
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
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.retry_config.as_str(),
                "/__codex_helper/api/v1/retry/config",
            ),
            req_timeout,
        )
        .await
        .ok()
        .map(|response| (response.configured, response.resolved))
    } else {
        None
    };
    let configured_providers = if supports_providers {
        get_json::<Vec<ProviderOption>>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.providers.as_str(),
                "/__codex_helper/api/v1/providers",
            ),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };
    let persisted_station_catalog = if supports_station_spec_api {
        get_json::<crate::config::PersistedStationsCatalog>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.station_specs.as_str(),
                "/__codex_helper/api/v1/stations/specs",
            ),
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
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.provider_specs.as_str(),
                "/__codex_helper/api/v1/providers/specs",
            ),
            req_timeout,
        )
        .await
        .ok()
    } else {
        None
    };
    let providers = configured_providers
        .clone()
        .or(operator_providers.clone())
        .unwrap_or_default();

    if supports_snapshot {
        let api = get_json::<ApiV1Snapshot>(
            client,
            format!(
                "{}?recent_limit=600&stats_days=21",
                linked_url(
                    base,
                    operator_summary_links_ref,
                    |summary_links| summary_links.snapshot.as_str(),
                    "/__codex_helper/api/v1/snapshot",
                )
            ),
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
            })
            .or_else(|| {
                operator_runtime_summary
                    .as_ref()
                    .and_then(|summary| summary.configured_default_profile.clone())
            });
        let profiles = configured_profiles
            .as_ref()
            .map(|response| response.profiles.clone())
            .or_else(|| operator_profiles.clone())
            .unwrap_or(profiles);
        let global_station_override = operator_runtime_summary
            .as_ref()
            .and_then(|summary| summary.global_station_override.clone())
            .or_else(|| {
                snapshot
                    .effective_global_station_override()
                    .map(str::to_owned)
            });
        let station_health = snapshot.effective_station_health().clone();

        return Ok(RefreshResult {
            management_base_url: base.to_string(),
            api_version: Some(api_version),
            service_name: Some(service_name),
            active: snapshot.active,
            recent: snapshot.recent,
            session_cards: snapshot.session_cards,
            global_station_override,
            configured_active_station: operator_runtime_summary
                .as_ref()
                .and_then(|summary| summary.configured_active_station.clone())
                .or(configured_active_station),
            effective_active_station: operator_runtime_summary
                .as_ref()
                .and_then(|summary| summary.effective_active_station.clone())
                .or(effective_active_station),
            configured_default_profile,
            default_profile: operator_runtime_summary
                .as_ref()
                .and_then(|summary| summary.default_profile.clone())
                .or(default_profile),
            profiles,
            providers: providers.clone(),
            session_model: snapshot.session_model_overrides,
            session_station: snapshot.session_station_overrides,
            session_effort: snapshot.session_effort_overrides,
            session_service_tier: snapshot.session_service_tier_overrides,
            session_stats: snapshot.session_stats,
            stations,
            station_health,
            provider_balances: snapshot.provider_balances,
            health_checks: snapshot.health_checks,
            usage_rollup: snapshot.usage_rollup,
            stats_5m: snapshot.stats_5m,
            stats_1h: snapshot.stats_1h,
            pricing_catalog,
            lb_view: snapshot.lb_view,
            runtime_loaded_at_ms: operator_runtime_summary
                .as_ref()
                .and_then(|summary| summary.runtime_loaded_at_ms)
                .or(runtime_loaded_at_ms),
            runtime_source_mtime_ms: operator_runtime_summary
                .as_ref()
                .and_then(|summary| summary.runtime_source_mtime_ms)
                .or(runtime_source_mtime_ms),
            operator_runtime_summary,
            operator_retry_summary,
            operator_health_summary,
            operator_counts,
            operator_summary_links,
            supports_operator_summary_api,
            supports_pricing_catalog_api,
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
            supports_persisted_station_settings,
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
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.status_active.as_str(),
                "/__codex_helper/api/v1/status/active",
            ),
            req_timeout,
        ),
        get_json::<Vec<FinishedRequest>>(
            client,
            format!(
                "{}?limit=200",
                linked_url(
                    base,
                    operator_summary_links_ref,
                    |summary_links| summary_links.status_recent.as_str(),
                    "/__codex_helper/api/v1/status/recent",
                )
            ),
            req_timeout,
        ),
        get_v1_runtime_status(client, base, operator_summary_links_ref, req_timeout),
        async {
            if operator_runtime_summary.is_some() || !supports_global_station_override {
                Ok::<Option<String>, anyhow::Error>(None)
            } else {
                get_v1_global_station_override(
                    client,
                    base,
                    operator_summary_links_ref,
                    req_timeout,
                )
                .await
            }
        },
        get_json::<HashMap<String, SessionStats>>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.status_session_stats.as_str(),
                "/__codex_helper/api/v1/status/session-stats",
            ),
            req_timeout,
        ),
        async {
            if let Some(stations) = operator_stations.clone() {
                Ok::<Vec<StationOption>, anyhow::Error>(stations)
            } else if supports_station_api {
                get_json::<Vec<StationOption>>(
                    client,
                    linked_url(
                        base,
                        operator_summary_links_ref,
                        |summary_links| summary_links.stations.as_str(),
                        "/__codex_helper/api/v1/stations",
                    ),
                    req_timeout,
                )
                .await
            } else {
                Ok(Vec::new())
            }
        },
        get_v1_station_health(client, base, operator_summary_links_ref, req_timeout),
        get_json::<HashMap<String, HealthCheckStatus>>(
            client,
            linked_url(
                base,
                operator_summary_links_ref,
                |summary_links| summary_links.status_health_checks.as_str(),
                "/__codex_helper/api/v1/status/health-checks",
            ),
            req_timeout,
        ),
    )?;

    let supports_session_model = resolved_surface.session_model;
    let supports_session_service_tier = resolved_surface.session_service_tier;
    let (session_station, session_effort, session_model, session_service_tier) =
        if supports_session_override_aggregate {
            let aggregate = get_json::<AttachedSessionManualOverridesListResponse>(
                client,
                linked_url(
                    base,
                    operator_summary_links_ref,
                    |summary_links| summary_links.session_overrides.as_str(),
                    "/__codex_helper/api/v1/overrides/session",
                ),
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
        None => (None, None, operator_profiles.unwrap_or_default()),
    };
    let global_station_override = operator_runtime_summary
        .as_ref()
        .and_then(|summary| summary.global_station_override.clone())
        .or(global_station_override);
    let configured_active_station = operator_runtime_summary
        .as_ref()
        .and_then(|summary| summary.configured_active_station.clone());
    let effective_active_station = operator_runtime_summary
        .as_ref()
        .and_then(|summary| summary.effective_active_station.clone());
    let configured_default_profile = operator_runtime_summary
        .as_ref()
        .and_then(|summary| summary.configured_default_profile.clone())
        .or(configured_default_profile);
    let default_profile = operator_runtime_summary
        .as_ref()
        .and_then(|summary| summary.default_profile.clone())
        .or(default_profile);

    Ok(RefreshResult {
        management_base_url: base.to_string(),
        api_version: Some(api_version),
        service_name: Some(service_name),
        active,
        recent,
        session_cards: operator_session_cards.unwrap_or_default(),
        global_station_override,
        configured_active_station,
        effective_active_station,
        configured_default_profile,
        default_profile,
        profiles,
        providers,
        session_model,
        session_station,
        session_effort,
        session_service_tier,
        session_stats: stats,
        stations,
        station_health,
        provider_balances: HashMap::new(),
        health_checks,
        usage_rollup: UsageRollupView::default(),
        stats_5m: WindowStats::default(),
        stats_1h: WindowStats::default(),
        pricing_catalog,
        lb_view: HashMap::new(),
        runtime_loaded_at_ms: operator_runtime_summary
            .as_ref()
            .and_then(|summary| summary.runtime_loaded_at_ms)
            .or(Some(runtime.loaded_at_ms)),
        runtime_source_mtime_ms: operator_runtime_summary
            .as_ref()
            .and_then(|summary| summary.runtime_source_mtime_ms)
            .or(runtime.source_mtime_ms),
        operator_runtime_summary: operator_runtime_summary.clone(),
        operator_retry_summary,
        operator_health_summary,
        operator_counts,
        operator_summary_links,
        supports_operator_summary_api,
        supports_pricing_catalog_api,
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
        supports_persisted_station_settings,
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
