use super::*;

#[test]
fn load_config_prefers_toml_over_json() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let json_path = dir.join("config.json");
        let toml_path = dir.join("config.toml");

        // JSON sets notify.enabled=false
        write_file(&json_path, r#"{"version":1,"notify":{"enabled":false}}"#);

        // TOML v5 is authoritative; config.json is ignored.
        write_file(
            &toml_path,
            r#"
version = 5

[notify]
enabled = true
"#,
        );

        let cfg = super::load_config().await.expect("load_config");
        assert!(
            cfg.notify.enabled,
            "expected config.toml to take precedence over config.json (home={:?})",
            home
        );
    });
}

#[test]
fn load_config_rejects_non_current_version() {
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
active = "right"

[codex.configs.right]
# name omitted on purpose

[[codex.configs.right.upstreams]]
base_url = "https://www.right.codes/codex/v1"
[codex.configs.right.upstreams.auth]
auth_token_env = "RIGHTCODE_API_KEY"
"#,
        );

        let err = super::load_config()
            .await
            .expect_err("non-current TOML should be rejected");
        assert_unsupported_config(&err, "version 1");
    });
}

#[test]
fn load_config_rejects_unversioned_toml_without_modification() {
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
[codex]
active = "right"

[codex.configs.right]

[[codex.configs.right.upstreams]]
base_url = "https://www.right.codes/codex/v1"
[codex.configs.right.upstreams.auth]
auth_token_env = "RIGHTCODE_API_KEY"
"#,
        );

        let err = super::load_config()
            .await
            .expect_err("unversioned config should be rejected");
        assert_unsupported_config(&err, "missing or invalid");

        let original = std::fs::read_to_string(&toml_path).expect("read unchanged config.toml");
        assert!(original.contains("[codex.configs.right]"));
        assert!(!original.contains("version = 5"));
    });
}

#[test]
fn save_helper_config_overwrites_existing_toml_and_updates_backup() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        let backup_path = dir.join("config.toml.bak");

        let mut cfg = HelperConfig::default();
        cfg.notify.enabled = true;
        super::save_helper_config(&cfg)
            .await
            .expect("first save_helper_config");

        let first_text = std::fs::read_to_string(&toml_path).expect("read first config.toml");
        assert!(first_text.contains("enabled = true"));
        assert!(
            !backup_path.exists(),
            "first save should not create backup without an existing file"
        );

        cfg.notify.enabled = false;
        super::save_helper_config(&cfg)
            .await
            .expect("second save_helper_config");

        let second_text = std::fs::read_to_string(&toml_path).expect("read second config.toml");
        assert!(second_text.contains("enabled = false"));

        let backup_text = std::fs::read_to_string(&backup_path).expect("read config.toml.bak");
        assert!(
            backup_text.contains("enabled = true"),
            "backup should preserve the previous config contents"
        );
    });
}

#[test]
fn init_config_toml_does_not_import_or_modify_codex_files() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();

    let codex_config = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
    let codex_auth = r#"{ "RIGHTCODE_API_KEY": "test-only" }"#;
    let codex_config_path = home.join("config.toml");
    let codex_auth_path = home.join("auth.json");
    write_file(&codex_config_path, codex_config);
    write_file(&codex_auth_path, codex_auth);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let path = super::init_config_toml(true)
            .await
            .expect("init_config_toml");
        let text = std::fs::read_to_string(&path).expect("read config.toml");
        assert!(text.contains("version = 5"), "expected v5 template");
        assert!(
            !text.contains("\n[codex.providers.right]\n"),
            "config init must not import Codex providers"
        );
        assert!(
            !text.contains("\n[codex.routing]\n"),
            "config init must not activate imported Codex routing"
        );
        assert_eq!(
            std::fs::read_to_string(&codex_config_path).expect("read Codex config"),
            codex_config
        );
        assert_eq!(
            std::fs::read_to_string(&codex_auth_path).expect("read Codex auth"),
            codex_auth
        );
        assert!(
            text.contains("\n[retry]\n") && text.contains("profile = \"balanced\""),
            "expected retry.profile default to be visible"
        );
    });
}
