use super::*;

#[test]
fn load_config_supports_v5_route_graph_schema() {
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
version = 5

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

        let loaded = super::load_config_with_source()
            .await
            .expect("load v5 config");
        assert_eq!(loaded.source.version, CURRENT_CONFIG_VERSION);

        let routing =
            crate::routing_ir::compile_route_handshake_plan("codex", &loaded.source.codex)
                .expect("compile canonical route graph");
        assert_eq!(
            routing
                .candidates
                .iter()
                .map(|candidate| {
                    (
                        candidate.provider_id.as_str(),
                        candidate.endpoint_id.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![("monthly", "default"), ("paygo", "default")]
        );
        assert_eq!(
            routing.candidates[0].base_url,
            "https://monthly.example.com/v1"
        );
        assert_eq!(
            routing.candidates[0].auth.auth_token_env.as_deref(),
            Some("MONTHLY_API_KEY")
        );
        assert_eq!(
            routing.candidates[1].base_url,
            "https://paygo.example.com/v1"
        );
    });
}

#[test]
fn load_current_v5_fleet_registry_validates_remote_admin_tokens() {
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
version = 5

[fleet.nodes.local]
label = "Local"
admin_url = "http://127.0.0.1:4211"
enabled = true

[fleet.nodes.workstation]
label = "Workstation"
admin_url = "https://workstation.example.com:4211"
admin_token_env = "CODEX_HELPER_WORKSTATION_ADMIN_TOKEN"
enabled = true
"#,
        );

        let cfg = super::load_config().await.expect("load fleet config");
        assert_eq!(cfg.fleet.nodes.len(), 2);
        assert_eq!(
            cfg.fleet.nodes["workstation"].admin_token_env.as_deref(),
            Some("CODEX_HELPER_WORKSTATION_ADMIN_TOKEN")
        );
    });
}

#[test]
fn load_config_rejects_remote_fleet_http_without_admin_token_env() {
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
version = 5

[fleet.nodes.remote]
label = "Remote"
admin_url = "http://nas.example.com:4211"
enabled = true
"#,
        );

        let err = super::load_config()
            .await
            .expect_err("remote fleet node without token should fail");
        assert!(err.to_string().contains("HTTPS"), "unexpected: {err}");
    });
}

#[test]
fn load_config_rejects_invalid_fleet_token_env_name() {
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
version = 5

[fleet.nodes.remote]
label = "Remote"
admin_url = "https://nas.example.com:4211"
admin_token_env = "Bearer abc"
enabled = true
"#,
        );

        let err = super::load_config()
            .await
            .expect_err("invalid env name should fail");
        assert!(err.to_string().contains("environment variable name"));
    });
}

#[test]
fn current_v5_tag_preferred_stop_excludes_non_matching_fallbacks() {
    let source = HelperConfig {
        version: CURRENT_CONFIG_VERSION,
        codex: ServiceRouteConfig {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfig {
                        base_url: Some("https://monthly.example.com/v1".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("MONTHLY_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfig {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["monthly".to_string(), "paygo".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Stop,
            )),
        },
        claude: ServiceRouteConfig::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        relay_targets: std::collections::BTreeMap::new(),
        fleet: Default::default(),
        ui: UiConfig::default(),
    };

    validate_helper_config(&source).expect("validate current config");

    let routing = crate::routing_ir::compile_route_handshake_plan("codex", &source.codex)
        .expect("compile canonical route graph");
    assert_eq!(routing.candidates.len(), 1);
    assert_eq!(routing.candidates[0].provider_id, "monthly");
    assert_eq!(routing.candidates[0].endpoint_id, "default");
    assert_eq!(
        routing.candidates[0].base_url,
        "https://monthly.example.com/v1"
    );
}

#[test]
fn current_v5_nested_route_graph_expands_monthly_pool_before_paygo() {
    let source = HelperConfig {
        version: CURRENT_CONFIG_VERSION,
        codex: ServiceRouteConfig {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfig {
                        base_url: Some("https://input.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "input1".to_string(),
                    ProviderConfig {
                        base_url: Some("https://input1.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "input2".to_string(),
                    ProviderConfig {
                        base_url: Some("https://input2.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "codex-for".to_string(),
                    ProviderConfig {
                        base_url: Some("https://codex-for.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec![
                                "input".to_string(),
                                "input1".to_string(),
                                "input2".to_string(),
                            ],
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "codex-for".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
        },
        claude: ServiceRouteConfig::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        relay_targets: std::collections::BTreeMap::new(),
        fleet: Default::default(),
        ui: UiConfig::default(),
    };

    validate_helper_config(&source).expect("validate current config");

    let routing = crate::routing_ir::compile_route_handshake_plan("codex", &source.codex)
        .expect("compile canonical route graph");
    let base_urls = routing
        .candidates
        .iter()
        .map(|candidate| candidate.base_url.as_str())
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
    assert_eq!(
        routing
            .candidates
            .iter()
            .map(|candidate| candidate.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["input", "input1", "input2", "codex-for"]
    );
}

#[test]
fn current_v5_compiled_route_graph_preserves_endpoint_order_and_model_mapping() {
    let source = HelperConfig {
        version: CURRENT_CONFIG_VERSION,
        codex: ServiceRouteConfig {
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
                    ProviderConfig {
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some("INPUT_API_KEY".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                        supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                        endpoints: BTreeMap::from([
                            (
                                "slow".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://slow.example.com/v1".to_string(),
                                    continuity_domain: None,
                                    enabled: true,
                                    priority: 10,
                                    tags: BTreeMap::from([(
                                        "region".to_string(),
                                        "us".to_string(),
                                    )]),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                            (
                                "fast".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://fast.example.com/v1".to_string(),
                                    continuity_domain: None,
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
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                        ]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "disabled".to_string(),
                    ProviderConfig {
                        enabled: false,
                        base_url: Some("https://disabled.example.com/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "paygo".to_string(),
                    ProviderConfig {
                        base_url: Some("https://paygo.example.com/v1".to_string()),
                        tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input".to_string(),
                "disabled".to_string(),
                "paygo".to_string(),
            ])),
        },
        claude: ServiceRouteConfig::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        relay_targets: std::collections::BTreeMap::new(),
        fleet: Default::default(),
        ui: UiConfig::default(),
    };

    validate_helper_config(&source).expect("validate current config");

    let routing = crate::routing_ir::CompiledRouteGraph::compile("codex", &source.codex)
        .expect("compile canonical route graph");
    let candidates = routing.candidates();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.base_url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://fast.example.com/v1",
            "https://slow.example.com/v1",
            "https://paygo.example.com/v1",
        ]
    );
    assert_eq!(
        candidates[0].tags.get("provider_id").map(String::as_str),
        Some("input")
    );
    assert_eq!(
        candidates[0].model_mapping.get("gpt-5").map(String::as_str),
        Some("provider-gpt-5")
    );
    assert_eq!(candidates[0].endpoint_id, "fast");
    assert_eq!(candidates[1].endpoint_id, "slow");
    assert_eq!(candidates[2].provider_id, "paygo");
    assert_eq!(candidates[2].endpoint_id, "default");
}

#[test]
fn current_v5_route_graph_rejects_cycles() {
    let source = HelperConfig {
        version: CURRENT_CONFIG_VERSION,
        codex: ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    base_url: Some("https://input.example.com/v1".to_string()),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "a".to_string(),
                routes: BTreeMap::from([
                    (
                        "a".to_string(),
                        RouteNodeConfig {
                            children: vec!["b".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "b".to_string(),
                        RouteNodeConfig {
                            children: vec!["a".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };

    let err = validate_helper_config(&source).expect_err("cycle should fail");
    assert!(err.to_string().contains("routing graph has a cycle"));
}

#[test]
fn current_v5_route_graph_rejects_missing_reference() {
    let source = HelperConfig {
        version: CURRENT_CONFIG_VERSION,
        codex: ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    base_url: Some("https://input.example.com/v1".to_string()),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "main".to_string(),
                routes: BTreeMap::from([(
                    "main".to_string(),
                    RouteNodeConfig {
                        children: vec!["missing".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };

    let err = validate_helper_config(&source).expect_err("missing reference should fail");
    assert!(
        err.to_string()
            .contains("routing references missing route or provider 'missing'")
    );
}

#[test]
fn save_current_v5_writes_route_graph_and_preserves_provider_tags() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let cfg = HelperConfig {
            version: CURRENT_CONFIG_VERSION,
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "main".to_string(),
                    ProviderConfig {
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
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::manual_sticky(
                    "main".to_string(),
                    vec!["main".to_string()],
                )),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };

        let path = super::save_helper_config(&cfg)
            .await
            .expect("save current config");
        let saved = std::fs::read_to_string(path).expect("read saved current config");
        assert!(saved.contains("version = 5"));
        assert!(saved.contains("[codex.routing]"));
        assert!(saved.contains("entry = \"main_route\""));
        assert!(saved.contains("affinity_policy = \"fallback-sticky\""));
        assert!(saved.contains("[codex.routing.routes.main_route]"));
        assert!(saved.contains("strategy = \"manual-sticky\""));
        assert!(saved.contains("target = \"main\""));
        assert!(saved.contains("auth_token_env = \"MAIN_API_KEY\""));
        assert!(!saved.contains("enabled = true"));
        assert!(saved.contains("provider_id = \"main\""));
        assert!(saved.contains("requires_openai_auth = \"false\""));
        assert!(saved.contains("source = \"codex-config\""));
        assert!(!saved.contains("[codex.stations."));
    });
}

#[test]
fn compile_current_v5_rejects_zero_provider_concurrency_limit() {
    let source = HelperConfig {
        version: 5,
        codex: ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    base_url: Some("https://relay.example/v1".to_string()),
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(0),
                        limit_group: None,
                    },
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };

    let err = validate_helper_config(&source).expect_err("zero concurrency limit should fail");
    assert!(
        err.to_string()
            .contains("limits.max_concurrent_requests must be greater than 0"),
        "unexpected error: {err}"
    );
}

#[test]
fn load_config_does_not_auto_add_default_route_graph_affinity_policy() {
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
version = 5

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "manual-sticky"
target = "main"
"#,
        );

        let loaded = super::load_config_with_source()
            .await
            .expect("load v5 config");
        assert_eq!(loaded.source.version, CURRENT_CONFIG_VERSION);

        let saved = std::fs::read_to_string(&toml_path).expect("read unchanged config");
        assert!(saved.contains("[codex.routing]"));
        assert!(!saved.contains("affinity_policy = \"fallback-sticky\""));
        assert!(!dir.join("config.toml.bak").exists());
    });
}

#[test]
fn save_current_v5_preserves_route_graph_when_metadata_changes() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["monthly".to_string(), "paygo".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Stop,
        );
        routing.affinity_policy = RouteAffinityPolicy::FallbackSticky;
        let cfg = HelperConfig {
            version: CURRENT_CONFIG_VERSION,
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "monthly".to_string(),
                        ProviderConfig {
                            base_url: Some("https://monthly.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "paygo".to_string(),
                        ProviderConfig {
                            base_url: Some("https://paygo.example.com/v1".to_string()),
                            tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(routing),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };

        let path = super::save_helper_config(&cfg)
            .await
            .expect("save current config");
        let mut source = super::load_config_with_source()
            .await
            .expect("load source")
            .source;
        source.ui.language = Some("zh".to_string());
        super::save_helper_config(&source)
            .await
            .expect("save source metadata");

        let saved = std::fs::read_to_string(path).expect("read saved config");
        let reparsed = toml::from_str::<HelperConfig>(&saved).expect("parse current config");
        assert_eq!(reparsed.version, CURRENT_CONFIG_VERSION);
        let routing = reparsed.codex.routing.expect("routing should remain");
        let entry = routing.entry_node().expect("entry node should remain");
        assert_eq!(entry.strategy, RouteStrategy::TagPreferred);
        assert_eq!(entry.on_exhausted, RouteExhaustedAction::Stop);
        assert_eq!(entry.children, vec!["monthly", "paygo"]);
        assert_eq!(routing.affinity_policy, RouteAffinityPolicy::FallbackSticky);
        assert_eq!(reparsed.ui.language.as_deref(), Some("zh"));
    });
}

#[test]
fn save_canonical_config_preserves_route_graph() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let cfg = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "relay".to_string(),
                    ProviderConfig {
                        base_url: Some("https://relay.example/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "relay".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let path = super::save_helper_config(&cfg)
            .await
            .expect("save current source");
        let mut source = super::load_config().await.expect("load canonical config");
        source.ui.language = Some("zh".to_string());

        super::save_helper_config(&source)
            .await
            .expect("update canonical source");

        let saved = std::fs::read_to_string(&path).expect("read updated source");
        let reparsed = toml::from_str::<HelperConfig>(&saved).expect("parse updated source");
        assert_eq!(reparsed.ui.language.as_deref(), Some("zh"));
        assert_eq!(
            reparsed
                .codex
                .routing
                .as_ref()
                .and_then(RouteGraphConfig::entry_node)
                .map(|node| node.children.as_slice()),
            Some(["relay".to_string()].as_slice())
        );
    });
}
