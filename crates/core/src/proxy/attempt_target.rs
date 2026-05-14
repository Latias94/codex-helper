use crate::config::UpstreamConfig;
use crate::lb::SelectedUpstream;
use crate::routing_ir::RouteCandidate;
use crate::runtime_identity::{LegacyUpstreamKey, ProviderEndpointKey};

use super::route_metadata::{
    ENDPOINT_ID_TAG, PREFERENCE_GROUP_TAG, PROVIDER_ENDPOINT_KEY_TAG, PROVIDER_ID_TAG,
    ROUTE_PATH_TAG,
};

#[derive(Debug, Clone)]
pub(super) struct ProviderEndpointAttemptTarget {
    pub(super) provider_endpoint: ProviderEndpointKey,
    pub(super) compatibility: Option<LegacyUpstreamKey>,
    pub(super) upstream: UpstreamConfig,
    pub(super) route_path: Vec<String>,
    pub(super) preference_group: u32,
    pub(super) stable_candidate_index: usize,
}

#[derive(Debug, Clone)]
pub(super) enum AttemptTarget {
    Legacy(SelectedUpstream),
    ProviderEndpoint(ProviderEndpointAttemptTarget),
}

impl AttemptTarget {
    pub(super) fn legacy(selected: SelectedUpstream) -> Self {
        Self::Legacy(selected)
    }

    pub(super) fn from_candidate(service_name: &str, candidate: &RouteCandidate) -> Self {
        let provider_endpoint = ProviderEndpointKey::new(
            service_name,
            candidate.provider_id.clone(),
            candidate.endpoint_id.clone(),
        );
        let mut upstream = candidate.to_upstream_config();
        upstream
            .tags
            .insert(PROVIDER_ID_TAG.to_string(), candidate.provider_id.clone());
        upstream
            .tags
            .insert(ENDPOINT_ID_TAG.to_string(), candidate.endpoint_id.clone());
        upstream.tags.insert(
            PROVIDER_ENDPOINT_KEY_TAG.to_string(),
            provider_endpoint.stable_key(),
        );
        upstream.tags.insert(
            PREFERENCE_GROUP_TAG.to_string(),
            candidate.preference_group.to_string(),
        );
        if let Ok(route_path) = serde_json::to_string(&candidate.route_path) {
            upstream.tags.insert(ROUTE_PATH_TAG.to_string(), route_path);
        }

        let compatibility = candidate
            .compatibility_station_name
            .as_ref()
            .and_then(|station| {
                candidate
                    .compatibility_upstream_index
                    .map(|upstream_index| {
                        LegacyUpstreamKey::new(
                            service_name.to_string(),
                            station.clone(),
                            upstream_index,
                        )
                    })
            });

        Self::ProviderEndpoint(ProviderEndpointAttemptTarget {
            provider_endpoint: provider_endpoint.clone(),
            compatibility,
            upstream,
            route_path: candidate.route_path.clone(),
            preference_group: candidate.preference_group,
            stable_candidate_index: candidate.stable_index,
        })
    }

    pub(super) fn upstream(&self) -> &UpstreamConfig {
        match self {
            Self::Legacy(selected) => &selected.upstream,
            Self::ProviderEndpoint(target) => &target.upstream,
        }
    }

    pub(super) fn compatibility_station_name(&self) -> Option<&str> {
        match self {
            Self::Legacy(selected) => Some(selected.station_name.as_str()),
            Self::ProviderEndpoint(target) => target
                .compatibility
                .as_ref()
                .map(|legacy| legacy.station_name.as_str()),
        }
    }

    pub(super) fn compatibility_upstream_index(&self) -> Option<usize> {
        match self {
            Self::Legacy(selected) => Some(selected.index),
            Self::ProviderEndpoint(target) => target
                .compatibility
                .as_ref()
                .map(|legacy| legacy.upstream_index),
        }
    }

    pub(super) fn log_target_label(&self) -> String {
        match self {
            Self::Legacy(selected) => selected.station_name.clone(),
            Self::ProviderEndpoint(target) => target.provider_endpoint.stable_key(),
        }
    }

    pub(super) fn attempt_avoid_index(&self) -> usize {
        match self {
            Self::Legacy(selected) => selected.index,
            Self::ProviderEndpoint(target) => target.stable_candidate_index,
        }
    }

    pub(super) fn uses_provider_endpoint_attempt_index(&self) -> bool {
        matches!(self, Self::ProviderEndpoint(_))
    }

    pub(super) fn route_attempt_identity(&self) -> String {
        match self {
            Self::Legacy(selected) => format!(
                "station={} upstream_index={} url={}",
                selected.station_name, selected.index, selected.upstream.base_url
            ),
            Self::ProviderEndpoint(target) => {
                let mut identity = format!(
                    "endpoint={} group={}",
                    target.provider_endpoint.stable_key(),
                    target.preference_group
                );
                if let Some(compatibility) = target.compatibility.as_ref() {
                    identity.push_str(&format!(
                        " compat_station={} upstream_index={}",
                        compatibility.station_name, compatibility.upstream_index
                    ));
                }
                identity.push_str(&format!(" url={}", target.upstream.base_url));
                identity
            }
        }
    }

    pub(super) fn provider_id(&self) -> Option<&str> {
        match self {
            Self::Legacy(selected) => selected
                .upstream
                .tags
                .get(PROVIDER_ID_TAG)
                .map(String::as_str),
            Self::ProviderEndpoint(target) => Some(target.provider_endpoint.provider_id.as_str()),
        }
    }

    pub(super) fn endpoint_id(&self) -> Option<String> {
        match self {
            Self::Legacy(selected) => selected
                .upstream
                .tags
                .get(ENDPOINT_ID_TAG)
                .cloned()
                .or_else(|| Some(selected.index.to_string())),
            Self::ProviderEndpoint(target) => Some(target.provider_endpoint.endpoint_id.clone()),
        }
    }

    pub(super) fn provider_endpoint_ref(&self) -> Option<&ProviderEndpointKey> {
        match self {
            Self::Legacy(_) => None,
            Self::ProviderEndpoint(target) => Some(&target.provider_endpoint),
        }
    }

    pub(super) fn provider_endpoint_key(&self) -> Option<String> {
        match self {
            Self::Legacy(selected) => selected
                .upstream
                .tags
                .get(PROVIDER_ENDPOINT_KEY_TAG)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty() && *value != "-")
                .map(ToOwned::to_owned),
            Self::ProviderEndpoint(target) => Some(target.provider_endpoint.stable_key()),
        }
    }

    pub(super) fn preference_group(&self) -> Option<u32> {
        match self {
            Self::Legacy(selected) => selected
                .upstream
                .tags
                .get(PREFERENCE_GROUP_TAG)
                .and_then(|value| value.parse::<u32>().ok()),
            Self::ProviderEndpoint(target) => Some(target.preference_group),
        }
    }

    pub(super) fn route_path(&self) -> Vec<String> {
        match self {
            Self::Legacy(selected) => selected
                .upstream
                .tags
                .get(ROUTE_PATH_TAG)
                .and_then(|value| super::route_metadata::parse_route_path_tag(value))
                .unwrap_or_else(|| {
                    vec![
                        "legacy".to_string(),
                        selected.station_name.clone(),
                        selected
                            .upstream
                            .tags
                            .get(PROVIDER_ID_TAG)
                            .cloned()
                            .unwrap_or_else(|| {
                                format!("{}#{}", selected.station_name, selected.index)
                            }),
                    ]
                }),
            Self::ProviderEndpoint(target) => target.route_path.clone(),
        }
    }
}
