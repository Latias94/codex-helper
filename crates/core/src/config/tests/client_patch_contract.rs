use super::*;

#[test]
fn codex_client_patch_parses_v0203_values_and_serializes_canonical_names() {
    let cases = [
        ("default", CodexClientPreset::Default),
        ("chatgpt_bridge", CodexClientPreset::ChatGptBridge),
        ("imagegen_bridge", CodexClientPreset::ImagegenBridge),
        ("official-relay-bridge", CodexClientPreset::OfficialRelay),
        (
            "official_imagegen_bridge",
            CodexClientPreset::OfficialImagegen,
        ),
    ];

    for (value, expected) in cases {
        let text = format!(
            r#"
version = 6

[codex.client_patch]
preset = "{value}"
responses_websocket = false
compaction = "remote_v1"
translate_models = true
hosted_image_generation = "off"
"#
        );
        let config = toml::from_str::<HelperConfig>(&text).expect("parse v0.20.3 alias");
        let patch = config
            .codex
            .client_patch
            .as_ref()
            .expect("client patch config");
        assert_eq!(patch.preset, expected);
        assert_eq!(patch.compaction, CodexCompactionStrategy::RemoteV1);
        assert!(patch.translate_models);
        assert_eq!(
            patch.hosted_image_generation,
            CodexHostedImageGenerationMode::Disabled
        );

        let rendered = toml::to_string_pretty(&config).expect("serialize client patch config");
        assert!(
            rendered.contains(&format!("preset = \"{}\"", expected.as_str())),
            "unexpected serialization: {rendered}"
        );
        assert!(rendered.contains("compaction = \"remote-v1\""));
        assert!(rendered.contains("hosted_image_generation = \"disabled\""));
        assert!(!rendered.contains("mode ="));
    }
}

#[test]
fn codex_client_patch_accepts_matching_legacy_mode_and_rejects_conflicts() {
    let matching = toml::from_str::<HelperConfig>(
        r#"
version = 6

[codex.client_patch]
preset = "official-imagegen"
mode = "official_imagegen_bridge"
"#,
    )
    .expect("matching legacy mode");
    assert_eq!(
        matching
            .codex
            .client_patch
            .as_ref()
            .map(|patch| patch.preset),
        Some(CodexClientPreset::OfficialImagegen)
    );

    let error = toml::from_str::<HelperConfig>(
        r#"
version = 6

[codex.client_patch]
preset = "official-relay"
mode = "imagegen-bridge"
"#,
    )
    .expect_err("conflicting preset and mode must fail");
    assert!(
        error.to_string().contains("conflicting"),
        "unexpected error: {error}"
    );
}

#[test]
fn codex_client_patch_validation_preserves_orthogonal_hosted_image_policy() {
    let valid = toml::from_str::<HelperConfig>(
        r#"
version = 6

[codex.client_patch]
preset = "official-imagegen"
responses_websocket = true
compaction = "remote-v2"
hosted_image_generation = "disabled"
"#,
    )
    .expect("parse valid orthogonal patch");
    validate_helper_config(&valid).expect("validate official imagegen with hosted image disabled");

    for text in [
        r#"
version = 6
[codex.client_patch]
preset = "default"
compaction = "remote-v1"
"#,
        r#"
version = 6
[codex.client_patch]
preset = "official-relay"
compaction = "local"
responses_websocket = true
"#,
    ] {
        let config = toml::from_str::<HelperConfig>(text).expect("parse invalid combination");
        validate_helper_config(&config).expect_err("invalid client patch combination must fail");
    }
}

#[test]
fn claude_client_patch_is_rejected_as_a_codex_only_contract() {
    let config = toml::from_str::<HelperConfig>(
        r#"
version = 6

[claude.client_patch]
preset = "default"
"#,
    )
    .expect("parse typed service config");
    let error = validate_helper_config(&config).expect_err("Claude client patch must fail");
    assert!(error.to_string().contains("claude.client_patch"));
}

#[test]
fn client_patch_unknown_field_fails_without_rewriting_or_backing_up_source() {
    let _env = setup_temp_codex_home();
    let path = config_file_path();
    let source = r#"
version = 5

[codex.client_patch]
preset = "official-relay"
response_websocket = true
"#;
    write_file(&path, source);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");

    let error = runtime
        .block_on(load_config())
        .expect_err("unknown client patch field must fail");
    assert!(
        format!("{error:#}").contains("response_websocket"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read unchanged source"),
        source
    );
    assert!(!path.with_file_name("config.toml.bak").exists());
}

#[test]
fn every_legacy_toml_version_preserves_client_patch_during_migration() {
    let _env = setup_temp_codex_home();
    let path = config_file_path();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");

    for version in 1..=5 {
        let source = format!(
            r#"version = {version}

[codex.client_patch]
mode = "official_imagegen_bridge"
responses_websocket = true
compaction = "remote_v2"
translate_models = true
hosted_image_generation = "disabled"
"#
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_file_name("config.toml.bak"));
        write_file(&path, &source);

        let config = runtime
            .block_on(load_config())
            .unwrap_or_else(|error| panic!("version {version} migration failed: {error:#}"));
        let patch = config
            .codex
            .client_patch
            .as_ref()
            .expect("migrated client patch");
        assert_eq!(patch.preset, CodexClientPreset::OfficialImagegen);
        assert_eq!(patch.compaction, CodexCompactionStrategy::RemoteV2);
        assert!(patch.responses_websocket);
        assert!(patch.translate_models);
        assert_eq!(
            patch.hosted_image_generation,
            CodexHostedImageGenerationMode::Disabled
        );
        let migrated = std::fs::read_to_string(&path).expect("read migrated config");
        assert!(migrated.contains("preset = \"official-imagegen\""));
        assert!(!migrated.contains("mode ="));
        assert_eq!(
            std::fs::read_to_string(path.with_file_name("config.toml.bak"))
                .expect("read exact migration backup"),
            source
        );
    }
}

#[test]
fn typed_config_mutation_preserves_client_patch() {
    let _env = setup_temp_codex_home();
    let path = config_file_path();
    write_file(
        &path,
        r#"version = 6

[codex.client_patch]
preset = "official-imagegen"
responses_websocket = true
compaction = "remote-v2"
translate_models = true
hosted_image_generation = "disabled"
"#,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");

    runtime
        .block_on(mutate_helper_config(|config| {
            config.ui.language = Some("zh".to_string());
            Ok(())
        }))
        .expect("mutate unrelated typed setting");

    let config = runtime
        .block_on(load_config())
        .expect("reload mutated config");
    let patch = config
        .codex
        .client_patch
        .as_ref()
        .expect("client patch survives typed mutation");
    assert_eq!(patch.preset, CodexClientPreset::OfficialImagegen);
    assert_eq!(patch.compaction, CodexCompactionStrategy::RemoteV2);
    assert_eq!(config.ui.language.as_deref(), Some("zh"));
}
