use std::collections::BTreeMap;

use super::proxy_settings_document::parse_proxy_settings_document;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfilePreviewStationSource {
    Profile,
    ConfiguredActive,
    Auto,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfilePreviewMemberRoute {
    pub(super) provider_name: String,
    pub(super) provider_alias: Option<String>,
    pub(super) provider_enabled: Option<bool>,
    pub(super) provider_missing: bool,
    pub(super) endpoint_names: Vec<String>,
    pub(super) uses_all_endpoints: bool,
    pub(super) preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileRoutePreview {
    pub(super) station_source: ProfilePreviewStationSource,
    pub(super) resolved_station_name: Option<String>,
    pub(super) station_exists: bool,
    pub(super) structure_available: bool,
    pub(super) station_alias: Option<String>,
    pub(super) station_enabled: Option<bool>,
    pub(super) station_level: Option<u8>,
    pub(super) members: Vec<ProfilePreviewMemberRoute>,
    pub(super) capabilities: Option<StationCapabilitySummary>,
    pub(super) model_supported: Option<bool>,
    pub(super) service_tier_supported: Option<bool>,
    pub(super) reasoning_supported: Option<bool>,
}

pub(super) fn build_profile_route_preview(
    profile: &crate::config::ServiceControlProfile,
    configured_active_station: Option<&str>,
    auto_station: Option<&str>,
    station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    runtime_station_catalog: Option<&BTreeMap<String, StationOption>>,
) -> ProfileRoutePreview {
    let explicit_station = non_empty_trimmed(profile.station.as_deref());
    let configured_active_station = non_empty_trimmed(configured_active_station);
    let auto_station = non_empty_trimmed(auto_station);

    let (station_source, resolved_station_name) = if let Some(name) = explicit_station {
        (ProfilePreviewStationSource::Profile, Some(name))
    } else if let Some(name) = configured_active_station {
        (ProfilePreviewStationSource::ConfiguredActive, Some(name))
    } else if let Some(name) = auto_station {
        (ProfilePreviewStationSource::Auto, Some(name))
    } else {
        (ProfilePreviewStationSource::Unresolved, None)
    };

    let station_spec = resolved_station_name
        .as_deref()
        .and_then(|name| station_specs.and_then(|specs| specs.get(name)));
    let runtime_station = resolved_station_name
        .as_deref()
        .and_then(|name| runtime_station_catalog.and_then(|catalog| catalog.get(name)));
    let capabilities = runtime_station.map(|station| station.capabilities.clone());

    let members = station_spec
        .map(|station| {
            station
                .members
                .iter()
                .map(|member| {
                    let provider =
                        provider_catalog.and_then(|providers| providers.get(&member.provider));
                    let endpoint_names = if member.endpoint_names.is_empty() {
                        provider
                            .map(|provider| {
                                provider
                                    .endpoints
                                    .iter()
                                    .map(|endpoint| endpoint.name.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        member.endpoint_names.clone()
                    };
                    ProfilePreviewMemberRoute {
                        provider_name: member.provider.clone(),
                        provider_alias: provider.and_then(|provider| provider.alias.clone()),
                        provider_enabled: provider.map(|provider| provider.enabled),
                        provider_missing: provider.is_none(),
                        endpoint_names,
                        uses_all_endpoints: member.endpoint_names.is_empty(),
                        preferred: member.preferred,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let model_supported = profile
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .and_then(|model| {
            capabilities.as_ref().and_then(|capabilities| {
                if capabilities.supported_models.is_empty() {
                    None
                } else {
                    Some(
                        capabilities
                            .supported_models
                            .iter()
                            .any(|item| item == model),
                    )
                }
            })
        });
    let service_tier_supported = profile
        .service_tier
        .as_deref()
        .filter(|tier| !tier.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_service_tier)
            })
        });
    let reasoning_supported = profile
        .reasoning_effort
        .as_deref()
        .filter(|effort| !effort.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_reasoning_effort)
            })
        });

    ProfileRoutePreview {
        station_source,
        station_exists: station_spec.is_some() || runtime_station.is_some(),
        structure_available: station_spec.is_some(),
        resolved_station_name,
        station_alias: station_spec
            .and_then(|station| station.alias.clone())
            .or_else(|| runtime_station.and_then(|station| station.alias.clone())),
        station_enabled: station_spec
            .map(|station| station.enabled)
            .or_else(|| runtime_station.map(|station| station.enabled)),
        station_level: station_spec
            .map(|station| station.level)
            .or_else(|| runtime_station.map(|station| station.level)),
        members,
        capabilities,
        model_supported,
        service_tier_supported,
        reasoning_supported,
    }
}

pub(super) fn local_profile_preview_catalogs_from_text(
    text: &str,
    service_name: &str,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    let _ = service_name;
    parse_proxy_settings_document(text).ok()?;
    None
}

pub(super) fn capability_support_truthy(support: CapabilitySupport) -> Option<bool> {
    match support {
        CapabilitySupport::Supported => Some(true),
        CapabilitySupport::Unsupported => Some(false),
        CapabilitySupport::Unknown => None,
    }
}
