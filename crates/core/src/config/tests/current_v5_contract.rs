use super::*;
use std::path::Path;

const COMPLEX_VERSION_5_CONFIG: &str = r#"# Current codex-helper route graph contract.
version = 5
default_service = "codex"

[codex]
default_profile = "daily"

[codex.profiles.daily]
model = "gpt-5.4"
reasoning_effort = "medium"
service_tier = "priority"

[codex.providers.monthly]
base_url = "https://monthly.example.com/v1"
continuity_domain = "monthly-account"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.regional]
auth_token_env = "REGIONAL_API_KEY"
tags = { billing = "monthly" }

[codex.providers.regional.endpoints.fast]
base_url = "https://regional-fast.example.com/v1"
continuity_domain = "regional-account"
priority = 0
tags = { region = "hk" }

[codex.providers.regional.endpoints.slow]
base_url = "https://regional-slow.example.com/v1"
continuity_domain = "regional-account"
priority = 10
tags = { region = "us" }

[codex.providers.paygo]
base_url = "https://paygo.example.com/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo", region = "us" }

[codex.routing]
entry = "main"
affinity_policy = "fallback-sticky"
scheduling_preset = "balanced"
fallback_ttl_ms = 90000
reprobe_preferred_after_ms = 30000

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly", "regional"]

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["monthly_pool", "paygo"]

[retry]
profile = "balanced"

[notify]
enabled = false
"#;

const V0_20_3_TYPICAL_CONFIG: &str = include_str!("fixtures/v0.20.3-typical.toml");

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn current_config_path() -> PathBuf {
    proxy_home_dir().join("config.toml")
}

fn assert_load_rejected_without_modification(
    runtime: &tokio::runtime::Runtime,
    path: &Path,
    label: &str,
    text: &str,
) {
    write_file(path, text);

    let error = runtime.block_on(load_config()).expect_err(label);

    assert!(
        error
            .to_string()
            .contains("normal startup only accepts version = 5"),
        "unexpected {label} rejection: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("read rejected config"),
        text,
        "rejecting {label} must not modify the source file"
    );
    assert!(
        !path.with_file_name("config.toml.bak").exists(),
        "rejecting {label} must not create a backup"
    );
}

fn assert_retired_config_rejected_without_modification(
    runtime: &tokio::runtime::Runtime,
    path: &Path,
    retired_path: &str,
    text: &str,
) {
    write_file(path, text);

    let error = runtime
        .block_on(load_config())
        .expect_err("retired version 5 input must be rejected");
    let message = error.to_string();
    assert!(
        message.contains(retired_path),
        "rejection must identify {retired_path}: {message}"
    );
    assert!(
        message.contains("has been removed") && message.contains("Remove"),
        "rejection must explain the migration action: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("read rejected config"),
        text,
        "rejecting {retired_path} must not modify the source file"
    );
    assert!(
        !path.with_file_name("config.toml.bak").exists(),
        "rejecting {retired_path} must not create a backup"
    );
}

#[test]
fn current_config_path_is_fixed_to_helper_home_config_toml() {
    let _env = setup_temp_codex_home();
    let expected = current_config_path();
    let legacy_json_path = proxy_home_dir().join("config.json");

    write_file(&legacy_json_path, r#"{"version":4}"#);

    assert_eq!(config_file_path(), expected);

    write_file(&expected, COMPLEX_VERSION_5_CONFIG);
    assert_eq!(config_file_path(), expected);
}

#[test]
fn complex_version_5_route_graph_loads_from_the_current_contract() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    write_file(&path, COMPLEX_VERSION_5_CONFIG);

    let config = test_runtime()
        .block_on(load_config())
        .expect("load complex version 5 config");

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    assert_eq!(config.default_service, Some(ServiceKind::Codex));
    assert_eq!(config.codex.default_profile.as_deref(), Some("daily"));
    assert_eq!(config.codex.providers.len(), 3);

    let loaded = test_runtime()
        .block_on(load_config_with_source())
        .expect("load version 5 source contract");
    assert_eq!(
        loaded
            .source
            .codex
            .routing
            .as_ref()
            .map(|routing| routing.scheduling_preset),
        Some(SchedulingPreset::Balanced)
    );
    assert!(!loaded.source.codex.providers.is_empty());

    let route = crate::routing_ir::compile_route_handshake_plan("codex", &loaded.source.codex)
        .expect("compile canonical route graph");
    let base_urls = route
        .candidates
        .iter()
        .map(|candidate| candidate.base_url.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        base_urls,
        vec![
            "https://monthly.example.com/v1",
            "https://regional-fast.example.com/v1",
            "https://regional-slow.example.com/v1",
            "https://paygo.example.com/v1",
        ]
    );
    assert_eq!(
        route.candidates[0].auth.auth_token_env.as_deref(),
        Some("MONTHLY_API_KEY")
    );
    assert_eq!(
        route.candidates[1].auth.auth_token_env.as_deref(),
        Some("REGIONAL_API_KEY")
    );
    assert_eq!(route.candidates[1].endpoint_id, "fast");
    assert_eq!(
        route
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.provider_id.as_str(),
                    candidate.endpoint_id.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("monthly", "default"),
            ("regional", "fast"),
            ("regional", "slow"),
            ("paygo", "default"),
        ]
    );
}

#[test]
fn v0_20_3_supported_version_5_fixture_loads_without_rewrite() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    write_file(&path, V0_20_3_TYPICAL_CONFIG);
    let original = std::fs::read(&path).expect("read v0.20.3 fixture bytes");

    let loaded = test_runtime()
        .block_on(load_config_with_source())
        .expect("load supported v0.20.3 version 5 fixture");
    let config = loaded.source;

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    assert_eq!(config.default_service, Some(ServiceKind::Codex));
    assert_eq!(config.codex.default_profile.as_deref(), Some("daily"));
    assert_eq!(
        config.codex.profiles["base"].reasoning_effort.as_deref(),
        Some("medium")
    );
    assert_eq!(
        config.codex.profiles["daily"].extends.as_deref(),
        Some("base")
    );
    assert_eq!(
        config.codex.profiles["daily"].model.as_deref(),
        Some("gpt-5.4")
    );
    assert_eq!(
        config.codex.profiles["daily"].service_tier.as_deref(),
        Some("priority")
    );
    let primary = &config.codex.providers["primary"];
    assert_eq!(primary.alias.as_deref(), Some("Primary relay"));
    assert_eq!(
        primary.base_url.as_deref(),
        Some("https://primary.example.com/v1")
    );
    assert_eq!(
        primary.continuity_domain.as_deref(),
        Some("primary-account")
    );
    assert_eq!(
        primary.inline_auth.auth_token_env.as_deref(),
        Some("PRIMARY_API_KEY")
    );
    assert_eq!(
        primary.tags.get("billing").map(String::as_str),
        Some("monthly")
    );
    assert_eq!(primary.tags.get("region").map(String::as_str), Some("hk"));
    assert_eq!(primary.supported_models.get("gpt-5.4"), Some(&true));
    assert_eq!(
        primary.model_mapping.get("gpt-5.4").map(String::as_str),
        Some("openai/gpt-5.4")
    );
    assert_eq!(primary.limits.max_concurrent_requests, Some(2));
    assert_eq!(
        primary.limits.limit_group.as_deref(),
        Some("primary-account")
    );
    let regional = &config.codex.providers["regional"];
    assert_eq!(
        regional.inline_auth.auth_token_env.as_deref(),
        Some("REGIONAL_API_KEY")
    );
    assert_eq!(
        regional.tags.get("billing").map(String::as_str),
        Some("paygo")
    );
    let fast = &regional.endpoints["fast"];
    assert_eq!(fast.base_url.as_str(), "https://regional.example.com/v1");
    assert_eq!(fast.continuity_domain.as_deref(), Some("regional-account"));
    assert_eq!(fast.priority, 5);
    assert_eq!(fast.supported_models.get("gpt-5.4"), Some(&true));

    let codex_routing = config.codex.routing.as_ref().expect("Codex routing");
    assert_eq!(codex_routing.entry, "main");
    assert_eq!(
        codex_routing.affinity_policy,
        RouteAffinityPolicy::FallbackSticky
    );
    assert_eq!(codex_routing.fallback_ttl_ms, Some(90_000));
    assert_eq!(codex_routing.reprobe_preferred_after_ms, Some(30_000));
    assert_eq!(
        codex_routing.routes["main"].strategy,
        RouteStrategy::OrderedFailover
    );
    assert_eq!(
        codex_routing.routes["main"].children,
        vec!["primary", "regional"]
    );

    assert_eq!(config.claude.default_profile.as_deref(), Some("daily"));
    assert_eq!(
        config.claude.profiles["daily"].model.as_deref(),
        Some("claude-sonnet-4")
    );
    assert_eq!(
        config.claude.providers["primary"].base_url.as_deref(),
        Some("https://claude.example.com/v1")
    );
    assert_eq!(
        config.claude.providers["primary"]
            .inline_auth
            .api_key_env
            .as_deref(),
        Some("CLAUDE_RELAY_API_KEY")
    );
    let claude_routing = config.claude.routing.as_ref().expect("Claude routing");
    assert_eq!(
        claude_routing.routes["main"].strategy,
        RouteStrategy::ManualSticky
    );
    assert_eq!(
        claude_routing.routes["main"].target.as_deref(),
        Some("primary")
    );

    assert_eq!(config.retry.profile, Some(RetryProfileName::CostPrimary));
    assert_eq!(config.retry.never_on_status.as_deref(), Some("413,415,422"));
    assert_eq!(config.retry.transport_cooldown_secs, Some(45));
    assert_eq!(config.retry.cooldown_backoff_factor, Some(2));
    assert_eq!(config.retry.cooldown_backoff_max_secs, Some(900));
    assert_eq!(
        config
            .retry
            .upstream
            .as_ref()
            .and_then(|layer| layer.max_attempts),
        Some(3)
    );
    assert_eq!(
        config
            .retry
            .upstream
            .as_ref()
            .and_then(|layer| layer.strategy),
        Some(RetryStrategy::SameUpstream)
    );
    assert_eq!(
        config
            .retry
            .provider
            .as_ref()
            .and_then(|layer| layer.max_attempts),
        Some(2)
    );
    assert_eq!(
        config
            .retry
            .provider
            .as_ref()
            .and_then(|layer| layer.strategy),
        Some(RetryStrategy::Failover)
    );

    assert!(config.notify.enabled);
    assert_eq!(config.notify.policy.min_duration_ms, 30_000);
    assert_eq!(config.notify.policy.global_cooldown_ms, 60_000);
    assert_eq!(config.notify.policy.merge_window_ms, 10_000);
    assert_eq!(config.notify.policy.per_thread_cooldown_ms, 180_000);
    assert_eq!(config.notify.policy.recent_search_window_ms, 300_000);
    assert_eq!(config.notify.policy.recent_endpoint_timeout_ms, 500);

    assert_eq!(
        config.relay_targets["nas"].service,
        Some(ServiceKind::Codex)
    );
    assert_eq!(
        config.relay_targets["nas"].proxy_url,
        "http://nas.local:3211"
    );
    assert_eq!(
        config.relay_targets["nas"].admin_url.as_deref(),
        Some("https://nas.example.com:4211")
    );
    assert_eq!(
        config.relay_targets["nas"].admin_token_env.as_deref(),
        Some("CODEX_HELPER_NAS_ADMIN_TOKEN")
    );
    assert_eq!(config.fleet.nodes["local"].label.as_deref(), Some("Local"));
    assert_eq!(
        config.fleet.nodes["local"].admin_url.as_deref(),
        Some("http://127.0.0.1:4211")
    );
    assert!(config.fleet.nodes["local"].enabled);
    assert_eq!(config.ui.language.as_deref(), Some("zh"));
    let service_status = &config.ui.service_status;
    assert!(service_status.enabled);
    assert_eq!(service_status.refresh_interval_secs, 120);
    assert_eq!(service_status.timeout_ms, 2_500);
    assert_eq!(service_status.high_latency_ms, 2_000);
    assert_eq!(service_status.history_cells, 30);
    assert_eq!(service_status.probes.len(), 1);
    assert_eq!(service_status.probes[0].id.as_deref(), Some("primary"));
    assert_eq!(
        service_status.probes[0].provider.as_deref(),
        Some("primary")
    );
    assert_eq!(
        service_status.probes[0].endpoint.as_deref(),
        Some("default")
    );
    assert_eq!(service_status.probes[0].models, vec!["gpt-5.4"]);

    // This field did not exist in v0.20.3; the compatibility guide documents the new default.
    assert_eq!(
        config
            .codex
            .routing
            .as_ref()
            .map(|routing| routing.scheduling_preset),
        Some(SchedulingPreset::Balanced)
    );

    let route = crate::routing_ir::compile_route_handshake_plan("codex", &config.codex)
        .expect("compile v0.20.3 route graph");
    assert_eq!(
        route
            .candidates
            .iter()
            .map(|candidate| (
                candidate.provider_id.as_str(),
                candidate.endpoint_id.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![("primary", "default"), ("regional", "fast")]
    );
    assert_eq!(
        std::fs::read(&path).expect("read fixture after load"),
        original
    );
    assert!(!path.with_file_name("config.toml.bak").exists());
}

#[test]
fn loading_version_5_config_does_not_rewrite_original_bytes() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    write_file(&path, COMPLEX_VERSION_5_CONFIG);
    let original = std::fs::read(&path).expect("read original config bytes");

    test_runtime()
        .block_on(load_config())
        .expect("load current config without rewriting it");

    assert_eq!(
        std::fs::read(&path).expect("read config bytes after load"),
        original
    );
    assert!(!path.with_file_name("config.toml.bak").exists());
}

#[test]
fn config_init_force_backs_up_retired_v5_before_replacement() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let retired = "version = 5\n[ui.usage_forecast]\nenabled = false\n";
    write_file(&path, retired);

    test_runtime()
        .block_on(init_config_toml(true))
        .expect("force init is the explicit recovery path for retired settings");

    assert_eq!(
        std::fs::read_to_string(path.with_file_name("config.toml.bak"))
            .expect("read retired config backup"),
        retired
    );
    let replacement = std::fs::read_to_string(&path).expect("read replacement template");
    assert!(replacement.contains("version = 5"));
    assert!(!replacement.contains("[ui.usage_forecast]"));
}

#[test]
fn removed_codex_compaction_config_is_rejected_without_modification() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let runtime = test_runtime();
    let cases = [
        (
            "remote_v2_downgrade true",
            "version = 5\n[codex.compaction]\nremote_v2_downgrade = true\n",
        ),
        (
            "remote_v2_downgrade false",
            "version = 5\n[codex.compaction]\nremote_v2_downgrade = false\n",
        ),
        (
            "empty compaction table",
            "version = 5\n[codex.compaction]\n",
        ),
        (
            "non-table compaction value",
            "version = 5\n[codex]\ncompaction = \"retired\"\n",
        ),
    ];

    for (label, text) in cases {
        write_file(&path, text);
        let original = std::fs::read(&path).expect("read retired config bytes");

        let error = runtime.block_on(load_config()).expect_err(label);
        let message = error.to_string();
        assert!(
            message.contains("`[codex.compaction].remote_v2_downgrade` has been removed"),
            "unexpected {label} rejection: {error}"
        );
        assert!(
            message.contains("no longer performs remote compaction v2-to-v1 downgrade"),
            "unexpected {label} rejection: {error}"
        );
        assert!(
            message.contains("Delete the entire `[codex.compaction]` table"),
            "unexpected {label} rejection: {error}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read rejected config bytes"),
            original,
            "rejecting {label} must not modify the source file"
        );
        assert!(
            !path.with_file_name("config.toml.bak").exists(),
            "rejecting {label} must not create a backup"
        );
    }
}

#[test]
fn removed_claude_compaction_config_is_rejected_without_modification() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let text = "version = 5\n[claude.compaction]\nremote_v2_downgrade = false\n";
    write_file(&path, text);

    let error = test_runtime()
        .block_on(load_config())
        .expect_err("shared-schema Claude compaction must not be silently dropped");
    let message = error.to_string();
    assert!(message.contains("`claude.compaction`"));
    assert!(message.contains("had no Claude runtime effect"));
    assert!(message.contains("Delete the entire `[claude.compaction]` table"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read preserved config"),
        text
    );
    assert!(!path.with_file_name("config.toml.bak").exists());
}

#[test]
fn retired_version_5_inputs_are_rejected_before_they_can_be_silently_dropped() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let runtime = test_runtime();
    let cases = [
        (
            "codex.client_patch",
            "version = 5\n[codex.client_patch]\npreset = \"default\"\n",
        ),
        (
            "ui.usage_forecast",
            "version = 5\n[ui.usage_forecast]\nenabled = true\n",
        ),
        (
            "retry.allow_cross_station_before_first_output",
            "version = 5\n[retry]\nallow_cross_station_before_first_output = true\n",
        ),
        (
            "codex.profiles.daily.station",
            "version = 5\n[codex.profiles.daily]\nstation = \"primary\"\n",
        ),
        (
            "claude.profiles.deep.station",
            "version = 5\n[claude.profiles.deep]\nstation = \"backup\"\n",
        ),
        (
            "relay_targets.nas.client_preset",
            "version = 5\n[relay_targets.nas]\nproxy_url = \"http://nas.local:3211\"\nclient_preset = \"default\"\n",
        ),
        (
            "relay_targets.nas.responses_websocket",
            "version = 5\n[relay_targets.nas]\nproxy_url = \"http://nas.local:3211\"\nresponses_websocket = true\n",
        ),
    ];

    for (retired_path, text) in cases {
        assert_retired_config_rejected_without_modification(&runtime, &path, retired_path, text);
    }
}

#[test]
fn unsupported_toml_schemas_are_rejected_without_modification() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let runtime = test_runtime();
    let cases = [
        ("unversioned config", "[notify]\nenabled = true\n"),
        ("version 1", "version = 1\n[notify]\nenabled = true\n"),
        ("version 2", "version = 2\n[notify]\nenabled = true\n"),
        ("version 3", "version = 3\n[notify]\nenabled = true\n"),
        ("version 4", "version = 4\n[notify]\nenabled = true\n"),
        (
            "version 4 with retired-looking field",
            "version = 4\n[codex.client_patch]\npreset = \"default\"\n",
        ),
        ("version 6", "version = 6\n[notify]\nenabled = true\n"),
    ];

    for (label, text) in cases {
        assert_load_rejected_without_modification(&runtime, &path, label, text);
    }
}

#[test]
fn legacy_json_config_is_rejected_without_import_or_modification() {
    let _env = setup_temp_codex_home();
    let toml_path = current_config_path();
    let json_path = proxy_home_dir().join("config.json");
    let json = r#"{"version":5,"notify":{"enabled":true}}"#;
    write_file(&json_path, json);

    let error = test_runtime()
        .block_on(load_config())
        .expect_err("config.json must be rejected");

    let message = error.to_string();
    assert!(
        message.contains("config.json"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("unsupported legacy config source"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("normal startup only reads") && message.contains("version = 5"),
        "unexpected error: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read unchanged config.json"),
        json
    );
    assert!(!toml_path.exists());
    assert!(!proxy_home_dir().join("config.json.bak").exists());
    assert!(!proxy_home_dir().join("config.toml.bak").exists());

    let malformed = "not valid json";
    write_file(&json_path, malformed);
    let malformed_error = test_runtime()
        .block_on(load_config())
        .expect_err("malformed config.json must get the same legacy-source rejection");
    assert!(
        malformed_error
            .to_string()
            .contains("unsupported legacy config source")
    );
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read unchanged malformed config.json"),
        malformed
    );
}

#[test]
fn canonical_toml_takes_precedence_when_legacy_json_also_exists() {
    let _env = setup_temp_codex_home();
    let toml_path = current_config_path();
    let json_path = proxy_home_dir().join("config.json");
    write_file(&toml_path, COMPLEX_VERSION_5_CONFIG);
    write_file(&json_path, "not valid json");

    let config = test_runtime()
        .block_on(load_config())
        .expect("canonical TOML must remain authoritative");

    assert_eq!(config.codex.providers.len(), 3);
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read unchanged legacy JSON"),
        "not valid json"
    );
}

#[test]
fn retired_settings_are_reported_together_in_stable_path_order() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let text = r#"version = 5

[ui.usage_forecast]
enabled = false

[codex.client_patch]
preset = "default"

[relay_targets.nas]
proxy_url = "http://nas.local:3211"
responses_websocket = false
client_preset = "default"
"#;
    write_file(&path, text);

    let error = test_runtime()
        .block_on(load_config())
        .expect_err("all retired settings must be reported");
    let message = error.to_string();
    let expected = [
        "codex.client_patch",
        "relay_targets.nas.client_preset",
        "relay_targets.nas.responses_websocket",
        "ui.usage_forecast",
    ];
    let positions = expected
        .iter()
        .map(|path| {
            message
                .find(path)
                .unwrap_or_else(|| panic!("missing {path} in {message}"))
        })
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read unchanged config"),
        text
    );
}

#[test]
fn legal_provider_tag_names_do_not_trigger_retired_field_checks() {
    let _env = setup_temp_codex_home();
    let path = current_config_path();
    let text = r#"version = 5

[codex.providers.primary]
base_url = "https://primary.example/v1"
tags = { station = "hk", client_preset = "custom", responses_websocket = "supported" }

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["primary"]

"#;
    write_file(&path, text);

    let config = test_runtime()
        .block_on(load_config())
        .expect("only exact retired paths should be rejected");
    assert_eq!(
        config.codex.providers["primary"]
            .tags
            .get("station")
            .map(String::as_str),
        Some("hk")
    );
    assert_eq!(
        config.codex.providers["primary"]
            .tags
            .get("responses_websocket")
            .map(String::as_str),
        Some("supported")
    );
}

#[test]
fn typed_save_refuses_to_overwrite_legacy_or_retired_sources() {
    let _env = setup_temp_codex_home();
    let runtime = test_runtime();
    let toml_path = current_config_path();
    let json_path = proxy_home_dir().join("config.json");
    let replacement = HelperConfig::default();

    for (label, text, expected) in [
        (
            "retired v5",
            "version = 5\n[ui.usage_forecast]\nenabled = false\n",
            "ui.usage_forecast",
        ),
        (
            "older schema",
            "version = 4\n[notify]\nenabled = true\n",
            "unsupported config version 4",
        ),
    ] {
        write_file(&toml_path, text);
        let error = runtime
            .block_on(save_helper_config(&replacement))
            .expect_err("typed save must reject unsafe existing source");
        assert!(
            error.to_string().contains(expected),
            "unexpected {label} error: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&toml_path).expect("read preserved TOML"),
            text
        );
        assert!(!toml_path.with_file_name("config.toml.bak").exists());
    }

    std::fs::remove_file(&toml_path).expect("remove test TOML");
    let json = "not valid json";
    write_file(&json_path, json);
    let error = runtime
        .block_on(save_helper_config(&replacement))
        .expect_err("typed save must reject JSON-only source");
    assert!(
        error
            .to_string()
            .contains("unsupported legacy config source")
    );
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read preserved JSON"),
        json
    );
    assert!(!toml_path.exists());
}
