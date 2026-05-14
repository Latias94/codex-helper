use super::*;

#[test]
fn load_config_supports_v4_route_graph_schema() {
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
version = 4

[codex.providers.monthly]
base_url = "https://monthly.example.com/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.paygo]
base_url = "https://paygo.example.com/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo", region = "us" }

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["paygo", "monthly"]
on_exhausted = "continue"
"#,
        );

        let cfg = super::load_config().await.expect("load v4 config");
        assert_eq!(cfg.version, Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION));
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
fn v4_tag_preferred_stop_excludes_non_matching_fallbacks() {
    let v4 = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://monthly.example.com/v1".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("MONTHLY_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["monthly".to_string(), "paygo".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Stop,
            )),
        },
        claude: ServiceViewV4::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let runtime = compile_v4_to_runtime(&v4).expect("compile v4");
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
fn v4_nested_route_graph_expands_monthly_pool_before_paygo() {
    let v4 = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://input.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "input1".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://input1.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "input2".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://input2.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "codex-for".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://codex-for.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec![
                                "input".to_string(),
                                "input1".to_string(),
                                "input2".to_string(),
                            ],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "codex-for".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
        },
        claude: ServiceViewV4::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let runtime = compile_v4_to_runtime(&v4).expect("compile v4");
    let routing = runtime
        .codex
        .station("routing")
        .expect("routing station should exist");
    let base_urls = routing
        .upstreams
        .iter()
        .map(|upstream| upstream.base_url.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        base_urls,
        vec![
            "https://input.example.com/v1",
            "https://input1.example.com/v1",
            "https://input2.example.com/v1",
            "https://codex-for.example.com/v1",
        ]
    );
}

#[test]
fn compile_v4_to_runtime_direct_path_matches_v2_bridge_for_compat_state() {
    let v4 = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            default_profile: Some("daily".to_string()),
            profiles: BTreeMap::from([(
                "daily".to_string(),
                ServiceControlProfile {
                    reasoning_effort: Some("medium".to_string()),
                    ..ServiceControlProfile::default()
                },
            )]),
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfigV4 {
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("INPUT_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                        endpoints: BTreeMap::from([
                            (
                                "slow".to_string(),
                                ProviderEndpointV4 {
                                    base_url: "https://slow.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 10,
                                    tags: BTreeMap::from([(
                                        "region".to_string(),
                                        "us".to_string(),
                                    )]),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                },
                            ),
                            (
                                "fast".to_string(),
                                ProviderEndpointV4 {
                                    base_url: "https://fast.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 0,
                                    tags: BTreeMap::from([(
                                        "region".to_string(),
                                        "hk".to_string(),
                                    )]),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::from([(
                                        "gpt-5".to_string(),
                                        "provider-gpt-5".to_string(),
                                    )]),
                                },
                            ),
                        ]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "disabled".to_string(),
                    ProviderConfigV4 {
                        enabled: false,
                        base_url: Some("https://disabled.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "input".to_string(),
                "disabled".to_string(),
                "paygo".to_string(),
            ])),
        },
        claude: ServiceViewV4::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let direct = compile_v4_to_runtime(&v4).expect("compile direct v4 runtime");
    let bridge_v2 = compile_v4_to_v2(&v4).expect("compile v4 bridge v2");
    let mut bridge = compile_v2_to_runtime(&bridge_v2).expect("compile bridge runtime");
    bridge.version = Some(4);

    assert_eq!(
        serde_json::to_value(&direct).expect("serialize direct runtime"),
        serde_json::to_value(&bridge).expect("serialize bridge runtime")
    );
    let routing = direct
        .codex
        .station("routing")
        .expect("routing station should exist");
    assert_eq!(
        routing
            .upstreams
            .iter()
            .map(|upstream| upstream.base_url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://fast.example.com/v1",
            "https://slow.example.com/v1",
            "https://paygo.example.com/v1",
        ]
    );
    assert_eq!(
        routing.upstreams[0]
            .tags
            .get("provider_id")
            .map(String::as_str),
        Some("input")
    );
    assert_eq!(
        routing.upstreams[0]
            .model_mapping
            .get("gpt-5")
            .map(String::as_str),
        Some("provider-gpt-5")
    );
}

#[test]
fn v4_route_graph_rejects_cycles() {
    let v4 = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    base_url: Some("https://input.example.com/v1".to_string()),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4 {
                entry: "a".to_string(),
                routes: BTreeMap::from([
                    (
                        "a".to_string(),
                        RoutingNodeV4 {
                            children: vec!["b".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "b".to_string(),
                        RoutingNodeV4 {
                            children: vec!["a".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };

    let err = compile_v4_to_runtime(&v4).expect_err("cycle should fail");
    assert!(err.to_string().contains("routing graph has a cycle"));
}

#[test]
fn v4_route_graph_rejects_missing_reference() {
    let v4 = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    base_url: Some("https://input.example.com/v1".to_string()),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4 {
                entry: "main".to_string(),
                routes: BTreeMap::from([(
                    "main".to_string(),
                    RoutingNodeV4 {
                        children: vec!["missing".to_string()],
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };

    let err = compile_v4_to_runtime(&v4).expect_err("missing reference should fail");
    assert!(
        err.to_string()
            .contains("routing references missing route or provider 'missing'")
    );
}

#[test]
fn legacy_v3_pool_fallback_migrates_to_nested_v4_route_nodes() {
    let legacy = crate::config::legacy::ProxyConfigV3Legacy {
        version: 3,
        codex: crate::config::legacy::ServiceViewV3Legacy {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://input.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "input1".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://input1.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "codex-for".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://codex-for.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(crate::config::legacy::RoutingConfigV3Legacy {
                policy: crate::config::legacy::RoutingPolicyV3Legacy::PoolFallback,
                chain: vec!["input".to_string(), "paygo".to_string()],
                pools: BTreeMap::from([
                    (
                        "input".to_string(),
                        crate::config::RoutingPoolV4 {
                            providers: vec!["input".to_string(), "input1".to_string()],
                        },
                    ),
                    (
                        "paygo".to_string(),
                        crate::config::RoutingPoolV4 {
                            providers: vec!["codex-for".to_string()],
                        },
                    ),
                ]),
                ..crate::config::legacy::RoutingConfigV3Legacy::default()
            }),
            ..crate::config::legacy::ServiceViewV3Legacy::default()
        },
        claude: crate::config::legacy::ServiceViewV3Legacy::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let report = crate::config::legacy::migrate_v3_legacy_to_v4(&legacy)
        .expect("legacy v3 should migrate to v4");
    assert_eq!(report.config.version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION);
    let routing = report
        .config
        .codex
        .routing
        .as_ref()
        .expect("routing should migrate");
    assert_eq!(routing.entry, "main");
    assert_eq!(
        routing.entry_node().map(|node| node.children.clone()),
        Some(vec!["input_pool".to_string(), "paygo".to_string()])
    );

    let runtime = compile_v4_to_runtime(&report.config).expect("compile migrated v4");
    let routing_station = runtime
        .codex
        .station("routing")
        .expect("routing station should exist");
    assert_eq!(
        routing_station
            .upstreams
            .iter()
            .map(|upstream| upstream.base_url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://input.example.com/v1",
            "https://input1.example.com/v1",
            "https://codex-for.example.com/v1",
        ]
    );
}

#[test]
fn migrate_v2_to_v4_emits_route_graph_and_inline_simple_providers() {
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

    let migrated = migrate_v2_to_v4(&v2).expect("migrate v2 to v4");
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
    assert_eq!(routing.policy, RoutingPolicyV4::OrderedFailover);
    assert_eq!(routing.order, vec!["primary", "backup"]);
    assert_eq!(
        routing.entry_node().map(|node| node.children.clone()),
        Some(vec!["primary".to_string(), "backup".to_string()])
    );
    assert_eq!(
        codex
            .profiles
            .get("daily")
            .and_then(|profile| profile.station.as_deref()),
        None
    );
}

#[test]
fn migrate_v2_to_v4_report_warns_when_flattening_endpoint_scoped_groups() {
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

    let report = migrate_v2_to_v4_with_report(&v2).expect("migrate v2 to v4");
    let warnings = report.warnings.join("\n");
    assert!(warnings.contains("flattens the effective route"));
    assert!(warnings.contains("scopes provider 'relay'"));
    assert!(warnings.contains("de-duplicated"));
    assert!(warnings.contains("fallback-sticky"));

    let routing = report
        .config
        .codex
        .routing
        .expect("routing should be emitted");
    assert_eq!(routing.order, vec!["relay", "backup"]);
    assert_eq!(
        routing.entry_node().map(|node| node.children.clone()),
        Some(vec!["relay".to_string(), "backup".to_string()])
    );
}

#[test]
fn v4_route_graph_affinity_upgrade_warning_mentions_fallback_sticky() {
    let cfg = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://monthly.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "monthly".to_string(),
                "paygo".to_string(),
            ])),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };

    let mut warnings = Vec::new();
    collect_route_graph_affinity_migration_warnings("codex", &cfg.codex, &mut warnings);

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("preferred-group"));
    assert!(warnings[0].contains("fallback-sticky"));
}

#[test]
fn v4_route_graph_affinity_upgrade_warning_ignores_single_provider() {
    let cfg = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            providers: BTreeMap::from([(
                "monthly".to_string(),
                ProviderConfigV4 {
                    base_url: Some("https://monthly.example.com/v1".to_string()),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "monthly".to_string(),
            ])),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };

    let mut warnings = Vec::new();
    collect_route_graph_affinity_migration_warnings("codex", &cfg.codex, &mut warnings);

    assert!(warnings.is_empty());
}

#[test]
fn migrate_v2_to_v4_omits_disabled_inactive_groups_from_route_graph() {
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

    let report = migrate_v2_to_v4_with_report(&v2).expect("migrate v2 to v4");
    let routing = report
        .config
        .codex
        .routing
        .expect("routing should be emitted");
    assert_eq!(routing.order, vec!["primary"]);
    assert_eq!(
        routing.entry_node().map(|node| node.children.clone()),
        Some(vec!["primary".to_string()])
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("disabled inactive"))
    );
}

#[test]
fn save_config_v4_writes_v4_route_graph_schema() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let cfg = ProxyConfigV4 {
            version: 4,
            codex: ServiceViewV4 {
                providers: BTreeMap::from([(
                    "main".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://api.example.com/v1".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("MAIN_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([
                            ("provider_id".to_string(), "main".to_string()),
                            ("requires_openai_auth".to_string(), "false".to_string()),
                            ("source".to_string(), "codex-config".to_string()),
                        ]),
                        ..ProviderConfigV4::default()
                    },
                )]),
                routing: Some(RoutingConfigV4::manual_sticky(
                    "main".to_string(),
                    vec!["main".to_string()],
                )),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };

        let path = super::save_config_v4(&cfg).await.expect("save v4");
        let saved = std::fs::read_to_string(path).expect("read saved v4 config");
        assert!(saved.contains("version = 5"));
        assert!(saved.contains("[codex.routing]"));
        assert!(saved.contains("entry = \"main_route\""));
        assert!(!saved.contains("affinity_policy"));
        assert!(saved.contains("[codex.routing.routes.main_route]"));
        assert!(saved.contains("strategy = \"manual-sticky\""));
        assert!(saved.contains("target = \"main\""));
        assert!(saved.contains("auth_token_env = \"MAIN_API_KEY\""));
        assert!(!saved.contains("enabled = true"));
        assert!(!saved.contains("[codex.providers.main.tags]"));
        assert!(!saved.contains("provider_id = \"main\""));
        assert!(!saved.contains("requires_openai_auth"));
        assert!(!saved.contains("source = \"codex-config\""));
        assert!(!saved.contains("[codex.stations."));
    });
}

#[test]
fn load_config_auto_compacts_legacy_v3_import_metadata_to_v4() {
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

[codex.providers.input]
enabled = true
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.input.tags]
provider_id = "input"
requires_openai_auth = "false"
source = "codex-config"

[codex.routing]
policy = "manual-sticky"
target = "input"
"#,
        );

        let cfg = super::load_config().await.expect("load legacy v3 config");
        assert_eq!(cfg.version, Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION));
        let saved = std::fs::read_to_string(&toml_path).expect("read compacted config");
        assert!(saved.contains("version = 5"));
        assert!(saved.contains("[codex.routing.routes.main]"));
        assert!(saved.contains("strategy = \"manual-sticky\""));
        assert!(saved.contains("[codex.providers.input]"));
        assert!(saved.contains("auth_token_env = \"INPUT_API_KEY\""));
        assert!(!saved.contains("enabled = true"));
        assert!(!saved.contains("[codex.providers.input.tags]"));
        assert!(!saved.contains("provider_id"));
        assert!(!saved.contains("requires_openai_auth"));
        assert!(!saved.contains("source = \"codex-config\""));
    });
}

#[test]
fn save_config_preserves_v4_route_graph_when_only_runtime_metadata_changes() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["monthly".to_string(), "paygo".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Stop,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::FallbackSticky;
        let cfg = ProxyConfigV4 {
            version: 4,
            codex: ServiceViewV4 {
                providers: BTreeMap::from([
                    (
                        "monthly".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("https://monthly.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "paygo".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("https://paygo.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(routing),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };

        let path = super::save_config_v4(&cfg).await.expect("save v4");
        let mut runtime = super::load_config().await.expect("load runtime");
        runtime.ui.language = Some("zh".to_string());
        super::save_config(&runtime)
            .await
            .expect("save runtime metadata");

        let saved = std::fs::read_to_string(path).expect("read saved config");
        let reparsed = toml::from_str::<ProxyConfigV4>(&saved).expect("parse v4");
        assert_eq!(reparsed.version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION);
        let routing = reparsed.codex.routing.expect("routing should remain");
        let entry = routing.entry_node().expect("entry node should remain");
        assert_eq!(entry.strategy, RoutingPolicyV4::TagPreferred);
        assert_eq!(entry.on_exhausted, RoutingExhaustedActionV4::Stop);
        assert_eq!(entry.children, vec!["monthly", "paygo"]);
        assert_eq!(
            routing.affinity_policy,
            RoutingAffinityPolicyV5::FallbackSticky
        );
        assert_eq!(reparsed.ui.language.as_deref(), Some("zh"));
    });
}

#[test]
fn old_v4_route_graph_auto_migrates_to_v5_and_preserves_endpoint_identity() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        let backup_path = dir.join("config.toml.bak");
        write_file(
            &toml_path,
            r#"
version = 4

[codex.providers.monthly]
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly.endpoints.fast]
base_url = "https://monthly-fast.example.com/v1"
priority = 0
tags = { region = "hk" }

[codex.providers.monthly.endpoints.slow]
base_url = "https://monthly-slow.example.com/v1"
priority = 10
tags = { region = "us" }

[codex.providers.paygo]
base_url = "https://paygo.example.com/v1"
tags = { billing = "paygo" }

[codex.routing]
entry = "main"
fallback_ttl_ms = 120000
reprobe_preferred_after_ms = 30000

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["monthly", "paygo"]
"#,
        );

        let loaded = super::load_config_with_v4_source()
            .await
            .expect("load old v4 route graph");
        assert_eq!(
            loaded.runtime.version,
            Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION)
        );
        assert_eq!(
            loaded.v4.as_ref().map(|cfg| cfg.version),
            Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION)
        );

        let saved = std::fs::read_to_string(&toml_path).expect("read migrated v5 config");
        assert!(saved.contains("version = 5"));
        assert!(saved.contains("fallback_ttl_ms = 120000"));
        assert!(saved.contains("reprobe_preferred_after_ms = 30000"));
        assert!(saved.contains("[codex.routing.routes.main]"));
        assert!(saved.contains("[codex.providers.monthly.endpoints.fast]"));
        assert!(saved.contains("auth_token_env = \"MONTHLY_API_KEY\""));

        let backup = std::fs::read_to_string(&backup_path).expect("read old v4 backup");
        assert!(backup.contains("version = 4"));
        assert!(backup.contains("[codex.providers.monthly.endpoints.fast]"));

        let reparsed = toml::from_str::<ProxyConfigV4>(&saved).expect("parse migrated v5 config");
        let reparsed_routing = reparsed
            .codex
            .routing
            .as_ref()
            .expect("migrated routing should remain");
        assert_eq!(reparsed_routing.fallback_ttl_ms, Some(120_000));
        assert_eq!(reparsed_routing.reprobe_preferred_after_ms, Some(30_000));
        let runtime = compile_v4_to_runtime(&reparsed).expect("compile migrated v5 config");
        let routing = runtime
            .codex
            .station("routing")
            .expect("compat routing projection should exist");
        assert_eq!(
            routing
                .upstreams
                .iter()
                .map(|upstream| upstream.base_url.as_str())
                .collect::<Vec<_>>(),
            vec![
                "https://monthly-fast.example.com/v1",
                "https://monthly-slow.example.com/v1",
                "https://paygo.example.com/v1",
            ]
        );
        assert_eq!(
            routing.upstreams[0]
                .tags
                .get("provider_id")
                .map(String::as_str),
            Some("monthly")
        );
        assert_eq!(
            routing.upstreams[0]
                .tags
                .get("endpoint_id")
                .map(String::as_str),
            Some("fast")
        );
        assert_eq!(
            routing.upstreams[2]
                .tags
                .get("endpoint_id")
                .map(String::as_str),
            Some("default")
        );
    });
}

#[test]
fn station_shaped_v2_config_migrates_to_route_graph_without_profile_station_binding() {
    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("monthly-first".to_string()),
            default_profile: Some("daily".to_string()),
            profiles: BTreeMap::from([(
                "daily".to_string(),
                ServiceControlProfile {
                    station: Some("monthly-first".to_string()),
                    model: Some("gpt-5".to_string()),
                    ..ServiceControlProfile::default()
                },
            )]),
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfigV2 {
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        endpoints: BTreeMap::from([
                            (
                                "fast".to_string(),
                                ProviderEndpointV2 {
                                    base_url: "https://monthly-fast.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 0,
                                    tags: BTreeMap::from([(
                                        "region".to_string(),
                                        "hk".to_string(),
                                    )]),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                },
                            ),
                            (
                                "slow".to_string(),
                                ProviderEndpointV2 {
                                    base_url: "https://monthly-slow.example.com/v1".to_string(),
                                    enabled: true,
                                    priority: 10,
                                    tags: BTreeMap::from([(
                                        "region".to_string(),
                                        "us".to_string(),
                                    )]),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                },
                            ),
                        ]),
                        ..ProviderConfigV2::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfigV2 {
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        endpoints: BTreeMap::from([(
                            "default".to_string(),
                            ProviderEndpointV2 {
                                base_url: "https://paygo.example.com/v1".to_string(),
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
                    "monthly-first".to_string(),
                    GroupConfigV2 {
                        enabled: true,
                        level: 1,
                        members: vec![GroupMemberRefV2 {
                            provider: "monthly".to_string(),
                            endpoint_names: vec!["fast".to_string()],
                            preferred: false,
                        }],
                        ..GroupConfigV2::default()
                    },
                ),
                (
                    "fallback".to_string(),
                    GroupConfigV2 {
                        enabled: true,
                        level: 2,
                        members: vec![GroupMemberRefV2 {
                            provider: "paygo".to_string(),
                            endpoint_names: Vec::new(),
                            preferred: false,
                        }],
                        ..GroupConfigV2::default()
                    },
                ),
            ]),
        },
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let report = migrate_v2_to_v4_with_report(&v2).expect("migrate station-shaped config");
    assert_eq!(report.config.version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("profile station bindings are cleared"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("scopes provider 'monthly'"))
    );

    let codex = &report.config.codex;
    assert_eq!(
        codex
            .profiles
            .get("daily")
            .and_then(|profile| profile.station.as_deref()),
        None
    );
    let routing = codex.routing.as_ref().expect("routing should be emitted");
    assert_eq!(routing.order, vec!["monthly", "paygo"]);
    assert_eq!(
        routing.entry_node().map(|node| node.children.clone()),
        Some(vec!["monthly".to_string(), "paygo".to_string()])
    );

    let runtime = compile_v4_to_runtime(&report.config).expect("compile migrated route graph");
    assert_eq!(runtime.codex.station_count(), 1);
    assert_eq!(runtime.codex.active.as_deref(), Some("routing"));
    let routing_station = runtime
        .codex
        .station("routing")
        .expect("compat routing projection should exist");
    assert_eq!(
        routing_station
            .upstreams
            .iter()
            .map(|upstream| upstream.base_url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://monthly-fast.example.com/v1",
            "https://monthly-slow.example.com/v1",
            "https://paygo.example.com/v1",
        ]
    );
    assert_eq!(
        routing_station.upstreams[0]
            .tags
            .get("provider_id")
            .map(String::as_str),
        Some("monthly")
    );
    assert_eq!(
        routing_station.upstreams[0]
            .tags
            .get("endpoint_id")
            .map(String::as_str),
        Some("fast")
    );
}
