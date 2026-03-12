use std::collections::{BTreeSet, HashMap};

use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamConfig};
use crate::state::RuntimeConfigState;

use super::types::{
    CapabilitySupport, ConfigCapabilitySummary, ConfigOption, ControlProfileOption,
    ModelCatalogKind,
};

pub fn build_config_options_from_mgr(
    mgr: &ServiceConfigManager,
    meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    state_overrides: &HashMap<String, RuntimeConfigState>,
) -> Vec<ConfigOption> {
    let mut configs = mgr
        .configs
        .iter()
        .map(|(name, c)| {
            let (enabled_override, level_override) = meta_overrides
                .get(name.as_str())
                .copied()
                .unwrap_or((None, None));
            let configured_level = c.level.clamp(1, 10);
            ConfigOption {
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
                capabilities: build_config_capability_summary(c),
            }
        })
        .collect::<Vec<_>>();
    configs.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
    configs
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
            station: profile.station.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            is_default: default_name == Some(name.as_str()),
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

fn build_config_capability_summary(config: &ServiceConfig) -> ConfigCapabilitySummary {
    let mut supported_models = BTreeSet::new();
    let mut has_declared_models = false;
    for upstream in &config.upstreams {
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

    ConfigCapabilitySummary {
        model_catalog_kind: if has_declared_models {
            ModelCatalogKind::Declared
        } else {
            ModelCatalogKind::ImplicitAny
        },
        supported_models: supported_models.into_iter().collect(),
        supports_service_tier: aggregate_capability_support(
            &config.upstreams,
            &[
                "supports_service_tier",
                "supports_service_tiers",
                "supports_fast_mode",
                "supports_fast",
            ],
        ),
        supports_reasoning_effort: aggregate_capability_support(
            &config.upstreams,
            &["supports_reasoning_effort", "supports_reasoning"],
        ),
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
    fn build_config_options_from_mgr_summarizes_station_capabilities() {
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

        let configs = build_config_options_from_mgr(&mgr, &HashMap::new(), &HashMap::new());

        let declared = configs
            .iter()
            .find(|cfg| cfg.name == "declared")
            .expect("declared config");
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

        let implicit = configs
            .iter()
            .find(|cfg| cfg.name == "implicit")
            .expect("implicit config");
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

        let blocked = configs
            .iter()
            .find(|cfg| cfg.name == "blocked")
            .expect("blocked config");
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
                station: Some("z".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("low".to_string()),
                service_tier: None,
            },
        );
        mgr.profiles.insert(
            "a-deep".to_string(),
            crate::config::ServiceControlProfile {
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
        assert_eq!(profiles[1].name, "z-fast");
        assert!(profiles[1].is_default);
        assert_eq!(profiles[1].reasoning_effort.as_deref(), Some("low"));
    }
}
