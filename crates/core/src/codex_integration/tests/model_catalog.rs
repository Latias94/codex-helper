use super::*;

#[test]
fn model_catalog_url_switch_on_preserves_trailing_slash_in_query() {
    let env = setup_temp_env();
    let config_path = env.codex_home.join("config.toml");

    switch_on_with_base_url(
        " https://relay.example/prefix/?route=tenant/ ",
        CodexPatchMode::Default,
        CodexSwitchOptions::default(),
    )
    .expect("patch isolated relay config");

    let config: Value = toml::from_str(&read_file(&config_path)).expect("patched relay TOML");
    let provider = &config["model_providers"]["codex_proxy"];
    assert_eq!(
        provider["base_url"].as_str(),
        Some("https://relay.example/prefix/?route=tenant/"),
    );
    assert_eq!(
        provider.get("model_catalog_url").and_then(Value::as_str),
        Some("https://relay.example/prefix/models?route=tenant/"),
    );
    switch_off().expect("restore isolated relay config");
    assert!(!config_path.exists());
}

#[test]
fn model_catalog_url_uses_proxy_base_path_and_preserves_query() {
    let plan =
        CodexPatchPlan::for_switch_on(CodexPatchMode::Default, CodexSwitchOptions::default())
            .expect("default patch plan");

    for (base_url, expected) in [
        ("http://127.0.0.1:3211", "http://127.0.0.1:3211/models"),
        ("http://127.0.0.1:3211/", "http://127.0.0.1:3211/models"),
        (
            "http://localhost:3211/v1",
            "http://localhost:3211/v1/models",
        ),
        (
            "http://localhost:3211/v1/",
            "http://localhost:3211/v1/models",
        ),
        (
            "https://relay.example/custom/prefix",
            "https://relay.example/custom/prefix/models",
        ),
        (
            "https://relay.example/custom/prefix///",
            "https://relay.example/custom/prefix/models",
        ),
        (
            "http://[::1]:3211/v1?route=a&route=b&empty=",
            "http://[::1]:3211/v1/models?route=a&route=b&empty=",
        ),
        (
            "https://relay.example/custom%2Fprefix/?target=%2Fv1&route=tenant/",
            "https://relay.example/custom%2Fprefix/models?target=%2Fv1&route=tenant/",
        ),
        (
            "http://127.0.0.1:3211?route=tenant/#client-fragment",
            "http://127.0.0.1:3211/models?route=tenant/",
        ),
    ] {
        let patched = switch_on_codex_toml_with_base_url_and_plan(
            "",
            base_url,
            plan,
            CodexHostedImageGenerationMode::Auto,
        )
        .expect("patch proxy catalog URL");
        let config: Value = toml::from_str(&patched).expect("patched TOML");
        let provider = &config["model_providers"]["codex_proxy"];
        assert_eq!(provider["base_url"].as_str(), Some(base_url));
        assert_eq!(
            provider.get("model_catalog_url").and_then(Value::as_str),
            Some(expected),
            "proxy base URL: {base_url}",
        );
    }
}

#[test]
fn model_catalog_url_is_explicit_for_every_patch_mode_and_idempotent() {
    for mode in [
        CodexPatchMode::Default,
        CodexPatchMode::ChatGptBridge,
        CodexPatchMode::ImagegenBridge,
        CodexPatchMode::OfficialRelayBridge,
        CodexPatchMode::OfficialImagegenBridge,
    ] {
        let patched =
            switch_on_codex_toml_with_mode("", 3211, mode).expect("patch provider catalog URL");
        let config: Value = toml::from_str(&patched).expect("patched TOML");
        assert_eq!(
            config["model_providers"]["codex_proxy"]
                .get("model_catalog_url")
                .and_then(Value::as_str),
            Some("http://127.0.0.1:3211/models"),
            "patch mode: {mode}",
        );
        assert!(config.get("features").is_none());
        assert_eq!(
            switch_on_codex_toml_with_mode(&patched, 3211, mode)
                .expect("repeat provider catalog patch"),
            patched,
            "patch mode: {mode}",
        );
    }
}

#[test]
fn model_catalog_url_replaces_upstream_catalog_and_restores_original_custom_config() {
    let original = r#"# Preserve the user's selected provider.
model_provider = "custom"

[features]
api_key_model_discovery = false # Keep the user's explicit choice.

[model_providers.custom]
name = "Custom"
base_url = "https://upstream.example/v1"
model_catalog_url = "https://upstream.example/custom-models"

[model_providers.codex_proxy]
name = "Original proxy"
base_url = "https://original.example/v1"
model_catalog_url = "https://original.example/catalog" # Restore this catalog.
request_max_retries = 5

[model_providers.codex_proxy.http_headers]
x-custom = "preserved"
"#;
    let state = CodexSwitchState::from_codex_config_text(original, false)
        .expect("snapshot original custom provider");
    let patched = switch_on_codex_toml_with_mode(original, 3211, CodexPatchMode::Default)
        .expect("patch local catalog URL");
    let config: Value = toml::from_str(&patched).expect("patched TOML");
    assert_eq!(
        config["model_providers"]["codex_proxy"]
            .get("model_catalog_url")
            .and_then(Value::as_str),
        Some("http://127.0.0.1:3211/models"),
    );
    assert_eq!(
        config["features"]["api_key_model_discovery"].as_bool(),
        Some(false)
    );
    assert_eq!(
        config["model_providers"]["codex_proxy"]["request_max_retries"].as_integer(),
        Some(5)
    );
    assert_eq!(
        config["model_providers"]["codex_proxy"]["http_headers"]["x-custom"].as_str(),
        Some("preserved")
    );
    assert!(patched.contains("# Restore this catalog."));
    assert!(patched.contains("# Keep the user's explicit choice."));
    let original_config: Value = toml::from_str(original).expect("original TOML");
    assert_eq!(
        config["model_providers"]["custom"],
        original_config["model_providers"]["custom"]
    );
    assert_eq!(
        switch_on_codex_toml_with_mode(&patched, 3211, CodexPatchMode::Default)
            .expect("repeat local catalog patch"),
        patched,
    );
    let CodexSwitchOffEdit::Write(restored) =
        switch_off_codex_toml(&patched, &state).expect("restore original custom provider")
    else {
        panic!("existing custom config must be restored");
    };
    assert_eq!(
        toml::from_str::<Value>(&restored).expect("restored TOML"),
        original_config
    );
    assert_eq!(
        toml::from_str::<Value>(
            &codex_config_text_with_switch_state(&patched, &state)
                .expect("import original custom config"),
        )
        .expect("imported TOML"),
        original_config,
    );
}

#[test]
fn model_catalog_url_updates_with_proxy_base_and_is_removed_on_switch_off() {
    let original = "model = \"user-model\"\n";
    let state = CodexSwitchState::from_codex_config_text(original, false)
        .expect("snapshot config without proxy");
    let patched = switch_on_codex_toml_with_mode(original, 3211, CodexPatchMode::Default)
        .expect("patch first local catalog URL");
    let repatched = switch_on_codex_toml_with_mode(&patched, 4321, CodexPatchMode::Default)
        .expect("update local catalog URL");
    let config: Value = toml::from_str(&repatched).expect("repatched TOML");
    assert_eq!(
        config["model_providers"]["codex_proxy"]
            .get("model_catalog_url")
            .and_then(Value::as_str),
        Some("http://127.0.0.1:4321/models"),
    );
    let CodexSwitchOffEdit::Write(restored) =
        switch_off_codex_toml(&repatched, &state).expect("remove helper provider and catalog URL")
    else {
        panic!("existing model selection must be preserved");
    };
    assert_eq!(restored, original);

    let absent_state =
        CodexSwitchState::from_codex_config_text("", true).expect("snapshot absent config");
    let patched = switch_on_codex_toml_with_mode("", 3211, CodexPatchMode::Default)
        .expect("patch absent config");
    assert!(matches!(
        switch_off_codex_toml(&patched, &absent_state).expect("restore absent config"),
        CodexSwitchOffEdit::RemoveFile,
    ));
}
