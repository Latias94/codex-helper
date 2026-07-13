use std::collections::HashMap;

use axum::http::StatusCode;

use crate::config::UpstreamConfig;
use crate::routing_ir::{CompiledRouteGraph, RouteCandidate};
use crate::runtime_identity::ProviderEndpointKey;

use super::ProxyControlError;

#[derive(Debug, Clone, Copy)]
pub(super) struct CodexRelayTargetSelection<'a> {
    pub(super) provider_id: Option<&'a str>,
    pub(super) endpoint_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(super) struct SelectedCodexRelayTarget {
    pub(super) upstream: UpstreamConfig,
    pub(super) provider_endpoint: ProviderEndpointKey,
}

pub(super) fn select_codex_relay_target(
    graph: &CompiledRouteGraph,
    selection: CodexRelayTargetSelection<'_>,
) -> Result<SelectedCodexRelayTarget, ProxyControlError> {
    let provider_id = clean_selection_value(selection.provider_id);
    let endpoint_id = clean_selection_value(selection.endpoint_id);
    if endpoint_id.is_some() && provider_id.is_none() {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            "endpoint_id requires provider_id",
        ));
    }

    let mut matches = graph
        .candidates()
        .iter()
        .filter(|candidate| {
            provider_id.is_none_or(|provider_id| candidate.provider_id == provider_id)
                && endpoint_id.is_none_or(|endpoint_id| candidate.endpoint_id == endpoint_id)
        })
        .collect::<Vec<_>>();

    if matches.is_empty() {
        let message = match (provider_id, endpoint_id) {
            (Some(provider_id), Some(endpoint_id)) => {
                format!("provider '{provider_id}' endpoint '{endpoint_id}' not found")
            }
            (Some(provider_id), None) => format!("provider '{provider_id}' not found"),
            (None, None) => "no codex provider endpoint is configured".to_string(),
            (None, Some(_)) => unreachable!("endpoint without provider is rejected above"),
        };
        return Err(ProxyControlError::new(StatusCode::NOT_FOUND, message));
    }

    if endpoint_id.is_none()
        && let Some(default_index) = matches
            .iter()
            .position(|candidate| candidate.endpoint_id == "default")
    {
        return Ok(selected_target(
            graph.service_name(),
            matches.remove(default_index),
        ));
    }

    Ok(selected_target(graph.service_name(), matches.remove(0)))
}

fn selected_target(service_name: &str, candidate: &RouteCandidate) -> SelectedCodexRelayTarget {
    let provider_endpoint = ProviderEndpointKey::new(
        service_name,
        candidate.provider_id.clone(),
        candidate.endpoint_id.clone(),
    );
    let mut tags = candidate
        .tags
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    tags.insert("provider_id".to_string(), candidate.provider_id.clone());
    tags.insert("endpoint_id".to_string(), candidate.endpoint_id.clone());
    tags.insert(
        "provider_endpoint_key".to_string(),
        provider_endpoint.stable_key(),
    );

    SelectedCodexRelayTarget {
        upstream: UpstreamConfig {
            base_url: candidate.base_url.clone(),
            auth: candidate.auth.clone(),
            tags,
            supported_models: candidate
                .supported_models
                .iter()
                .map(|(model, supported)| (model.clone(), *supported))
                .collect(),
            model_mapping: candidate
                .model_mapping
                .iter()
                .map(|(source, target)| (source.clone(), target.clone()))
                .collect(),
        },
        provider_endpoint,
    }
}

fn clean_selection_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{ProviderConfig, RouteGraphConfig, ServiceRouteConfig};

    fn graph() -> CompiledRouteGraph {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input8".to_string(),
                    ProviderConfig {
                        base_url: Some("https://input8.example/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "ciii".to_string(),
                    ProviderConfig {
                        base_url: Some("https://ciii.example/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input8".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        CompiledRouteGraph::compile("codex", &view).expect("compile graph")
    }

    #[test]
    fn select_codex_relay_target_finds_provider_endpoint() {
        let selected = select_codex_relay_target(
            &graph(),
            CodexRelayTargetSelection {
                provider_id: Some("ciii"),
                endpoint_id: None,
            },
        )
        .expect("select provider");

        assert_eq!(selected.upstream.base_url, "https://ciii.example/v1");
        assert_eq!(selected.provider_endpoint.provider_id, "ciii");
        assert_eq!(selected.provider_endpoint.endpoint_id, "default");
        assert_eq!(
            selected.provider_endpoint.stable_key(),
            "codex/ciii/default"
        );
    }

    #[test]
    fn select_codex_relay_target_rejects_endpoint_without_provider() {
        let error = select_codex_relay_target(
            &graph(),
            CodexRelayTargetSelection {
                provider_id: None,
                endpoint_id: Some("default"),
            },
        )
        .expect_err("endpoint without provider should fail");

        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert!(error.message().contains("endpoint_id requires provider_id"));
    }
}
