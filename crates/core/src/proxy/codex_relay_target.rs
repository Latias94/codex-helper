use axum::http::StatusCode;

use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamConfig};

use super::ProxyControlError;

#[derive(Debug, Clone, Copy)]
pub(super) struct CodexRelayTargetSelection<'a> {
    pub(super) station_name: Option<&'a str>,
    pub(super) upstream_index: Option<usize>,
    pub(super) provider_id: Option<&'a str>,
    pub(super) endpoint_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(super) struct SelectedCodexRelayTarget {
    pub(super) station_name: String,
    pub(super) upstream_index: usize,
    pub(super) upstream: UpstreamConfig,
    pub(super) provider_id: Option<String>,
    pub(super) endpoint_id: Option<String>,
    pub(super) provider_endpoint_key: Option<String>,
}

pub(super) fn select_codex_relay_target(
    mgr: &ServiceConfigManager,
    selection: CodexRelayTargetSelection<'_>,
) -> Result<SelectedCodexRelayTarget, ProxyControlError> {
    let provider_id = clean_selection_value(selection.provider_id);
    let endpoint_id = clean_selection_value(selection.endpoint_id);
    let station_name_selection = clean_selection_value(selection.station_name);

    if let Some(provider_id) = provider_id {
        if station_name_selection.is_some() || selection.upstream_index.is_some() {
            return Err(ProxyControlError::new(
                StatusCode::BAD_REQUEST,
                "provider targeting cannot be combined with station_name or upstream_index",
            ));
        }
        return select_provider_endpoint_target(mgr, provider_id, endpoint_id);
    }

    if endpoint_id.is_some() {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            "endpoint_id requires provider_id",
        ));
    }

    let station_name = selection
        .station_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| mgr.active.clone())
        .or_else(|| stable_first_station_name(mgr))
        .ok_or_else(|| {
            ProxyControlError::new(StatusCode::BAD_REQUEST, "no codex station is configured")
        })?;
    let station = mgr.station(&station_name).ok_or_else(|| {
        ProxyControlError::new(
            StatusCode::NOT_FOUND,
            format!("station '{station_name}' not found"),
        )
    })?;
    let upstream_index = selection.upstream_index.unwrap_or(0);
    let upstream = station
        .upstreams
        .get(upstream_index)
        .cloned()
        .ok_or_else(|| {
            let (status, message) = upstream_not_found(&station_name, station, upstream_index);
            ProxyControlError::new(status, message)
        })?;
    let provider_id = target_provider_id(&upstream);
    let endpoint_id = target_endpoint_id(&upstream, upstream_index);
    let provider_endpoint_key =
        provider_endpoint_key_for_parts(provider_id.as_deref(), endpoint_id.as_deref());
    Ok(SelectedCodexRelayTarget {
        station_name,
        upstream_index,
        upstream,
        provider_id,
        endpoint_id,
        provider_endpoint_key,
    })
}

fn select_provider_endpoint_target(
    mgr: &ServiceConfigManager,
    provider_id: &str,
    endpoint_id: Option<&str>,
) -> Result<SelectedCodexRelayTarget, ProxyControlError> {
    let mut matches = Vec::new();
    for station_name in ordered_station_names(mgr) {
        let Some(station) = mgr.station(station_name.as_str()) else {
            continue;
        };
        for (upstream_index, upstream) in station.upstreams.iter().enumerate() {
            if target_provider_id(upstream).as_deref() != Some(provider_id) {
                continue;
            }
            let upstream_endpoint_id = target_endpoint_id(upstream, upstream_index);
            if endpoint_id
                .is_some_and(|endpoint_id| upstream_endpoint_id.as_deref() != Some(endpoint_id))
            {
                continue;
            }
            matches.push(SelectedCodexRelayTarget {
                station_name: station_name.clone(),
                upstream_index,
                upstream: upstream.clone(),
                provider_id: Some(provider_id.to_string()),
                endpoint_id: upstream_endpoint_id.clone(),
                provider_endpoint_key: provider_endpoint_key_for_parts(
                    Some(provider_id),
                    upstream_endpoint_id.as_deref(),
                ),
            });
        }
    }

    if matches.is_empty() {
        let message = if let Some(endpoint_id) = endpoint_id {
            format!("provider '{provider_id}' endpoint '{endpoint_id}' not found")
        } else {
            format!("provider '{provider_id}' not found")
        };
        return Err(ProxyControlError::new(StatusCode::NOT_FOUND, message));
    }

    if endpoint_id.is_none()
        && let Some(default_index) = matches
            .iter()
            .position(|target| target.endpoint_id.as_deref() == Some("default"))
    {
        return Ok(matches.remove(default_index));
    }

    Ok(matches.remove(0))
}

fn ordered_station_names(mgr: &ServiceConfigManager) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(active) = mgr
        .active
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|name| mgr.contains_station(name))
    {
        names.push(active.to_string());
    }

    let mut rest = mgr
        .stations()
        .keys()
        .filter(|name| !names.iter().any(|existing| existing == *name))
        .cloned()
        .collect::<Vec<_>>();
    rest.sort();
    names.extend(rest);
    names
}

fn clean_selection_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn target_provider_id(upstream: &UpstreamConfig) -> Option<String> {
    upstream
        .tags
        .get("provider_id")
        .map(String::as_str)
        .and_then(|value| clean_selection_value(Some(value)))
        .map(ToOwned::to_owned)
}

fn target_endpoint_id(upstream: &UpstreamConfig, upstream_index: usize) -> Option<String> {
    target_provider_id(upstream)?;
    Some(
        upstream
            .tags
            .get("endpoint_id")
            .map(String::as_str)
            .and_then(|value| clean_selection_value(Some(value)))
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| upstream_index.to_string()),
    )
}

fn provider_endpoint_key_for_parts(
    provider_id: Option<&str>,
    endpoint_id: Option<&str>,
) -> Option<String> {
    Some(format!(
        "codex/{}/{}",
        provider_id?,
        endpoint_id.unwrap_or("default")
    ))
}

fn stable_first_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.stations().keys().min().cloned()
}

fn upstream_not_found(
    station_name: &str,
    station: &ServiceConfig,
    upstream_index: usize,
) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        format!(
            "upstream index {upstream_index} not found for station '{}' ({} upstreams configured)",
            station_name,
            station.upstreams.len()
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn upstream(
        base_url: &str,
        provider_id: Option<&str>,
        endpoint_id: Option<&str>,
    ) -> UpstreamConfig {
        let mut tags = HashMap::new();
        if let Some(provider_id) = provider_id {
            tags.insert("provider_id".to_string(), provider_id.to_string());
        }
        if let Some(endpoint_id) = endpoint_id {
            tags.insert("endpoint_id".to_string(), endpoint_id.to_string());
        }
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: Default::default(),
            tags,
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn manager() -> ServiceConfigManager {
        let mut mgr = ServiceConfigManager {
            active: Some("routing".to_string()),
            ..Default::default()
        };
        mgr.configs.insert(
            "routing".to_string(),
            ServiceConfig {
                name: "routing".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![
                    upstream("https://input8.example/v1", Some("input8"), Some("default")),
                    upstream("https://ciii.example/v1", Some("ciii"), Some("default")),
                ],
            },
        );
        mgr
    }

    #[test]
    fn select_codex_relay_target_finds_route_graph_provider_id() {
        let selected = select_codex_relay_target(
            &manager(),
            CodexRelayTargetSelection {
                station_name: None,
                upstream_index: None,
                provider_id: Some("ciii"),
                endpoint_id: None,
            },
        )
        .expect("select provider");

        assert_eq!(selected.station_name, "routing");
        assert_eq!(selected.upstream_index, 1);
        assert_eq!(selected.upstream.base_url, "https://ciii.example/v1");
        assert_eq!(selected.provider_id.as_deref(), Some("ciii"));
        assert_eq!(selected.endpoint_id.as_deref(), Some("default"));
        assert_eq!(
            selected.provider_endpoint_key.as_deref(),
            Some("codex/ciii/default")
        );
    }

    #[test]
    fn select_codex_relay_target_rejects_endpoint_without_provider() {
        let error = select_codex_relay_target(
            &manager(),
            CodexRelayTargetSelection {
                station_name: None,
                upstream_index: None,
                provider_id: None,
                endpoint_id: Some("default"),
            },
        )
        .expect_err("endpoint without provider should fail");

        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert!(error.message().contains("endpoint_id requires provider_id"));
    }
}
