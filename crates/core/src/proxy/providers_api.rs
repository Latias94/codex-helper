use std::collections::BTreeSet;

use axum::Json;
use axum::http::StatusCode;

use crate::dashboard_core::{ProviderOption, build_provider_options_from_view};
use crate::logging::{log_retry_trace, now_ms};
use crate::state::RuntimeConfigState;

use super::ProxyService;
use super::control_plane_service::{load_persisted_config_v2, service_view_v2};

#[derive(serde::Deserialize)]
pub(super) struct ProviderRuntimeMetaRequest {
    provider_name: String,
    #[serde(default)]
    endpoint_name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    clear_enabled: bool,
    #[serde(default)]
    runtime_state: Option<RuntimeConfigState>,
    #[serde(default)]
    clear_runtime_state: bool,
}

fn normalize_provider_name(value: &str) -> Result<String, (StatusCode, String)> {
    let value = value.trim();
    if value.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provider_name is required".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalize_optional_endpoint_name(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_target_base_urls(
    view: &crate::config::ServiceViewV2,
    provider_name: &str,
    endpoint_name: Option<&str>,
) -> Result<Vec<String>, (StatusCode, String)> {
    let provider = view.providers.get(provider_name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("provider '{}' not found", provider_name),
        )
    })?;

    let mut urls = BTreeSet::new();
    if let Some(endpoint_name) = endpoint_name {
        let endpoint = provider.endpoints.get(endpoint_name).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!(
                    "provider endpoint '{}.{}' not found",
                    provider_name, endpoint_name
                ),
            )
        })?;
        urls.insert(endpoint.base_url.clone());
    } else {
        for endpoint in provider.endpoints.values() {
            urls.insert(endpoint.base_url.clone());
        }
    }

    if urls.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("provider '{}' has no endpoints", provider_name),
        ));
    }
    Ok(urls.into_iter().collect())
}

pub(super) async fn list_providers(
    proxy: ProxyService,
) -> Result<Json<Vec<ProviderOption>>, (StatusCode, String)> {
    let cfg = load_persisted_config_v2().await?;
    let upstream_overrides = proxy
        .state
        .get_upstream_meta_overrides(proxy.service_name)
        .await;
    Ok(Json(build_provider_options_from_view(
        service_view_v2(&cfg, proxy.service_name),
        &upstream_overrides,
    )))
}

pub(super) async fn apply_provider_runtime_meta(
    proxy: ProxyService,
    Json(payload): Json<ProviderRuntimeMetaRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if payload.enabled.is_none()
        && payload.runtime_state.is_none()
        && !payload.clear_enabled
        && !payload.clear_runtime_state
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one provider runtime action must be provided".to_string(),
        ));
    }

    let provider_name = normalize_provider_name(payload.provider_name.as_str())?;
    let endpoint_name = normalize_optional_endpoint_name(payload.endpoint_name);
    let cfg = load_persisted_config_v2().await?;
    let base_urls = resolve_target_base_urls(
        service_view_v2(&cfg, proxy.service_name),
        provider_name.as_str(),
        endpoint_name.as_deref(),
    )?;
    let runtime_state = payload.runtime_state;
    let applied_base_urls = base_urls.clone();

    let now = now_ms();
    for base_url in base_urls {
        if payload.clear_enabled {
            proxy
                .state
                .clear_upstream_enabled_override(proxy.service_name, base_url.as_str())
                .await;
        } else if let Some(enabled) = payload.enabled {
            proxy
                .state
                .set_upstream_enabled_override(proxy.service_name, base_url.clone(), enabled, now)
                .await;
        }

        if payload.clear_runtime_state {
            proxy
                .state
                .clear_upstream_runtime_state_override(proxy.service_name, base_url.as_str())
                .await;
        } else if let Some(runtime_state) = payload.runtime_state {
            proxy
                .state
                .set_upstream_runtime_state_override(
                    proxy.service_name,
                    base_url.clone(),
                    runtime_state,
                    now,
                )
                .await;
        }
    }

    log_retry_trace(serde_json::json!({
        "event": "provider_runtime_override",
        "service": proxy.service_name,
        "provider_name": provider_name,
        "endpoint_name": endpoint_name,
        "base_urls": applied_base_urls,
        "enabled": payload.enabled,
        "clear_enabled": payload.clear_enabled,
        "runtime_state": runtime_state.map(|state| format!("{state:?}").to_ascii_lowercase()),
        "clear_runtime_state": payload.clear_runtime_state,
    }));

    Ok(StatusCode::NO_CONTENT)
}
