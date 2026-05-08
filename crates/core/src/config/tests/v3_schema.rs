use super::*;

#[test]
fn load_config_supports_v3_routing_first_schema() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        write_file(
            &toml_path,
            r#"
version = 3

[codex.providers.monthly]
base_url = "https://monthly.example.com/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.paygo]
base_url = "https://paygo.example.com/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo", region = "us" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["paygo", "monthly"]
on_exhausted = "continue"
"#,
        );

        let cfg = super::load_config().await.expect("load v3 config");
        assert_eq!(cfg.version, Some(3));
        assert_eq!(cfg.codex.active.as_deref(), Some("routing"));

        let routing = cfg
            .codex
            .station("routing")
            .expect("routing station should exist");
        assert_eq!(routing.upstreams.len(), 2);
        assert_eq!(
            routing.upstreams[0].base_url,
            "https://monthly.example.com/v1"
        );
        assert_eq!(
            routing.upstreams[0].auth.auth_token_env.as_deref(),
            Some("MONTHLY_API_KEY")
        );
        assert_eq!(
            routing.upstreams[1].base_url,
            "https://paygo.example.com/v1"
        );
    });
}

#[test]
fn v3_tag_preferred_stop_excludes_non_matching_fallbacks() {
    let v3 = ProxyConfigV3 {
        version: 3,
        codex: ServiceViewV3 {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfigV3 {
                        base_url: Some("https://monthly.example.com/v1".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("MONTHLY_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfigV3::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfigV3 {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfigV3::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV3 {
                policy: RoutingPolicyV3::TagPreferred,
                order: vec!["monthly".to_string(), "paygo".to_string()],
                target: None,
                prefer_tags: vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                on_exhausted: RoutingExhaustedActionV3::Stop,
            }),
        },
        claude: ServiceViewV3::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let runtime = compile_v3_to_runtime(&v3).expect("compile v3");
    let routing = runtime
        .codex
        .station("routing")
        .expect("routing station should exist");
    assert_eq!(routing.upstreams.len(), 1);
    assert_eq!(
        routing.upstreams[0].base_url,
        "https://monthly.example.com/v1"
    );
}

#[test]
fn migrate_v2_to_v3_emits_routing_order_and_inline_simple_providers() {
    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("primary".to_string()),
            default_profile: Some("daily".to_string()),
            profiles: BTreeMap::from([(
                "daily".to_string(),
                ServiceControlProfile {
                    station: Some("primary".to_string()),
                    reasoning_effort: Some("medium".to_string()),
                    ..ServiceControlProfile::default()
                },
            )]),
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfigV2 {
                        auth: UpstreamAuth {
                            auth_token_env: Some("PRIMARY_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://primary.example.com/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        )]),
                        ..ProviderConfigV2::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfigV2 {
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://backup.example.com/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        )]),
                        ..ProviderConfigV2::default()
                    },
                ),
            ]),
            groups: BTreeMap::from([(
                "primary".to_string(),
                GroupConfigV2 {
                    alias: None,
                    enabled: true,
                    level: 1,
                    members: vec![
                        GroupMemberRefV2 {
                            provider: "backup".to_string(),
                            endpoint_names: Vec::new(),
                            preferred: false,
                        },
                        GroupMemberRefV2 {
                            provider: "primary".to_string(),
                            endpoint_names: Vec::new(),
                            preferred: true,
                        },
                    ],
                },
            )]),
        },
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let migrated = migrate_v2_to_v3(&v2).expect("migrate v2 to v3");
    let codex = migrated.codex;
    assert_eq!(
        codex
            .providers
            .get("primary")
            .and_then(|provider| provider.base_url.as_deref()),
        Some("https://primary.example.com/v1")
    );
    assert_eq!(
        codex
            .providers
            .get("primary")
            .and_then(|provider| provider.inline_auth.auth_token_env.as_deref()),
        Some("PRIMARY_API_KEY")
    );
    let routing = codex.routing.expect("routing should be emitted");
    assert_eq!(routing.policy, RoutingPolicyV3::OrderedFailover);
    assert_eq!(routing.order, vec!["primary", "backup"]);
    assert_eq!(
        codex
            .profiles
            .get("daily")
            .and_then(|profile| profile.station.as_deref()),
        Some("routing")
    );
}

#[test]
fn migrate_v2_to_v3_report_warns_when_flattening_endpoint_scoped_groups() {
    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("primary".to_string()),
            providers: BTreeMap::from([
                (
                    "relay".to_string(),
                    ProviderConfigV2 {
                        endpoints: BTreeMap::from([
                            (
                                "fast".to_string(),
                                ProviderEndpointV2 {
                                    base_url: "https://fast.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 0,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                },
                            ),
                            (
                                "slow".to_string(),
                                ProviderEndpointV2 {
                                    base_url: "https://slow.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 10,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                },
                            ),
                        ]),
                        ..ProviderConfigV2::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfigV2 {
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://backup.example.com/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        )]),
                        ..ProviderConfigV2::default()
                    },
                ),
            ]),
            groups: BTreeMap::from([
                (
                    "primary".to_string(),
                    GroupConfigV2 {
                        alias: Some("Primary".to_string()),
                        enabled: true,
                        level: 1,
                        members: vec![GroupMemberRefV2 {
                            provider: "relay".to_string(),
                            endpoint_names: vec!["fast".to_string()],
                            preferred: false,
                        }],
                    },
                ),
                (
                    "secondary".to_string(),
                    GroupConfigV2 {
                        alias: Some("Secondary".to_string()),
                        enabled: true,
                        level: 2,
                        members: vec![
                            GroupMemberRefV2 {
                                provider: "relay".to_string(),
                                endpoint_names: vec!["slow".to_string()],
                                preferred: false,
                            },
                            GroupMemberRefV2 {
                                provider: "backup".to_string(),
                                endpoint_names: Vec::new(),
                                preferred: false,
                            },
                        ],
                    },
                ),
            ]),
            ..ServiceViewV2::default()
        },
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let report = migrate_v2_to_v3_with_report(&v2).expect("migrate v2 to v3");
    let warnings = report.warnings.join("\n");
    assert!(warnings.contains("flattens the effective route"));
    assert!(warnings.contains("scopes provider 'relay'"));
    assert!(warnings.contains("de-duplicated"));

    let routing = report
        .config
        .codex
        .routing
        .expect("routing should be emitted");
    assert_eq!(routing.order, vec!["relay", "backup"]);
}

#[test]
fn migrate_v2_to_v3_omits_disabled_inactive_groups_from_routing_order() {
    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("primary".to_string()),
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfigV2 {
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://primary.example.com/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        )]),
                        ..ProviderConfigV2::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfigV2 {
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://backup.example.com/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        )]),
                        ..ProviderConfigV2::default()
                    },
                ),
            ]),
            groups: BTreeMap::from([
                (
                    "primary".to_string(),
                    GroupConfigV2 {
                        enabled: true,
                        level: 1,
                        members: vec![GroupMemberRefV2 {
                            provider: "primary".to_string(),
                            endpoint_names: Vec::new(),
                            preferred: false,
                        }],
                        ..GroupConfigV2::default()
                    },
                ),
                (
                    "disabled-backup".to_string(),
                    GroupConfigV2 {
                        enabled: false,
                        level: 2,
                        members: vec![GroupMemberRefV2 {
                            provider: "backup".to_string(),
                            endpoint_names: Vec::new(),
                            preferred: false,
                        }],
                        ..GroupConfigV2::default()
                    },
                ),
            ]),
            ..ServiceViewV2::default()
        },
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let report = migrate_v2_to_v3_with_report(&v2).expect("migrate v2 to v3");
    let routing = report
        .config
        .codex
        .routing
        .expect("routing should be emitted");
    assert_eq!(routing.order, vec!["primary"]);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("disabled inactive"))
    );
}

#[test]
fn save_config_v3_writes_routing_first_schema() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let cfg = ProxyConfigV3 {
            version: 3,
            codex: ServiceViewV3 {
                providers: BTreeMap::from([(
                    "main".to_string(),
                    ProviderConfigV3 {
                        base_url: Some("https://api.example.com/v1".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("MAIN_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfigV3::default()
                    },
                )]),
                routing: Some(RoutingConfigV3 {
                    policy: RoutingPolicyV3::ManualSticky,
                    target: Some("main".to_string()),
                    ..RoutingConfigV3::default()
                }),
                ..ServiceViewV3::default()
            },
            ..ProxyConfigV3::default()
        };

        let path = super::save_config_v3(&cfg).await.expect("save v3");
        let saved = std::fs::read_to_string(path).expect("read saved v3 config");
        assert!(saved.contains("version = 3"));
        assert!(saved.contains("[codex.routing]"));
        assert!(saved.contains("policy = \"manual-sticky\""));
        assert!(saved.contains("auth_token_env = \"MAIN_API_KEY\""));
        assert!(!saved.contains("[codex.stations."));
    });
}

#[test]
fn save_config_preserves_v3_routing_when_only_runtime_metadata_changes() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let cfg = ProxyConfigV3 {
            version: 3,
            codex: ServiceViewV3 {
                providers: BTreeMap::from([
                    (
                        "monthly".to_string(),
                        ProviderConfigV3 {
                            base_url: Some("https://monthly.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                            ..ProviderConfigV3::default()
                        },
                    ),
                    (
                        "paygo".to_string(),
                        ProviderConfigV3 {
                            base_url: Some("https://paygo.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                            ..ProviderConfigV3::default()
                        },
                    ),
                ]),
                routing: Some(RoutingConfigV3 {
                    policy: RoutingPolicyV3::TagPreferred,
                    prefer_tags: vec![BTreeMap::from([(
                        "billing".to_string(),
                        "monthly".to_string(),
                    )])],
                    order: vec!["monthly".to_string(), "paygo".to_string()],
                    on_exhausted: RoutingExhaustedActionV3::Stop,
                    ..RoutingConfigV3::default()
                }),
                ..ServiceViewV3::default()
            },
            ..ProxyConfigV3::default()
        };

        let path = super::save_config_v3(&cfg).await.expect("save v3");
        let mut runtime = super::load_config().await.expect("load runtime");
        runtime.ui.language = Some("zh".to_string());
        super::save_config(&runtime)
            .await
            .expect("save runtime metadata");

        let saved = std::fs::read_to_string(path).expect("read saved config");
        let reparsed = toml::from_str::<ProxyConfigV3>(&saved).expect("parse v3");
        let routing = reparsed.codex.routing.expect("routing should remain");
        assert_eq!(routing.policy, RoutingPolicyV3::TagPreferred);
        assert_eq!(routing.on_exhausted, RoutingExhaustedActionV3::Stop);
        assert_eq!(routing.order, vec!["monthly", "paygo"]);
        assert_eq!(reparsed.ui.language.as_deref(), Some("zh"));
    });
}
