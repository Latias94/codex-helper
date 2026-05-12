use std::collections::{BTreeSet, HashMap};

use crate::config::{
    ProviderConfigV2, ServiceConfig, ServiceConfigManager, ServiceViewV2, UpstreamConfig,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::RuntimeConfigState;

use super::types::{
    CapabilitySupport, ControlProfileOption, ModelCatalogKind, ProviderEndpointOption,
    ProviderOption, StationCapabilitySummary, StationOption,
};

pub fn build_station_options_from_mgr(
    mgr: &ServiceConfigManager,
    meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    state_overrides: &HashMap<String, RuntimeConfigState>,
) -> Vec<StationOption> {
    let mut stations = mgr
        .stations()
        .iter()
        .map(|(name, c)| {
            let (enabled_override, level_override) = meta_overrides
                .get(name.as_str())
                .copied()
                .unwrap_or((None, None));
            let configured_level = c.level.clamp(1, 10);
            StationOption {
                name: name.clone(),
                alias: c.alias.clone(),
                enabled: enabled_override.unwrap_or(c.enabled),
                level: level_override.unwrap_or(configured_level).clamp(1, 10),
                configured_enabled: c.enabled,
                configured_level,
                runtime_enabled_override: enabled_override,
                runtime_level_override: level_override.map(|level| level.clamp(1, 10)),
                runtime_state: state_overrides
                    .get(name.as_str())
                    .copied()
                    .unwrap_or_default(),
                runtime_state_override: state_overrides.get(name.as_str()).copied(),
                capabilities: build_station_capability_summary(c),
            }
        })
        .collect::<Vec<_>>();
    stations.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
    stations
}

pub fn build_profile_options_from_mgr(
    mgr: &ServiceConfigManager,
    default_name: Option<&str>,
) -> Vec<ControlProfileOption> {
    let mut profiles = mgr
        .profiles
        .iter()
        .map(|(name, profile)| ControlProfileOption {
            name: name.clone(),
            extends: profile.extends.clone(),
            station: profile.station.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            fast_mode: profile.service_tier.as_deref() == Some("priority"),
            is_default: default_name == Some(name.as_str()),
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

pub fn build_model_options_from_mgr(mgr: &ServiceConfigManager) -> Vec<String> {
    let mut models = BTreeSet::new();

    for profile in mgr.profiles.values() {
        if let Some(model) = profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            models.insert(model.to_string());
        }
    }

    for station in mgr.stations().values() {
        models.extend(build_station_capability_summary(station).supported_models);
    }

    models.into_iter().collect()
}

pub fn build_provider_options_from_view(
    service_name: &str,
    view: &ServiceViewV2,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> Vec<ProviderOption> {
    let mut providers = view
        .providers
        .iter()
        .map(|(provider_name, provider)| {
            let mut endpoints = provider
                .endpoints
                .iter()
                .map(|(endpoint_name, endpoint)| {
                    build_provider_endpoint_option(
                        service_name,
                        provider_name,
                        provider,
                        endpoint_name,
                        endpoint,
                        upstream_overrides,
                    )
                })
                .collect::<Vec<_>>();
            endpoints.sort_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.base_url.cmp(&b.base_url))
            });

            ProviderOption {
                name: provider_name.clone(),
                alias: provider.alias.clone(),
                configured_enabled: provider.enabled,
                effective_enabled: provider.enabled
                    && endpoints.iter().any(|endpoint| endpoint.effective_enabled),
                routable_endpoints: endpoints
                    .iter()
                    .filter(|endpoint| endpoint.routable)
                    .count(),
                endpoints,
            }
        })
        .collect::<Vec<_>>();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers
}

fn build_station_capability_summary(station: &ServiceConfig) -> StationCapabilitySummary {
    let mut supported_models = BTreeSet::new();
    let mut has_declared_models = false;
    for upstream in &station.upstreams {
        if !upstream.supported_models.is_empty() || !upstream.model_mapping.is_empty() {
            has_declared_models = true;
        }
        for (model, supported) in &upstream.supported_models {
            if *supported {
                supported_models.insert(model.clone());
            }
        }
        for external_model in upstream.model_mapping.keys() {
            supported_models.insert(external_model.clone());
        }
    }

    StationCapabilitySummary {
        model_catalog_kind: if has_declared_models {
            ModelCatalogKind::Declared
        } else {
            ModelCatalogKind::ImplicitAny
        },
        supported_models: supported_models.into_iter().collect(),
        supports_service_tier: aggregate_capability_support(
            &station.upstreams,
            &[
                "supports_service_tier",
                "supports_service_tiers",
                "supports_fast_mode",
                "supports_fast",
            ],
        ),
        supports_reasoning_effort: aggregate_capability_support(
            &station.upstreams,
            &["supports_reasoning_effort", "supports_reasoning"],
        ),
    }
}

fn build_provider_endpoint_option(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV2,
    endpoint_name: &str,
    endpoint: &crate::config::ProviderEndpointV2,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> ProviderEndpointOption {
    let override_key =
        ProviderEndpointKey::new(service_name, provider_name, endpoint_name).stable_key();
    let (runtime_enabled_override, runtime_state_override) = upstream_overrides
        .get(override_key.as_str())
        .copied()
        .or_else(|| upstream_overrides.get(endpoint.base_url.as_str()).copied())
        .unwrap_or((None, None));
    let runtime_state = runtime_state_override.unwrap_or_default();
    let configured_enabled = provider.enabled && endpoint.enabled;
    let effective_enabled = configured_enabled && runtime_enabled_override.unwrap_or(true);
    let routable = effective_enabled && runtime_state == RuntimeConfigState::Normal;

    ProviderEndpointOption {
        provider_name: provider_name.to_string(),
        name: endpoint_name.to_string(),
        base_url: endpoint.base_url.clone(),
        priority: endpoint.priority,
        configured_enabled,
        effective_enabled,
        routable,
        runtime_enabled_override,
        runtime_state,
        runtime_state_override,
    }
}

fn aggregate_capability_support(
    upstreams: &[UpstreamConfig],
    tag_keys: &[&str],
) -> CapabilitySupport {
    let mut saw_supported = false;
    let mut saw_explicit_unsupported = false;
    let mut saw_unknown = false;

    for upstream in upstreams {
        match tag_keys
            .iter()
            .find_map(|key| upstream.tags.get(*key))
            .and_then(|value| parse_boolish_tag(value))
        {
            Some(true) => saw_supported = true,
            Some(false) => saw_explicit_unsupported = true,
            None => saw_unknown = true,
        }
    }

    if saw_supported {
        CapabilitySupport::Supported
    } else if saw_explicit_unsupported && !saw_unknown {
        CapabilitySupport::Unsupported
    } else {
        CapabilitySupport::Unknown
    }
}

fn parse_boolish_tag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" | "supported" => Some(true),
        "0" | "false" | "no" | "n" | "off" | "unsupported" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamAuth, UpstreamConfig};

    use super::*;

    #[test]
    fn build_station_options_from_mgr_summarizes_station_capabilities() {
        let mut mgr = ServiceConfigManager::default();
        mgr.configs.insert(
            "declared".to_string(),
            ServiceConfig {
                name: "declared".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://declared.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::from([
                        ("supports_service_tier".to_string(), "true".to_string()),
                        ("supports_reasoning_effort".to_string(), "true".to_string()),
                    ]),
                    supported_models: HashMap::from([
                        ("gpt-5".to_string(), true),
                        ("gpt-5.4".to_string(), true),
                        ("gpt-4.1".to_string(), false),
                    ]),
                    model_mapping: HashMap::from([(
                        "gpt-5-fast".to_string(),
                        "gpt-5.4-fast".to_string(),
                    )]),
                }],
            },
        );
        mgr.configs.insert(
            "implicit".to_string(),
            ServiceConfig {
                name: "implicit".to_string(),
                alias: None,
                enabled: true,
                level: 2,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://implicit.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            },
        );
        mgr.configs.insert(
            "blocked".to_string(),
            ServiceConfig {
                name: "blocked".to_string(),
                alias: None,
                enabled: true,
                level: 3,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://blocked.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::from([
                        ("supports_fast_mode".to_string(), "false".to_string()),
                        ("supports_reasoning".to_string(), "false".to_string()),
                    ]),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            },
        );

        let stations = build_station_options_from_mgr(&mgr, &HashMap::new(), &HashMap::new());

        let declared = stations
            .iter()
            .find(|cfg| cfg.name == "declared")
            .expect("declared station");
        assert_eq!(
            declared.capabilities.model_catalog_kind,
            ModelCatalogKind::Declared
        );
        assert_eq!(
            declared.capabilities.supported_models,
            vec![
                "gpt-5".to_string(),
                "gpt-5-fast".to_string(),
                "gpt-5.4".to_string(),
            ]
        );
        assert_eq!(
            declared.capabilities.supports_service_tier,
            CapabilitySupport::Supported
        );
        assert_eq!(
            declared.capabilities.supports_reasoning_effort,
            CapabilitySupport::Supported
        );

        let implicit = stations
            .iter()
            .find(|cfg| cfg.name == "implicit")
            .expect("implicit station");
        assert_eq!(
            implicit.capabilities.model_catalog_kind,
            ModelCatalogKind::ImplicitAny
        );
        assert!(implicit.capabilities.supported_models.is_empty());
        assert_eq!(
            implicit.capabilities.supports_service_tier,
            CapabilitySupport::Unknown
        );
        assert_eq!(
            implicit.capabilities.supports_reasoning_effort,
            CapabilitySupport::Unknown
        );

        let blocked = stations
            .iter()
            .find(|cfg| cfg.name == "blocked")
            .expect("blocked station");
        assert_eq!(
            blocked.capabilities.supports_service_tier,
            CapabilitySupport::Unsupported
        );
        assert_eq!(
            blocked.capabilities.supports_reasoning_effort,
            CapabilitySupport::Unsupported
        );
    }

    #[test]
    fn build_profile_options_from_mgr_marks_default_and_sorts() {
        let mut mgr = ServiceConfigManager::default();
        mgr.profiles.insert(
            "z-fast".to_string(),
            crate::config::ServiceControlProfile {
                extends: None,
                station: Some("z".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("low".to_string()),
                service_tier: None,
            },
        );
        mgr.profiles.insert(
            "a-deep".to_string(),
            crate::config::ServiceControlProfile {
                extends: Some("base".to_string()),
                station: Some("a".to_string()),
                model: Some("gpt-5".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("priority".to_string()),
            },
        );

        let profiles = build_profile_options_from_mgr(&mgr, Some("z-fast"));

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "a-deep");
        assert!(!profiles[0].is_default);
        assert!(profiles[0].fast_mode);
        assert_eq!(profiles[0].extends.as_deref(), Some("base"));
        assert_eq!(profiles[1].name, "z-fast");
        assert!(profiles[1].is_default);
        assert!(!profiles[1].fast_mode);
        assert_eq!(profiles[1].reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn build_model_options_from_mgr_merges_profiles_and_declared_models() {
        let mut mgr = ServiceConfigManager::default();
        mgr.profiles.insert(
            "z-fast".to_string(),
            crate::config::ServiceControlProfile {
                extends: None,
                station: Some("z".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("low".to_string()),
                service_tier: None,
            },
        );
        mgr.profiles.insert(
            "trimmed".to_string(),
            crate::config::ServiceControlProfile {
                extends: None,
                station: None,
                model: Some("  gpt-5.5-preview  ".to_string()),
                reasoning_effort: None,
                service_tier: None,
            },
        );
        mgr.configs.insert(
            "alpha".to_string(),
            crate::config::ServiceConfig {
                name: "alpha".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![crate::config::UpstreamConfig {
                    base_url: "https://alpha.example/v1".to_string(),
                    auth: crate::config::UpstreamAuth::default(),
                    tags: HashMap::from([("provider_id".to_string(), "alpha".to_string())]),
                    supported_models: HashMap::from([
                        ("gpt-5.4".to_string(), true),
                        ("gpt-5.4-mini".to_string(), true),
                    ]),
                    model_mapping: HashMap::from([(
                        "gpt-5.4-fast".to_string(),
                        "gpt-5.4-mini".to_string(),
                    )]),
                }],
            },
        );

        let models = build_model_options_from_mgr(&mgr);

        assert_eq!(
            models,
            vec![
                "gpt-5.4".to_string(),
                "gpt-5.4-fast".to_string(),
                "gpt-5.4-mini".to_string(),
                "gpt-5.5-preview".to_string(),
            ]
        );
    }

    #[test]
    fn build_provider_options_from_view_merges_runtime_endpoint_overrides() {
        let mut view = crate::config::ServiceViewV2::default();
        view.providers.insert(
            "alpha".to_string(),
            crate::config::ProviderConfigV2 {
                alias: Some("Alpha".to_string()),
                enabled: true,
                auth: UpstreamAuth::default(),
                tags: Default::default(),
                supported_models: Default::default(),
                model_mapping: Default::default(),
                endpoints: [
                    (
                        "default".to_string(),
                        crate::config::ProviderEndpointV2 {
                            base_url: "https://alpha.example/v1".to_string(),
                            enabled: true,
                            priority: 0,
                            tags: Default::default(),
                            supported_models: Default::default(),
                            model_mapping: Default::default(),
                        },
                    ),
                    (
                        "backup".to_string(),
                        crate::config::ProviderEndpointV2 {
                            base_url: "https://alpha-backup.example/v1".to_string(),
                            enabled: true,
                            priority: 1,
                            tags: Default::default(),
                            supported_models: Default::default(),
                            model_mapping: Default::default(),
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            },
        );

        let options = build_provider_options_from_view(
            "codex",
            &view,
            &HashMap::from([
                (
                    ProviderEndpointKey::new("codex", "alpha", "default").stable_key(),
                    (Some(true), Some(RuntimeConfigState::Normal)),
                ),
                (
                    "https://alpha.example/v1".to_string(),
                    (Some(false), Some(RuntimeConfigState::BreakerOpen)),
                ),
                (
                    "https://alpha-backup.example/v1".to_string(),
                    (None, Some(RuntimeConfigState::Draining)),
                ),
            ]),
        );

        assert_eq!(options.len(), 1);
        let provider = &options[0];
        assert_eq!(provider.name, "alpha");
        assert!(provider.configured_enabled);
        assert!(provider.effective_enabled);
        assert_eq!(provider.routable_endpoints, 1);
        assert_eq!(provider.endpoints.len(), 2);
        assert_eq!(provider.endpoints[0].name, "default");
        assert_eq!(provider.endpoints[0].runtime_enabled_override, Some(true));
        assert_eq!(
            provider.endpoints[0].runtime_state_override,
            Some(RuntimeConfigState::Normal)
        );
        assert!(provider.endpoints[0].effective_enabled);
        assert!(provider.endpoints[0].routable);
        assert_eq!(provider.endpoints[1].name, "backup");
        assert_eq!(
            provider.endpoints[1].runtime_state_override,
            Some(RuntimeConfigState::Draining)
        );
        assert!(provider.endpoints[1].effective_enabled);
        assert!(!provider.endpoints[1].routable);
    }
}
