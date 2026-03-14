use super::*;

#[test]
fn local_profile_preview_catalogs_from_text_extracts_v2_station_provider_structure() {
    let text = r#"
version = 2

[codex]
active_station = "primary"

[codex.providers.right]
alias = "Right"
[codex.providers.right.auth]
auth_token_env = "RIGHT_API_KEY"
[codex.providers.right.endpoints.main]
base_url = "https://right.example.com/v1"

[codex.stations.primary]
alias = "Primary"
level = 3

[[codex.stations.primary.members]]
provider = "right"
preferred = true
"#;

    let (stations, providers) =
        local_profile_preview_catalogs_from_text(text, "codex").expect("catalog");

    let station = stations.get("primary").expect("primary station");
    assert_eq!(station.alias.as_deref(), Some("Primary"));
    assert_eq!(station.level, 3);
    assert_eq!(station.members.len(), 1);
    assert_eq!(station.members[0].provider, "right");

    let provider = providers.get("right").expect("right provider");
    assert_eq!(provider.alias.as_deref(), Some("Right"));
    assert_eq!(provider.endpoints.len(), 1);
    assert_eq!(provider.endpoints[0].name, "main");
}

#[test]
fn build_profile_route_preview_resolves_station_source_in_order() {
    let explicit = build_profile_route_preview(
        &crate::config::ServiceControlProfile {
            station: Some("beta".to_string()),
            ..Default::default()
        },
        Some("alpha"),
        Some("gamma"),
        None,
        None,
        None,
    );
    assert_eq!(
        explicit.station_source,
        ProfilePreviewStationSource::Profile
    );
    assert_eq!(explicit.resolved_station_name.as_deref(), Some("beta"));

    let configured = build_profile_route_preview(
        &crate::config::ServiceControlProfile::default(),
        Some("alpha"),
        Some("gamma"),
        None,
        None,
        None,
    );
    assert_eq!(
        configured.station_source,
        ProfilePreviewStationSource::ConfiguredActive
    );
    assert_eq!(configured.resolved_station_name.as_deref(), Some("alpha"));

    let auto = build_profile_route_preview(
        &crate::config::ServiceControlProfile::default(),
        None,
        Some("gamma"),
        None,
        None,
        None,
    );
    assert_eq!(auto.station_source, ProfilePreviewStationSource::Auto);
    assert_eq!(auto.resolved_station_name.as_deref(), Some("gamma"));
}

#[test]
fn build_profile_route_preview_collects_member_routes_and_capability_checks() {
    let station_specs = BTreeMap::from([(
        "primary".to_string(),
        PersistedStationSpec {
            name: "primary".to_string(),
            alias: Some("Primary".to_string()),
            enabled: true,
            level: 2,
            members: vec![GroupMemberRefV2 {
                provider: "right".to_string(),
                endpoint_names: Vec::new(),
                preferred: true,
            }],
        },
    )]);
    let provider_catalog = BTreeMap::from([(
        "right".to_string(),
        PersistedStationProviderRef {
            name: "right".to_string(),
            alias: Some("Right".to_string()),
            enabled: true,
            endpoints: vec![
                crate::config::PersistedStationProviderEndpointRef {
                    name: "hk".to_string(),
                    base_url: "https://hk.example.com/v1".to_string(),
                    enabled: true,
                },
                crate::config::PersistedStationProviderEndpointRef {
                    name: "us".to_string(),
                    base_url: "https://us.example.com/v1".to_string(),
                    enabled: true,
                },
            ],
        },
    )]);
    let runtime_catalog = BTreeMap::from([(
        "primary".to_string(),
        StationOption {
            name: "primary".to_string(),
            alias: Some("Primary".to_string()),
            enabled: true,
            level: 2,
            configured_enabled: true,
            configured_level: 2,
            runtime_enabled_override: None,
            runtime_level_override: None,
            runtime_state: RuntimeConfigState::Normal,
            runtime_state_override: None,
            capabilities: StationCapabilitySummary {
                model_catalog_kind: ModelCatalogKind::Declared,
                supported_models: vec!["gpt-5.4".to_string()],
                supports_service_tier: CapabilitySupport::Supported,
                supports_reasoning_effort: CapabilitySupport::Unsupported,
            },
        },
    )]);
    let preview = build_profile_route_preview(
        &crate::config::ServiceControlProfile {
            extends: None,
            station: Some("primary".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
        },
        None,
        None,
        Some(&station_specs),
        Some(&provider_catalog),
        Some(&runtime_catalog),
    );

    assert!(preview.station_exists);
    assert_eq!(preview.station_alias.as_deref(), Some("Primary"));
    assert_eq!(preview.members.len(), 1);
    assert!(preview.members[0].uses_all_endpoints);
    assert_eq!(
        preview.members[0].endpoint_names,
        vec!["hk".to_string(), "us".to_string()]
    );
    assert_eq!(preview.model_supported, Some(true));
    assert_eq!(preview.service_tier_supported, Some(true));
    assert_eq!(preview.reasoning_supported, Some(false));
}
