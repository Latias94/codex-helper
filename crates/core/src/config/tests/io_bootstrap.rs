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

        // TOML overrides notify.enabled=true
        write_file(
            &toml_path,
            r#"
version = 1

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
fn load_config_toml_allows_missing_service_name_and_infers_from_key() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
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

        let cfg = super::load_config().await.expect("load_config");
        let svc = cfg
            .codex
            .configs
            .get("right")
            .expect("codex config 'right'");
        assert_eq!(
            svc.name, "right",
            "expected ServiceConfig.name to default to the map key (home={:?})",
            home
        );
    });
}

#[test]
fn save_config_overwrites_existing_toml_and_updates_backup() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        let backup_path = dir.join("config.toml.bak");

        let mut cfg = ProxyConfig::default();
        cfg.notify.enabled = true;
        super::save_config(&cfg).await.expect("first save_config");

        let first_text = std::fs::read_to_string(&toml_path).expect("read first config.toml");
        assert!(first_text.contains("enabled = true"));
        assert!(
            !backup_path.exists(),
            "first save should not create backup without an existing file"
        );

        cfg.notify.enabled = false;
        super::save_config(&cfg).await.expect("second save_config");

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
fn init_config_toml_inserts_codex_bootstrap_when_available() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();

    // Provide a minimal Codex config that bootstrap_from_codex can parse.
    write_file(
        &home.join("config.toml"),
        r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#,
    );
    write_file(
        &home.join("auth.json"),
        r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let path = super::init_config_toml(true, true)
            .await
            .expect("init_config_toml");
        let text = std::fs::read_to_string(&path).expect("read config.toml");
        assert!(text.contains("version = 2"), "expected v2 template");
        assert!(
            text.contains("\n[codex]\n"),
            "expected init to insert a real [codex] block (path={:?})",
            path
        );
        assert!(
            text.contains("active_station = \"right\""),
            "expected imported active station to be present"
        );
        assert!(
            text.contains("[codex.providers.right]"),
            "expected imported provider block to be present"
        );
        assert!(
            text.contains("[codex.stations.right]"),
            "expected imported station block to be present"
        );
        assert!(
            text.contains("\n[retry]\n") && text.contains("profile = \"balanced\""),
            "expected retry.profile default to be visible"
        );
    });
}

#[test]
fn init_config_toml_can_skip_codex_bootstrap_with_no_import() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();

    // Even if Codex config exists, no_import should not insert the real [codex] block.
    write_file(
        &home.join("config.toml"),
        r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#,
    );
    write_file(
        &home.join("auth.json"),
        r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let path = super::init_config_toml(true, false)
            .await
            .expect("init_config_toml");
        let text = std::fs::read_to_string(&path).expect("read config.toml");
        assert!(text.contains("version = 2"), "expected v2 template");
        assert!(
            !text.contains("\n[codex]\n"),
            "expected no_import to skip inserting a real [codex] block"
        );
        // But the template still contains the commented example.
        assert!(text.contains("# [codex]"));
    });
}

#[test]
fn bootstrap_from_codex_with_env_key_and_auth_json() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    // Write config.toml with explicit env_key
    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
    write_file(&cfg_path, config_text);

    // Write auth.json with matching RIGHTCODE_API_KEY
    let auth_path = home.join("auth.json");
    let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
    write_file(&auth_path, auth_text);

    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

    assert!(!cfg.codex.configs.is_empty());
    let svc = cfg.codex.active_station().expect("active codex station");
    assert_eq!(svc.name, "right");
    assert_eq!(svc.upstreams.len(), 1);
    let up = &svc.upstreams[0];
    assert_eq!(up.base_url, "https://www.right.codes/codex/v1");
    assert!(up.auth.auth_token.is_none());
    assert_eq!(up.auth.auth_token_env.as_deref(), Some("RIGHTCODE_API_KEY"));
}

#[test]
fn bootstrap_from_codex_infers_env_key_from_auth_json_when_missing() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    // config.toml without env_key
    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
"#;
    write_file(&cfg_path, config_text);

    // auth.json with a single *_API_KEY field
    let auth_path = home.join("auth.json");
    let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-456" }"#;
    write_file(&auth_path, auth_text);

    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should infer env_key");

    let svc = cfg.codex.active_station().expect("active codex station");
    assert_eq!(svc.name, "right");
    let up = &svc.upstreams[0];
    assert!(up.auth.auth_token.is_none());
    assert_eq!(up.auth.auth_token_env.as_deref(), Some("RIGHTCODE_API_KEY"));
}

#[test]
fn bootstrap_from_codex_fails_when_multiple_api_keys_without_env_key() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    // config.toml still without env_key
    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
"#;
    write_file(&cfg_path, config_text);

    // auth.json with multiple *_API_KEY fields
    let auth_path = home.join("auth.json");
    let auth_text = r#"
{
  "RIGHTCODE_API_KEY": "sk-test-1",
  "PACKYAPI_API_KEY": "sk-test-2"
}
"#;
    write_file(&auth_path, auth_text);

    let mut cfg = ProxyConfig::default();
    let err = bootstrap_from_codex(&mut cfg).expect_err("should fail to infer unique token");
    let msg = err.to_string();
    assert!(
        msg.contains("无法从 ~/.codex/auth.json 推断唯一的 `*_API_KEY` 字段"),
        "unexpected error message: {}",
        msg
    );
}

#[test]
fn load_or_bootstrap_for_service_writes_proxy_config() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        // Prepare Codex CLI config and auth under CODEX_HOME/HOME
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-789" }"#;
        write_file(&auth_path, auth_text);

        // 确保 proxy 配置文件起始不存在
        let proxy_cfg_path = super::proxy_home_dir().join("config.json");
        let proxy_cfg_toml_path = super::proxy_home_dir().join("config.toml");
        let _ = std::fs::remove_file(&proxy_cfg_path);
        let _ = std::fs::remove_file(&proxy_cfg_toml_path);

        let cfg = super::load_or_bootstrap_for_service(ServiceKind::Codex)
            .await
            .expect("load_or_bootstrap_for_service should succeed");

        // 内存中的配置应包含 right upstream 与正确的 token
        let svc = cfg.codex.active_station().expect("active codex station");
        assert_eq!(svc.name, "right");
        assert_eq!(svc.upstreams.len(), 1);
        assert!(svc.upstreams[0].auth.auth_token.is_none());
        assert_eq!(
            svc.upstreams[0].auth.auth_token_env.as_deref(),
            Some("RIGHTCODE_API_KEY")
        );

        // 并且应已将配置写入到 proxy_home_dir()/config.toml（fresh install defaults to TOML）
        let text = std::fs::read_to_string(&proxy_cfg_toml_path)
            .expect("config.toml should be written by load_or_bootstrap");
        let text = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let loaded: ProxyConfig =
            toml::from_str(&text).expect("config.toml should be valid ProxyConfig");
        let svc2 = loaded.codex.active_station().expect("active codex station");
        assert_eq!(svc2.name, "right");
        assert!(svc2.upstreams[0].auth.auth_token.is_none());
        assert_eq!(
            svc2.upstreams[0].auth.auth_token_env.as_deref(),
            Some("RIGHTCODE_API_KEY")
        );
    });
}

#[test]
fn bootstrap_from_codex_openai_defaults_to_requires_openai_auth_and_allows_missing_token() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
    write_file(&cfg_path, config_text);

    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

    let svc = cfg.codex.active_station().expect("active codex station");
    assert_eq!(svc.name, "openai");
    let up = &svc.upstreams[0];
    assert_eq!(up.base_url, "https://api.openai.com/v1");
    assert!(
        up.auth.auth_token.is_none(),
        "openai default requires_openai_auth=true should not force a stored token"
    );
    assert_eq!(
        up.tags.get("requires_openai_auth").map(|s| s.as_str()),
        Some("true")
    );
}

#[test]
fn bootstrap_from_codex_allows_requires_openai_auth_true_for_custom_provider() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "packycode"

[model_providers.packycode]
name = "packycode"
base_url = "https://codex-api.packycode.com/v1"
requires_openai_auth = true
wire_api = "responses"
"#;
    write_file(&cfg_path, config_text);

    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

    let svc = cfg.codex.active_station().expect("active codex station");
    assert_eq!(svc.name, "packycode");
    let up = &svc.upstreams[0];
    assert_eq!(up.base_url, "https://codex-api.packycode.com/v1");
    assert!(up.auth.auth_token.is_none());
    assert_eq!(
        up.tags.get("requires_openai_auth").map(|s| s.as_str()),
        Some("true")
    );
}

#[test]
fn probe_codex_bootstrap_detects_codex_proxy_without_backup() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
"#;
        write_file(&cfg_path, config_text);

        // 不写备份文件，模拟“已经被本地代理接管且无原始备份”的场景
        let err = super::probe_codex_bootstrap_from_cli()
            .await
            .expect_err("probe should fail when model_provider is codex_proxy without backup");
        let msg = err.to_string();
        assert!(
            msg.contains("当前 model_provider 指向本地代理 codex-helper，且未找到备份配置"),
            "unexpected error message: {}",
            msg
        );
    });
}

#[test]
fn sync_codex_auth_updates_env_key_without_changing_routing_config() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();

    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
    write_file(&cfg_path, config_text);

    let auth_path = home.join("auth.json");
    let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
    write_file(&auth_path, auth_text);

    let mut cfg = ProxyConfig::default();
    cfg.codex.active = Some("keep-active".to_string());
    cfg.codex.configs.insert(
        "right".to_string(),
        ServiceConfig {
            name: "right".to_string(),
            alias: None,
            enabled: false,
            level: 7,
            upstreams: vec![UpstreamConfig {
                base_url: "https://www.right.codes/codex/v1".to_string(),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: Some("OLD_KEY".to_string()),
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".into(), "right".into());
                    t.insert("source".into(), "codex-config".into());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let report = sync_codex_auth_from_codex_cli(
        &mut cfg,
        SyncCodexAuthFromCodexOptions {
            add_missing: false,
            set_active: false,
            force: false,
        },
    )
    .expect("sync should succeed");

    assert_eq!(report.updated, 1);
    assert_eq!(report.added, 0);
    assert!(!report.active_set);

    let svc = cfg.codex.configs.get("right").expect("right config exists");
    assert_eq!(svc.level, 7);
    assert!(!svc.enabled, "enabled should not be changed by sync");
    assert_eq!(
        svc.upstreams[0].auth.auth_token_env.as_deref(),
        Some("RIGHTCODE_API_KEY")
    );
    assert_eq!(
        cfg.codex.active.as_deref(),
        Some("keep-active"),
        "active should not be changed by sync unless set_active is true"
    );
}

#[test]
fn sync_codex_auth_can_add_missing_provider_and_set_active() {
    let env = setup_temp_codex_home();
    let home = env.home.clone();

    let cfg_path = home.join("config.toml");
    let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
    write_file(&cfg_path, config_text);

    let auth_path = home.join("auth.json");
    let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
    write_file(&auth_path, auth_text);

    let mut cfg = ProxyConfig::default();
    cfg.codex.active = Some("openai".to_string());

    let report = sync_codex_auth_from_codex_cli(
        &mut cfg,
        SyncCodexAuthFromCodexOptions {
            add_missing: true,
            set_active: true,
            force: false,
        },
    )
    .expect("sync should succeed");

    assert_eq!(report.added, 1);
    assert!(report.active_set);
    assert_eq!(cfg.codex.active.as_deref(), Some("right"));

    let svc = cfg
        .codex
        .configs
        .get("right")
        .expect("right config should be added");
    assert!(svc.enabled);
    assert_eq!(svc.level, 1);
    assert_eq!(
        svc.upstreams[0].auth.auth_token_env.as_deref(),
        Some("RIGHTCODE_API_KEY")
    );
    assert_eq!(
        svc.upstreams[0].tags.get("source").map(|s| s.as_str()),
        Some("codex-config")
    );
}
