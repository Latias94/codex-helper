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
        ("version 6", "version = 6\n[notify]\nenabled = true\n"),
    ];

    for (label, text) in cases {
        assert_load_rejected_without_modification(&runtime, &path, label, text);
    }
}

#[test]
fn legacy_json_config_is_ignored_without_import_or_modification() {
    let _env = setup_temp_codex_home();
    let toml_path = current_config_path();
    let json_path = proxy_home_dir().join("config.json");
    let json = r#"{"version":5,"notify":{"enabled":true}}"#;
    write_file(&json_path, json);

    let config = test_runtime()
        .block_on(load_config())
        .expect("config.json must be ignored");

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    assert!(!config.notify.enabled);
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read unchanged config.json"),
        json
    );
    assert!(!toml_path.exists());
    assert!(!proxy_home_dir().join("config.json.bak").exists());
    assert!(!proxy_home_dir().join("config.toml.bak").exists());
}
