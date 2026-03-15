use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;

use super::ProxyService;
use super::api_responses::{ProfilesResponse, make_profiles_response};
use super::control_plane_service::{
    load_persisted_config_v2, runtime_service_manager_mut, save_proxy_config_v2_and_reload,
    save_runtime_config_and_reload, save_runtime_profiles_config_and_reload, service_view_v2,
    service_view_v2_mut,
};

fn default_persisted_station_enabled() -> bool {
    true
}

fn default_persisted_station_level() -> u8 {
    1
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationUpdateRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    level: Option<u8>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationSpecUpsertRequest {
    #[serde(default)]
    alias: Option<String>,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default = "default_persisted_station_level")]
    level: u8,
    #[serde(default)]
    members: Vec<crate::config::GroupMemberRefV2>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProviderEndpointSpecUpsertRequest {
    name: String,
    base_url: String,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default)]
    priority: u32,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProviderSpecUpsertRequest {
    #[serde(default)]
    alias: Option<String>,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default)]
    auth_token_env: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    endpoints: Vec<PersistedProviderEndpointSpecUpsertRequest>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProfileUpsertRequest {
    #[serde(default)]
    extends: Option<String>,
    #[serde(default, alias = "config")]
    station: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationActiveRequest {
    #[serde(default)]
    station_name: Option<String>,
}

impl PersistedStationActiveRequest {
    fn station_name(&self) -> Option<String> {
        self.station_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedDefaultProfileRequest {
    profile_name: Option<String>,
}

fn sanitize_profile_name(profile_name: &str) -> Result<String, (StatusCode, String)> {
    let profile_name = profile_name.trim();
    if profile_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "profile name is required".to_string(),
        ));
    }
    Ok(profile_name.to_string())
}

fn sanitize_station_name(station_name: &str) -> Result<String, (StatusCode, String)> {
    let station_name = station_name.trim();
    if station_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "station name is required".to_string(),
        ));
    }
    Ok(station_name.to_string())
}

fn sanitize_provider_name(provider_name: &str) -> Result<String, (StatusCode, String)> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provider name is required".to_string(),
        ));
    }
    Ok(provider_name.to_string())
}

fn sanitize_profile_request(
    payload: PersistedProfileUpsertRequest,
) -> crate::config::ServiceControlProfile {
    fn normalize(value: Option<String>) -> Option<String> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    crate::config::ServiceControlProfile {
        extends: normalize(payload.extends),
        station: normalize(payload.station),
        model: normalize(payload.model),
        reasoning_effort: normalize(payload.reasoning_effort),
        service_tier: normalize(payload.service_tier),
    }
}

fn normalize_optional_config_string(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn sanitize_station_spec_request(
    payload: PersistedStationSpecUpsertRequest,
) -> Result<crate::config::PersistedStationSpec, (StatusCode, String)> {
    let mut members = Vec::new();
    for member in payload.members {
        let provider = member.provider.trim();
        if provider.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "station member provider is required".to_string(),
            ));
        }

        let mut endpoint_names = member
            .endpoint_names
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        endpoint_names.dedup();

        members.push(crate::config::GroupMemberRefV2 {
            provider: provider.to_string(),
            endpoint_names,
            preferred: member.preferred,
        });
    }

    Ok(crate::config::PersistedStationSpec {
        name: String::new(),
        alias: normalize_optional_config_string(payload.alias),
        enabled: payload.enabled,
        level: payload.level.clamp(1, 10),
        members,
    })
}

fn sanitize_provider_spec_request(
    payload: PersistedProviderSpecUpsertRequest,
) -> Result<crate::config::PersistedProviderSpec, (StatusCode, String)> {
    let mut endpoints = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for endpoint in payload.endpoints {
        let endpoint_name = endpoint.name.trim();
        if endpoint_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "provider endpoint name is required".to_string(),
            ));
        }
        let base_url = endpoint.base_url.trim();
        if base_url.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("provider endpoint '{}' base_url is required", endpoint_name),
            ));
        }
        if !seen.insert(endpoint_name.to_string()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate provider endpoint '{}'", endpoint_name),
            ));
        }

        endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.to_string(),
            base_url: base_url.to_string(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
        });
    }

    Ok(crate::config::PersistedProviderSpec {
        name: String::new(),
        alias: normalize_optional_config_string(payload.alias),
        enabled: payload.enabled,
        auth_token_env: normalize_optional_config_string(payload.auth_token_env),
        api_key_env: normalize_optional_config_string(payload.api_key_env),
        endpoints,
    })
}

fn merge_persisted_provider_spec(
    existing: Option<&crate::config::ProviderConfigV2>,
    provider: &crate::config::PersistedProviderSpec,
) -> crate::config::ProviderConfigV2 {
    let mut auth = existing
        .map(|provider| provider.auth.clone())
        .unwrap_or_default();
    auth.auth_token_env = provider.auth_token_env.clone();
    auth.api_key_env = provider.api_key_env.clone();

    crate::config::ProviderConfigV2 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth,
        tags: existing
            .map(|provider| provider.tags.clone())
            .unwrap_or_default(),
        supported_models: existing
            .map(|provider| provider.supported_models.clone())
            .unwrap_or_default(),
        model_mapping: existing
            .map(|provider| provider.model_mapping.clone())
            .unwrap_or_default(),
        endpoints: provider
            .endpoints
            .iter()
            .map(|endpoint| {
                let existing_endpoint =
                    existing.and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                (
                    endpoint.name.clone(),
                    crate::config::ProviderEndpointV2 {
                        base_url: endpoint.base_url.clone(),
                        enabled: endpoint.enabled,
                        priority: endpoint.priority,
                        tags: existing_endpoint
                            .map(|endpoint| endpoint.tags.clone())
                            .unwrap_or_default(),
                        supported_models: existing_endpoint
                            .map(|endpoint| endpoint.supported_models.clone())
                            .unwrap_or_default(),
                        model_mapping: existing_endpoint
                            .map(|endpoint| endpoint.model_mapping.clone())
                            .unwrap_or_default(),
                    },
                )
            })
            .collect(),
    }
}

fn validate_station_members_for_view(
    service_name: &str,
    station_name: &str,
    view: &crate::config::ServiceViewV2,
    members: &[crate::config::GroupMemberRefV2],
) -> Result<(), (StatusCode, String)> {
    for member in members {
        let provider = view
            .providers
            .get(member.provider.as_str())
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "[{service_name}] station '{}' references missing provider '{}'",
                        station_name, member.provider
                    ),
                )
            })?;

        for endpoint_name in &member.endpoint_names {
            if !provider.endpoints.contains_key(endpoint_name.as_str()) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "[{service_name}] station '{}' references missing endpoint '{}.{}'",
                        station_name, member.provider, endpoint_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn list_persisted_station_specs(
    proxy: ProxyService,
) -> Result<Json<crate::config::PersistedStationsCatalog>, (StatusCode, String)> {
    let cfg = load_persisted_config_v2().await?;
    Ok(Json(crate::config::build_persisted_station_catalog(
        service_view_v2(&cfg, proxy.service_name),
    )))
}

pub(super) async fn list_persisted_provider_specs(
    proxy: ProxyService,
) -> Result<Json<crate::config::PersistedProvidersCatalog>, (StatusCode, String)> {
    let cfg = load_persisted_config_v2().await?;
    Ok(Json(crate::config::build_persisted_provider_catalog(
        service_view_v2(&cfg, proxy.service_name),
    )))
}

pub(super) async fn upsert_persisted_profile(
    proxy: ProxyService,
    Path(profile_name): Path<String>,
    Json(payload): Json<PersistedProfileUpsertRequest>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = sanitize_profile_name(profile_name.as_str())?;
    let profile = sanitize_profile_request(payload);

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    mgr.profiles.insert(profile_name.clone(), profile);
    let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    crate::config::validate_profile_station_compatibility(
        proxy.service_name,
        mgr,
        profile_name.as_str(),
        &resolved,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(
        save_runtime_profiles_config_and_reload(&proxy, cfg).await?,
    ))
}

pub(super) async fn delete_persisted_profile(
    proxy: ProxyService,
    Path(profile_name): Path<String>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = sanitize_profile_name(profile_name.as_str())?;

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    let referencing_profiles = mgr
        .profiles
        .iter()
        .filter_map(|(name, profile)| {
            (profile.extends.as_deref() == Some(profile_name.as_str())).then_some(name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_profiles.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "profile '{}' is extended by profiles: {}",
                profile_name,
                referencing_profiles.join(", ")
            ),
        ));
    }

    if mgr.profiles.remove(profile_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("profile '{}' not found", profile_name),
        ));
    }
    if mgr.default_profile.as_deref() == Some(profile_name.as_str()) {
        mgr.default_profile = None;
    }

    save_runtime_profiles_config_and_reload(&proxy, cfg).await?;
    if proxy
        .state
        .get_runtime_default_profile_override(proxy.service_name)
        .await
        .as_deref()
        == Some(profile_name.as_str())
    {
        proxy
            .state
            .clear_runtime_default_profile_override(proxy.service_name)
            .await;
    }

    Ok(Json(make_profiles_response(&proxy).await))
}

pub(super) async fn update_persisted_station(
    proxy: ProxyService,
    Path(station_name): Path<String>,
    Json(payload): Json<PersistedStationUpdateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    if payload.enabled.is_none() && payload.level.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one persisted station field must be provided".to_string(),
        ));
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);
    let Some(station) = mgr.station_mut(station_name.as_str()) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    };
    if let Some(enabled) = payload.enabled {
        station.enabled = enabled;
    }
    if let Some(level) = payload.level {
        station.level = level.clamp(1, 10);
    }

    save_runtime_config_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn set_persisted_active_station(
    proxy: ProxyService,
    Json(payload): Json<PersistedStationActiveRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = payload.station_name();

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);
    if let Some(station_name) = station_name.as_deref()
        && !mgr.contains_station(station_name)
    {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    }
    mgr.active = station_name;

    save_runtime_config_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn upsert_persisted_station_spec(
    proxy: ProxyService,
    Path(station_name): Path<String>,
    Json(payload): Json<PersistedStationSpecUpsertRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    let mut station = sanitize_station_spec_request(payload)?;
    station.name = station_name.clone();

    let mut cfg = load_persisted_config_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);
    validate_station_members_for_view(
        proxy.service_name,
        station_name.as_str(),
        view,
        &station.members,
    )?;
    view.groups.insert(
        station_name.clone(),
        crate::config::GroupConfigV2 {
            alias: station.alias.clone(),
            enabled: station.enabled,
            level: station.level.clamp(1, 10),
            members: station.members.clone(),
        },
    );

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_proxy_config_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_persisted_station_spec(
    proxy: ProxyService,
    Path(station_name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    let mut cfg = load_persisted_config_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);

    let referencing_profiles = view
        .profiles
        .iter()
        .filter_map(|(profile_name, profile)| {
            (profile.station.as_deref() == Some(station_name.as_str()))
                .then_some(profile_name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_profiles.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "station '{}' is referenced by profiles: {}",
                station_name,
                referencing_profiles.join(", ")
            ),
        ));
    }

    if view.groups.remove(station_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    }
    if view.active_group.as_deref() == Some(station_name.as_str()) {
        view.active_group = None;
    }

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_proxy_config_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn upsert_persisted_provider_spec(
    proxy: ProxyService,
    Path(provider_name): Path<String>,
    Json(payload): Json<PersistedProviderSpecUpsertRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let provider_name = sanitize_provider_name(provider_name.as_str())?;
    let mut provider = sanitize_provider_spec_request(payload)?;
    provider.name = provider_name.clone();

    let mut cfg = load_persisted_config_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);
    let existing_provider = view.providers.get(provider_name.as_str()).cloned();
    view.providers.insert(
        provider_name,
        merge_persisted_provider_spec(existing_provider.as_ref(), &provider),
    );

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_proxy_config_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_persisted_provider_spec(
    proxy: ProxyService,
    Path(provider_name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let provider_name = sanitize_provider_name(provider_name.as_str())?;
    let mut cfg = load_persisted_config_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);

    let referencing_stations = view
        .groups
        .iter()
        .filter_map(|(station_name, station)| {
            station
                .members
                .iter()
                .any(|member| member.provider == provider_name)
                .then_some(station_name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_stations.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "provider '{}' is referenced by stations: {}",
                provider_name,
                referencing_stations.join(", ")
            ),
        ));
    }

    if view.providers.remove(provider_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("provider '{}' not found", provider_name),
        ));
    }

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_proxy_config_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn set_persisted_default_profile(
    proxy: ProxyService,
    Json(payload): Json<PersistedDefaultProfileRequest>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = payload
        .profile_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    if let Some(profile_name) = profile_name.as_deref() {
        if mgr.profile(profile_name).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }
        let resolved = crate::config::resolve_service_profile(mgr, profile_name)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        crate::config::validate_profile_station_compatibility(
            proxy.service_name,
            mgr,
            profile_name,
            &resolved,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    mgr.default_profile = profile_name;

    Ok(Json(
        save_runtime_profiles_config_and_reload(&proxy, cfg).await?,
    ))
}
