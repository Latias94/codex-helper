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
fn malformed_current_toml_reports_parser_error_and_blocks_load_and_save() {
    let _env = setup_temp_codex_home();
    let dir = super::proxy_home_dir();
    let toml_path = dir.join("config.toml");
    let original = b"version = 5\n[notify\nenabled = true\n";
    std::fs::write(&toml_path, original).expect("write malformed config");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    for (label, error) in [
        (
            "load",
            runtime
                .block_on(super::load_config())
                .expect_err("malformed config must not load"),
        ),
        (
            "save",
            runtime
                .block_on(super::save_helper_config(&HelperConfig::default()))
                .expect_err("malformed config must not be overwritten"),
        ),
    ] {
        let message = error.to_string();
        assert!(
            message.contains("parse current config.toml"),
            "{label} error must identify the parse failure: {message}"
        );
        assert!(
            message.contains("line") && message.contains("column"),
            "{label} error must retain parser location: {message}"
        );
        assert!(!message.contains("unsupported config version"));
    }
    assert_eq!(
        std::fs::read(&toml_path).expect("read preserved malformed config"),
        original
    );
    assert!(!dir.join("config.toml.bak").exists());
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
fn config_mutations_fail_before_overwrite_when_backup_cannot_be_written() {
    let _env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        let backup_path = dir.join("config.toml.bak");
        let original = "version = 5\n[notify]\nenabled = true\n";
        write_file(&toml_path, original);
        std::fs::create_dir(&backup_path).expect("reserve backup path as a directory");

        let init_error = super::init_config_toml(true)
            .await
            .expect_err("init must stop when the backup cannot be written");
        assert!(init_error.to_string().contains("back up config.toml"));
        assert_eq!(
            std::fs::read_to_string(&toml_path).expect("read config after failed init"),
            original
        );

        let save_error = super::save_helper_config(&HelperConfig::default())
            .await
            .expect_err("save must stop when the backup cannot be written");
        assert!(save_error.to_string().contains("back up config.toml"));
        assert_eq!(
            std::fs::read_to_string(&toml_path).expect("read config after failed save"),
            original
        );
    });
}

#[cfg(unix)]
#[test]
fn config_mutations_reject_config_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let env = setup_temp_codex_home();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let dir = super::proxy_home_dir();
        let toml_path = dir.join("config.toml");
        let target_path = env.home.join("dotfiles/codex-helper.toml");
        let original = "version = 5\n[notify]\nenabled = true\n";
        write_file(&target_path, original);
        symlink(&target_path, &toml_path).expect("create config symlink");
        let original_link = std::fs::read_link(&toml_path).expect("read config symlink");

        let save_error = super::save_helper_config(&HelperConfig::default())
            .await
            .expect_err("typed save must reject config symlink");
        assert!(save_error.to_string().contains("symbolic link"));

        let init_error = super::init_config_toml(true)
            .await
            .expect_err("force init must reject config symlink");
        assert!(init_error.to_string().contains("symbolic link"));

        assert!(
            std::fs::symlink_metadata(&toml_path)
                .expect("inspect config symlink")
                .file_type()
                .is_symlink(),
            "config mutations must not replace the config symlink"
        );
        assert_eq!(
            std::fs::read_link(&toml_path).expect("read preserved config symlink"),
            original_link
        );
        assert_eq!(
            std::fs::read_to_string(&target_path).expect("read preserved config target"),
            original
        );
        assert!(!dir.join("config.toml.bak").exists());
    });
}

#[cfg(unix)]
#[test]
fn config_symlink_cannot_alias_the_backup_path() {
    use std::os::unix::fs::symlink;

    let _env = setup_temp_codex_home();
    let dir = super::proxy_home_dir();
    let toml_path = dir.join("config.toml");
    let backup_path = dir.join("config.toml.bak");
    let original = "version = 5\n[notify]\nenabled = true\n";
    write_file(&backup_path, original);
    symlink(&backup_path, &toml_path).expect("alias config to backup path");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let save_error = runtime
        .block_on(super::save_helper_config(&HelperConfig::default()))
        .expect_err("save must reject backup alias");
    assert!(save_error.to_string().contains("symbolic link"));
    let init_error = runtime
        .block_on(super::init_config_toml(true))
        .expect_err("init must reject backup alias");
    assert!(init_error.to_string().contains("symbolic link"));
    assert!(
        std::fs::symlink_metadata(&toml_path)
            .expect("inspect preserved alias")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(&backup_path).expect("read preserved backup target"),
        original
    );
}

#[cfg(unix)]
#[test]
fn valid_config_directory_symlink_is_preserved_for_mutations() {
    use std::os::unix::fs::symlink;

    let env = setup_temp_codex_home();
    let logical_dir = super::proxy_home_dir();
    let target_dir = env.home.join("dotfiles/codex-helper");
    std::fs::remove_dir(&logical_dir).expect("remove empty logical config directory");
    std::fs::create_dir_all(&target_dir).expect("create config directory target");
    symlink(&target_dir, &logical_dir).expect("create config directory symlink");
    write_file(
        &target_dir.join("config.toml"),
        "version = 5\n[notify]\nenabled = true\n",
    );

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(super::save_helper_config(&HelperConfig::default()))
        .expect("save through stable config directory symlink");

    assert!(
        std::fs::symlink_metadata(&logical_dir)
            .expect("inspect config directory symlink")
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(target_dir.join("config.toml"))
            .expect("read updated config")
            .contains("enabled = false")
    );
    assert!(target_dir.join("config.toml.bak").is_file());
}

#[cfg(unix)]
#[test]
fn dangling_config_directory_symlink_is_not_treated_as_missing() {
    use std::os::unix::fs::symlink;

    let env = setup_temp_codex_home();
    let logical_dir = super::proxy_home_dir();
    let missing_target = env.home.join("missing-helper-home");
    std::fs::remove_dir(&logical_dir).expect("remove empty logical config directory");
    symlink(&missing_target, &logical_dir).expect("create dangling config directory symlink");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(super::load_config())
        .expect_err("dangling config directory must not fall through to defaults");

    assert!(error.to_string().contains("resolve config directory"));
    assert!(std::fs::symlink_metadata(&logical_dir).is_ok());
    assert!(!missing_target.exists());
}

#[cfg(unix)]
#[test]
fn config_backup_preserves_private_source_mode() {
    use std::os::unix::fs::PermissionsExt;

    let _env = setup_temp_codex_home();
    let dir = super::proxy_home_dir();
    let toml_path = dir.join("config.toml");
    let backup_path = dir.join("config.toml.bak");
    write_file(&toml_path, "version = 5\n[notify]\nenabled = true\n");
    std::fs::set_permissions(&toml_path, std::fs::Permissions::from_mode(0o600))
        .expect("set private config mode");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(super::save_helper_config(&HelperConfig::default()))
        .expect("save private config");

    for path in [toml_path, backup_path] {
        let mode = std::fs::metadata(&path)
            .expect("read config mode")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{} must remain private", path.display());
    }
}

#[test]
fn config_mutation_lock_rejects_a_concurrent_writer_without_changes() {
    let _env = setup_temp_codex_home();
    let dir = super::proxy_home_dir();
    let toml_path = dir.join("config.toml");
    let lock_path = dir.join("config.toml.lock");
    let original = "version = 5\n[notify]\nenabled = true\n";
    write_file(&toml_path, original);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open config mutation lock");
    lock.try_lock().expect("hold config mutation lock");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(super::save_helper_config(&HelperConfig::default()))
        .expect_err("concurrent config mutation must fail");

    assert!(error.to_string().contains("another config mutation"));
    assert_eq!(
        std::fs::read_to_string(&toml_path).expect("read preserved config"),
        original
    );
    assert!(!dir.join("config.toml.bak").exists());
}

#[cfg(unix)]
#[test]
fn dangling_config_symlink_is_not_treated_as_missing() {
    use std::os::unix::fs::symlink;

    let env = setup_temp_codex_home();
    let dir = super::proxy_home_dir();
    let toml_path = dir.join("config.toml");
    let json_path = dir.join("config.json");
    let missing_target = env.home.join("missing-config.toml");
    write_file(&json_path, r#"{"version":4}"#);
    symlink(&missing_target, &toml_path).expect("create dangling config symlink");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(super::load_config())
        .expect_err("dangling canonical TOML must not fall through to JSON or defaults");

    assert!(error.to_string().contains("resolve config symlink"));
    assert!(std::fs::symlink_metadata(&toml_path).is_ok());
    assert!(!missing_target.exists());
    assert_eq!(
        std::fs::read_to_string(&json_path).expect("read preserved JSON"),
        r#"{"version":4}"#
    );
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
