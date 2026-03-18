use super::*;

#[test]
fn compile_v2_to_runtime_orders_preferred_members() {
    let mut openai_endpoints = BTreeMap::new();
    openai_endpoints.insert(
        "hk".to_string(),
        ProviderEndpointV2 {
            base_url: "https://hk.example.com/v1".to_string(),
            enabled: true,
            priority: 0,
            tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        },
    );
    openai_endpoints.insert(
        "us".to_string(),
        ProviderEndpointV2 {
            base_url: "https://us.example.com/v1".to_string(),
            enabled: true,
            priority: 1,
            tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        },
    );

    let mut backup_endpoints = BTreeMap::new();
    backup_endpoints.insert(
        "default".to_string(),
        ProviderEndpointV2 {
            base_url: "https://backup.example.com/v1".to_string(),
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        },
    );

    let mut providers = BTreeMap::new();
    providers.insert(
        "openai".to_string(),
        ProviderConfigV2 {
            alias: Some("OpenAI".to_string()),
            enabled: true,
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("OPENAI_API_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: BTreeMap::from([("provider_id".to_string(), "openai".to_string())]),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            endpoints: openai_endpoints,
        },
    );
    providers.insert(
        "backup".to_string(),
        ProviderConfigV2 {
            alias: Some("Backup".to_string()),
            enabled: true,
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            endpoints: backup_endpoints,
        },
    );

    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("primary".to_string()),
            default_profile: None,
            profiles: BTreeMap::new(),
            providers,
            groups: BTreeMap::from([(
                "primary".to_string(),
                GroupConfigV2 {
                    alias: Some("Primary".to_string()),
                    enabled: true,
                    level: 1,
                    members: vec![
                        GroupMemberRefV2 {
                            provider: "backup".to_string(),
                            endpoint_names: vec!["default".to_string()],
                            preferred: false,
                        },
                        GroupMemberRefV2 {
                            provider: "openai".to_string(),
                            endpoint_names: vec!["hk".to_string(), "us".to_string()],
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

    let runtime = compile_v2_to_runtime(&v2).expect("compile_v2_to_runtime");
    let svc = runtime
        .codex
        .configs
        .get("primary")
        .expect("compiled primary group");

    assert_eq!(svc.upstreams.len(), 3);
    assert_eq!(svc.upstreams[0].base_url, "https://hk.example.com/v1");
    assert_eq!(svc.upstreams[1].base_url, "https://us.example.com/v1");
    assert_eq!(svc.upstreams[2].base_url, "https://backup.example.com/v1");
    assert_eq!(
        svc.upstreams[0].auth.auth_token_env.as_deref(),
        Some("OPENAI_API_KEY")
    );
    assert_eq!(
        svc.upstreams[0].tags.get("provider_id").map(|s| s.as_str()),
        Some("openai")
    );
    assert_eq!(
        svc.upstreams[0].tags.get("region").map(|s| s.as_str()),
        Some("hk")
    );
}

#[test]
fn compile_v2_to_runtime_orders_provider_endpoints_by_priority() {
    let mut endpoints = BTreeMap::new();
    endpoints.insert(
        "aaa".to_string(),
        ProviderEndpointV2 {
            base_url: "https://backup.example.com/v1".to_string(),
            enabled: true,
            priority: 10,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        },
    );
    endpoints.insert(
        "zzz".to_string(),
        ProviderEndpointV2 {
            base_url: "https://primary.example.com/v1".to_string(),
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        },
    );

    let v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2 {
            active_group: Some("primary".to_string()),
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfigV2 {
                    alias: None,
                    enabled: true,
                    auth: UpstreamAuth::default(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    endpoints,
                },
            )]),
            groups: BTreeMap::from([(
                "primary".to_string(),
                GroupConfigV2 {
                    alias: None,
                    enabled: true,
                    level: 1,
                    members: vec![GroupMemberRefV2 {
                        provider: "relay".to_string(),
                        endpoint_names: Vec::new(),
                        preferred: true,
                    }],
                },
            )]),
        },
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: Some(ServiceKind::Codex),
        ui: UiConfig::default(),
    };

    let runtime = compile_v2_to_runtime(&v2).expect("compile_v2_to_runtime");
    let svc = runtime
        .codex
        .configs
        .get("primary")
        .expect("compiled primary group");

    assert_eq!(svc.upstreams.len(), 2);
    assert_eq!(svc.upstreams[0].base_url, "https://primary.example.com/v1");
    assert_eq!(svc.upstreams[1].base_url, "https://backup.example.com/v1");
}

#[test]
fn migrate_legacy_to_v2_creates_provider_per_upstream() {
    let mut legacy = ProxyConfig::default();
    legacy.codex.active = Some("team".to_string());
    legacy.codex.configs.insert(
        "team".to_string(),
        ServiceConfig {
            name: "team".to_string(),
            alias: Some("Team".to_string()),
            enabled: false,
            level: 3,
            upstreams: vec![
                UpstreamConfig {
                    base_url: "https://one.example.com/v1".to_string(),
                    auth: UpstreamAuth {
                        auth_token: None,
                        auth_token_env: Some("ONE_KEY".to_string()),
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: HashMap::from([("provider_id".to_string(), "one".to_string())]),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
                UpstreamConfig {
                    base_url: "https://two.example.com/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
            ],
        },
    );

    let migrated = migrate_legacy_to_v2(&legacy);
    assert_eq!(migrated.version, 2);
    assert_eq!(migrated.codex.active_group.as_deref(), Some("team"));

    let group = migrated
        .codex
        .groups
        .get("team")
        .expect("team group should exist");
    assert_eq!(group.alias.as_deref(), Some("Team"));
    assert!(!group.enabled);
    assert_eq!(group.level, 3);
    assert_eq!(group.members.len(), 2);
    assert_eq!(group.members[0].provider, "team__u01");
    assert_eq!(group.members[1].provider, "team__u02");

    let provider = migrated
        .codex
        .providers
        .get("team__u01")
        .expect("team__u01 provider should exist");
    assert_eq!(provider.alias.as_deref(), Some("one"));
    assert_eq!(provider.auth.auth_token_env.as_deref(), Some("ONE_KEY"));
    assert_eq!(
        provider
            .endpoints
            .get("default")
            .expect("default endpoint")
            .base_url,
        "https://one.example.com/v1"
    );
}

#[test]
fn compact_v2_config_merges_same_provider_endpoints() {
    let mut legacy = ProxyConfig::default();
    legacy.codex.active = Some("team".to_string());
    legacy.codex.configs.insert(
        "team".to_string(),
        ServiceConfig {
            name: "team".to_string(),
            alias: Some("Team".to_string()),
            enabled: true,
            level: 1,
            upstreams: vec![
                UpstreamConfig {
                    base_url: "https://hk.example.com/v1".to_string(),
                    auth: UpstreamAuth {
                        auth_token: None,
                        auth_token_env: Some("OPENAI_API_KEY".to_string()),
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: HashMap::from([
                        ("provider_id".to_string(), "openai".to_string()),
                        ("region".to_string(), "hk".to_string()),
                    ]),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
                UpstreamConfig {
                    base_url: "https://us.example.com/v1".to_string(),
                    auth: UpstreamAuth {
                        auth_token: None,
                        auth_token_env: Some("OPENAI_API_KEY".to_string()),
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: HashMap::from([
                        ("provider_id".to_string(), "openai".to_string()),
                        ("region".to_string(), "us".to_string()),
                    ]),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
            ],
        },
    );

    let migrated = migrate_legacy_to_v2(&legacy);
    let compact = compact_v2_config(&migrated).expect("compact_v2_config");

    assert_eq!(compact.codex.providers.len(), 1);
    let provider = compact
        .codex
        .providers
        .get("openai")
        .expect("openai provider should exist");
    assert_eq!(
        provider.auth.auth_token_env.as_deref(),
        Some("OPENAI_API_KEY")
    );
    assert_eq!(
        provider.tags.get("provider_id").map(|s| s.as_str()),
        Some("openai")
    );
    assert_eq!(provider.endpoints.len(), 2);
    assert!(provider.endpoints.contains_key("hk"));
    assert!(provider.endpoints.contains_key("us"));

    let group = compact
        .codex
        .groups
        .get("team")
        .expect("team group should exist");
    assert_eq!(group.members.len(), 1);
    assert_eq!(group.members[0].provider, "openai");
    assert_eq!(
        group.members[0].endpoint_names,
        vec!["hk".to_string(), "us".to_string()]
    );
}

#[test]
fn compact_v2_config_preserves_explicit_provider_alias() {
    let mut v2 = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2::default(),
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: NotifyConfig::default(),
        default_service: None,
        ui: UiConfig::default(),
    };
    v2.codex.providers.insert(
        "alpha".to_string(),
        ProviderConfigV2 {
            alias: Some("Relay Alpha".to_string()),
            enabled: true,
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("ALPHA_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: BTreeMap::from([("provider_id".to_string(), "alpha".to_string())]),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            endpoints: [(
                "default".to_string(),
                ProviderEndpointV2 {
                    base_url: "https://alpha.example.com/v1".to_string(),
                    enabled: true,
                    priority: 0,
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        },
    );

    let compact = compact_v2_config(&v2).expect("compact_v2_config");
    let provider = compact
        .codex
        .providers
        .get("alpha")
        .expect("alpha provider should exist");
    assert_eq!(provider.alias.as_deref(), Some("Relay Alpha"));
}

#[test]
fn load_config_supports_v2_schema() {
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
version = 2

[codex]
active_station = "primary"
default_profile = "daily"

[codex.profiles.daily]
station = "primary"
reasoning_effort = "medium"
service_tier = "priority"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.tags]
provider_id = "openai"
[codex.providers.openai.endpoints.hk]
base_url = "https://hk.example.com/v1"
[codex.providers.openai.endpoints.hk.tags]
region = "hk"
[codex.providers.openai.endpoints.us]
base_url = "https://us.example.com/v1"

[codex.stations.primary]
level = 2

[[codex.stations.primary.members]]
provider = "openai"
endpoint_names = ["us"]
preferred = true
"#,
        );

        let cfg = super::load_config().await.expect("load v2 config");
        assert_eq!(cfg.version, Some(2));
        assert_eq!(cfg.codex.active.as_deref(), Some("primary"));
        assert_eq!(cfg.codex.default_profile.as_deref(), Some("daily"));
        assert_eq!(
            cfg.codex
                .profiles
                .get("daily")
                .and_then(|profile| profile.station.as_deref()),
            Some("primary")
        );

        let svc = cfg
            .codex
            .configs
            .get("primary")
            .expect("primary config should exist");
        assert_eq!(svc.level, 2);
        assert_eq!(svc.upstreams.len(), 1);
        assert_eq!(svc.upstreams[0].base_url, "https://us.example.com/v1");
        assert_eq!(
            svc.upstreams[0].auth.auth_token_env.as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            svc.upstreams[0].tags.get("provider_id").map(|s| s.as_str()),
            Some("openai")
        );
    });
}

#[test]
fn load_config_supports_profile_extends() {
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
version = 2

[codex]
default_profile = "fast"

[codex.profiles.base]
station = "primary"
model = "gpt-5.4"
service_tier = "priority"

[codex.profiles.fast]
extends = "base"
reasoning_effort = "low"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.endpoints.default]
base_url = "https://api.example.com/v1"

[codex.stations.primary]
level = 1

[[codex.stations.primary.members]]
provider = "openai"
endpoint_names = ["default"]
"#,
        );

        let cfg = super::load_config().await.expect("load inherited profiles");
        let fast = cfg.codex.profiles.get("fast").expect("fast profile");
        assert_eq!(fast.extends.as_deref(), Some("base"));
        assert_eq!(fast.reasoning_effort.as_deref(), Some("low"));

        let resolved =
            super::resolve_service_profile(&cfg.codex, "fast").expect("resolve fast profile");
        assert_eq!(resolved.station.as_deref(), Some("primary"));
        assert_eq!(resolved.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(resolved.service_tier.as_deref(), Some("priority"));
    });
}

#[test]
fn load_config_rejects_profile_extends_cycle() {
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
version = 2

[codex.profiles.alpha]
extends = "beta"

[codex.profiles.beta]
extends = "alpha"
"#,
        );

        let err = super::load_config().await.expect_err("load should fail");
        assert!(err.to_string().contains("profile inheritance cycle"));
        assert!(err.to_string().contains("alpha -> beta -> alpha"));
    });
}

#[test]
fn save_config_after_loading_v2_preserves_v2_schema() {
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
version = 2

[codex]
active_station = "primary"
default_profile = "daily"

[codex.profiles.daily]
station = "primary"
service_tier = "priority"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.endpoints.default]
base_url = "https://api.example.com/v1"

[codex.stations.primary]
level = 1

[[codex.stations.primary.members]]
provider = "openai"
endpoint_names = ["default"]
"#,
        );

        let cfg = super::load_config().await.expect("load v2 config");
        assert_eq!(cfg.version, Some(2));

        super::save_config(&cfg).await.expect("save v2 config");
        let saved = std::fs::read_to_string(&toml_path).expect("read saved config.toml");
        assert!(saved.contains("version = 2"));
        assert!(saved.contains("active_station = \"primary\""));
        assert!(saved.contains("[codex.stations.primary]"));
        assert!(saved.contains("[codex.providers.openai]"));
        assert!(saved.contains("default_profile = \"daily\""));
        assert!(saved.contains("[codex.profiles.daily]"));
        assert!(saved.contains("service_tier = \"priority\""));
        assert!(!saved.contains("[codex.configs.primary]"));
    });
}

#[test]
fn load_config_supports_v2_group_aliases() {
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
version = 2

[codex]
active_group = "legacy"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.endpoints.default]
base_url = "https://api.example.com/v1"

[codex.groups.legacy]
level = 1

[[codex.groups.legacy.members]]
provider = "openai"
"#,
        );

        let cfg = super::load_config()
            .await
            .expect("load legacy-named v2 config");
        assert_eq!(cfg.version, Some(2));
        assert_eq!(cfg.codex.active.as_deref(), Some("legacy"));
        assert!(cfg.codex.configs.contains_key("legacy"));
    });
}

#[test]
fn load_config_migrates_boolish_active_true_to_first_enabled_config() {
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
version = 1

[codex]
active = "true"

[codex.configs.right]
name = "right"
enabled = true
level = 1

[[codex.configs.right.upstreams]]
base_url = "https://right.example.com/v1"

[codex.configs.vibe]
name = "vibe"
enabled = true
level = 2

[[codex.configs.vibe.upstreams]]
base_url = "https://vibe.example.com/v1"
"#,
        );

        let cfg = super::load_config()
            .await
            .expect("load boolish active config");
        assert_eq!(cfg.codex.active.as_deref(), Some("right"));
    });
}

#[test]
fn load_config_migrates_boolish_active_false_to_none() {
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
version = 1

[codex]
active = "false"

[codex.configs.right]
name = "right"
enabled = true
level = 1

[[codex.configs.right.upstreams]]
base_url = "https://right.example.com/v1"
"#,
        );

        let cfg = super::load_config()
            .await
            .expect("load boolish inactive config");
        assert_eq!(cfg.codex.active, None);
    });
}

#[test]
fn load_config_rejects_invalid_default_profile() {
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
version = 1

[codex]
active = "primary"
default_profile = "missing"

[codex.configs.primary]
name = "primary"

[[codex.configs.primary.upstreams]]
base_url = "https://api.example.com/v1"
"#,
        );

        let err = super::load_config().await.expect_err("load should fail");
        assert!(
            err.to_string().contains("default_profile"),
            "unexpected error: {err}"
        );
    });
}

#[test]
fn load_config_rejects_profile_model_incompatible_with_station_capabilities() {
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
version = 1

[codex]
active = "primary"
default_profile = "fast"

[codex.profiles.fast]
station = "primary"
model = "gpt-4.1"

[codex.configs.primary]
name = "primary"

[[codex.configs.primary.upstreams]]
base_url = "https://api.example.com/v1"
supported_models = { "gpt-5.4" = true }
"#,
        );

        let err = super::load_config().await.expect_err("load should fail");
        assert!(
            err.to_string().contains("not supported"),
            "unexpected error: {err}"
        );
    });
}

#[test]
fn save_config_v2_writes_v2_schema_and_backup() {
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
version = 1

[codex]
active = "legacy"

[codex.configs.legacy]
name = "legacy"
level = 1

[[codex.configs.legacy.upstreams]]
base_url = "https://legacy.example.com/v1"
"#,
        );

        let legacy = super::load_config().await.expect("load legacy config");
        let migrated = migrate_legacy_to_v2(&legacy);
        let written_path = super::save_config_v2(&migrated)
            .await
            .expect("save_config_v2 should succeed");

        assert_eq!(written_path, toml_path);
        let saved = std::fs::read_to_string(&toml_path).expect("read v2 config.toml");
        assert!(saved.contains("version = 2"));
        assert!(saved.contains("[codex.stations.legacy]"));
        assert!(saved.contains("base_url = \"https://legacy.example.com/v1\""));

        let backup = std::fs::read_to_string(&backup_path).expect("read config.toml.bak");
        assert!(backup.contains("version = 1"));
        assert!(backup.contains("[codex.configs.legacy]"));
    });
}
