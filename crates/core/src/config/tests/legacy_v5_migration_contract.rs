use super::*;

const VERSION_5_CREDENTIAL_COMPAT: &str = include_str!("fixtures/version-5-credential-compat.toml");
const VERSION_6_DOWNGRADE_BOUNDARY: &str =
    include_str!("fixtures/version-6-downgrade-boundary.toml");

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime")
}

#[test]
fn version_5_legacy_auth_migrates_without_source_inference() {
    let _env = setup_temp_codex_home();
    let path = proxy_home_dir().join("config.toml");
    write_file(&path, VERSION_5_CREDENTIAL_COMPAT);
    let original = std::fs::read(&path).expect("read version 5 fixture");

    let loaded = test_runtime()
        .block_on(load_config_with_source())
        .expect("migrate version 5 credential fixture");

    assert_eq!(loaded.source.version, CURRENT_CONFIG_VERSION);
    let inline = loaded.source.codex.providers["inline"].effective_auth();
    assert_eq!(inline.auth_token.as_deref(), Some("fixture-inline-token"));
    assert_eq!(inline.auth_token_env.as_deref(), Some("FIXTURE_INLINE_ENV"));
    assert!(inline.auth_token_ref.is_none());

    let env = loaded.source.codex.providers["environment"].effective_auth();
    assert_eq!(env.auth_token_env.as_deref(), Some("FIXTURE_RELAY_TOKEN"));
    assert!(env.auth_token.is_none());
    assert!(env.auth_token_ref.is_none());

    let api_key = loaded.source.claude.providers["environment"].effective_auth();
    assert_eq!(api_key.api_key_env.as_deref(), Some("FIXTURE_CLAUDE_KEY"));
    assert!(api_key.api_key.is_none());
    assert!(api_key.api_key_ref.is_none());

    let migrated = std::fs::read_to_string(&path).expect("read migrated version 6 config");
    assert!(migrated.contains("version = 6"));
    assert!(!migrated.contains("auth_token_ref"));
    assert!(!migrated.contains("api_key_ref"));
    let backup_path = path.with_file_name("config.toml.bak");
    assert_eq!(
        std::fs::read(&backup_path).expect("read exact version 5 backup"),
        original
    );
}

#[test]
fn malformed_version_5_provider_fields_fail_without_publishing_migration() {
    let _env = setup_temp_codex_home();
    let path = proxy_home_dir().join("config.toml");
    let source = r#"version = 5
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token = ["not", "a", "string"]
"#;
    write_file(&path, source);

    let error = test_runtime()
        .block_on(load_config())
        .expect_err("invalid version 5 auth must fail before migration");

    let message = format!("{error:#}");
    assert!(
        message.contains("parse migrated configuration against the current v6 schema"),
        "unexpected: {message}"
    );
    assert!(
        message.contains("[codex.providers.relay]"),
        "unexpected: {message}"
    );
    assert!(
        message.contains("invalid type: sequence, expected a string"),
        "unexpected: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read unchanged invalid source"),
        source
    );
    assert!(!path.with_file_name("config.toml.bak").exists());
}

#[test]
fn version_5_input_cannot_activate_version_6_credential_references() {
    let _env = setup_temp_codex_home();
    let path = proxy_home_dir().join("config.toml");
    let cases = [
        r#"version = 5
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_ref = { source = "native", name = "relay.primary" }
"#,
        r#"version = 5
[claude.providers.relay]
base_url = "https://relay.example/v1"
[claude.providers.relay.auth]
api_key_ref = { source = "native", name = "relay.primary" }
"#,
        r#"version = 5
[codex]
active = "legacy"
[codex.configs.legacy]
[[codex.configs.legacy.upstreams]]
base_url = "https://relay.example/v1"
[codex.configs.legacy.upstreams.auth]
auth_token_ref = { source = "native", name = "relay.primary" }
"#,
        r#"version = 5
[codex]
active = "legacy"
[codex.configs.legacy]
[[codex.configs.legacy.upstreams]]
base_url = "https://relay.example/v1"
auth_token_ref = { source = "native", name = "relay.primary" }
[codex.configs.legacy.upstreams.auth]
auth_token_env = "RELAY_API_KEY"
"#,
    ];

    for source in cases {
        write_file(&path, source);
        let error = test_runtime()
            .block_on(load_config())
            .expect_err("version 5 credential reference must fail closed");
        let message = format!("{error:#}");
        assert!(
            message.contains("credential reference"),
            "unexpected: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read rejected source"),
            source
        );
        assert!(!path.with_file_name("config.toml.bak").exists());
    }
}

#[test]
fn version_5_non_auth_extension_keys_named_like_references_still_migrate() {
    let _env = setup_temp_codex_home();
    let path = proxy_home_dir().join("config.toml");
    let source = r#"version = 5

[ui.service_status]
enabled = true

[[ui.service_status.probes]]
id = "legacy-header"
url = "https://status.example/health"
models = ["gpt-5"]
headers = { auth_token_ref = "opaque-header-value" }
"#;
    write_file(&path, source);

    let loaded = test_runtime()
        .block_on(load_config())
        .expect("non-auth extension key must not be mistaken for a credential reference");

    assert!(loaded.ui.service_status.enabled);
    assert!(
        std::fs::read_to_string(&path)
            .expect("read migrated extension")
            .contains("auth_token_ref = \"opaque-header-value\"")
    );
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("config.toml.bak"))
            .expect("read exact version 5 extension backup"),
        source
    );
}

#[test]
fn downgrade_boundary_fixture_is_valid_canonical_version_6() {
    let config = toml::from_str::<HelperConfig>(VERSION_6_DOWNGRADE_BOUNDARY)
        .expect("parse version 6 downgrade fixture");
    validate_helper_config(&config).expect("validate version 6 downgrade fixture");

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    let auth = config.codex.providers["relay"].effective_auth();
    assert_eq!(auth.auth_token.as_deref(), Some("fixture-inline-token"));
    assert_eq!(auth.auth_token_env.as_deref(), Some("FIXTURE_INLINE_ENV"));
}
