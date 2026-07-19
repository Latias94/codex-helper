use super::*;
use crate::routing_ir::{CompiledRouteGraph, compile_route_plan_template};

fn config_with_auth(auth: UpstreamAuth) -> HelperConfig {
    let mut config = HelperConfig::default();
    config.codex.providers.insert(
        "relay".to_string(),
        ProviderConfig {
            base_url: Some("https://relay.example/v1".to_string()),
            auth,
            ..ProviderConfig::default()
        },
    );
    config
}

fn native(name: impl Into<String>) -> CredentialRef {
    CredentialRef::Native { name: name.into() }
}

fn absolute_secret_path() -> String {
    #[cfg(windows)]
    {
        r"C:\ProgramData\codex-helper\relay.token".to_string()
    }
    #[cfg(not(windows))]
    {
        "/run/secrets/codex-helper/relay.token".to_string()
    }
}

#[test]
fn inline_credential_value_zeroizes_after_its_final_owner_drops() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let zeroized = Arc::new(AtomicBool::new(false));
    let first =
        InlineCredentialValue::with_drop_observer("inline-secret-canary", Arc::clone(&zeroized));
    let final_owner = first.clone();

    drop(first);
    assert!(!zeroized.load(Ordering::SeqCst));
    drop(final_owner);
    assert!(zeroized.load(Ordering::SeqCst));
}

#[test]
fn credential_source_inspection_distinguishes_sources_without_exposing_values() {
    let unconfigured = UpstreamAuth::default();
    assert_eq!(
        unconfigured.auth_token_source(),
        UpstreamCredentialSource::Unconfigured
    );
    assert_eq!(
        unconfigured.api_key_source(),
        UpstreamCredentialSource::Unconfigured
    );

    let inline = UpstreamAuth {
        auth_token: Some("inline-secret-canary".to_string().into()),
        auth_token_env: Some("IGNORED_TOKEN_ENV".to_string()),
        ..UpstreamAuth::default()
    };
    let inline_source = inline.auth_token_source();
    assert_eq!(inline_source, UpstreamCredentialSource::LegacyInline);
    assert!(!format!("{inline_source:?}").contains("inline-secret-canary"));

    let environment = UpstreamAuth {
        api_key_env: Some("  RELAY_API_KEY  ".to_string()),
        ..UpstreamAuth::default()
    };
    assert_eq!(
        environment.api_key_source(),
        UpstreamCredentialSource::LegacyEnvironment {
            variable: "RELAY_API_KEY"
        }
    );

    let reference = native("relay.primary");
    let referenced = UpstreamAuth {
        auth_token_ref: Some(reference.clone()),
        ..UpstreamAuth::default()
    };
    assert_eq!(
        referenced.auth_token_source(),
        UpstreamCredentialSource::Reference {
            reference: &reference
        }
    );
}

#[test]
fn reference_sources_are_not_conflated_with_unconfigured_auth() {
    let bearer_reference = native("relay.bearer");
    let api_key_reference = CredentialRef::SecretFile {
        path: absolute_secret_path(),
    };
    let auth = UpstreamAuth {
        auth_token_ref: Some(bearer_reference.clone()),
        api_key_ref: Some(api_key_reference.clone()),
        ..UpstreamAuth::default()
    };

    assert_eq!(
        auth.auth_token_source(),
        UpstreamCredentialSource::Reference {
            reference: &bearer_reference
        }
    );
    assert_eq!(
        auth.api_key_source(),
        UpstreamCredentialSource::Reference {
            reference: &api_key_reference
        }
    );
}

#[test]
fn version_6_credential_references_round_trip_without_default_noise() {
    let path = absolute_secret_path();
    let source = format!(
        r#"
version = 6

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_ref = {{ source = "native", name = "relay.primary" }}
api_key_ref = {{ source = "secret_file", path = {:?} }}
"#,
        path
    );

    let config = toml::from_str::<HelperConfig>(&source).expect("parse version 6 references");
    validate_helper_config(&config).expect("validate version 6 references");
    let auth = config.codex.providers["relay"].effective_auth();
    assert_eq!(
        auth.auth_token_ref,
        Some(CredentialRef::Native {
            name: "relay.primary".to_string()
        })
    );
    assert_eq!(
        auth.api_key_ref,
        Some(CredentialRef::SecretFile { path: path.clone() })
    );

    let rendered = toml::to_string_pretty(&config).expect("serialize version 6 references");
    assert!(rendered.contains("source = \"native\""));
    assert!(rendered.contains("source = \"secret_file\""));
    assert!(!rendered.contains("allow_anonymous"));
    let reparsed = toml::from_str::<HelperConfig>(&rendered).expect("reparse references");
    validate_helper_config(&reparsed).expect("revalidate references");
    let reparsed_auth = reparsed.codex.providers["relay"].effective_auth();
    assert_eq!(reparsed_auth.auth_token_ref, auth.auth_token_ref);
    assert_eq!(reparsed_auth.api_key_ref, auth.api_key_ref);
}

#[test]
fn tagged_credential_references_reject_unknown_sources_and_extra_fields() {
    let invalid = [
        r#"source = "vault"
name = "relay.primary"
"#,
        r#"source = "native"
name = "relay.primary"
path = "/run/secrets/relay"
"#,
        r#"source = "secret_file"
path = "/run/secrets/relay"
name = "relay.primary"
"#,
    ];

    for source in invalid {
        toml::from_str::<CredentialRef>(source)
            .expect_err("unknown or ambiguous credential reference must be rejected");
    }
}

#[test]
fn nested_credential_reference_is_not_serialized_as_default_auth() {
    let mut config = HelperConfig::default();
    config.codex.providers.insert(
        "relay".to_string(),
        ProviderConfig {
            base_url: Some("https://relay.example/v1".to_string()),
            auth: UpstreamAuth {
                auth_token_ref: Some(native("relay.nested")),
                ..UpstreamAuth::default()
            },
            ..ProviderConfig::default()
        },
    );

    validate_helper_config(&config).expect("validate nested reference");
    let rendered = toml::to_string_pretty(&config).expect("serialize nested reference");

    let raw = toml::from_str::<toml::Value>(&rendered).expect("parse serialized nested reference");
    assert_eq!(
        raw["codex"]["providers"]["relay"]["auth"]["auth_token_ref"]["name"].as_str(),
        Some("relay.nested")
    );
    let reparsed = toml::from_str::<HelperConfig>(&rendered).expect("reparse nested reference");
    assert_eq!(
        reparsed.codex.providers["relay"].auth.auth_token_ref,
        Some(native("relay.nested"))
    );
}

#[test]
fn native_credential_name_validation_covers_every_boundary() {
    let invalid_names = [
        "".to_string(),
        ".leading".to_string(),
        "Uppercase".to_string(),
        "contains/slash".to_string(),
        "non-ascii-凭据".to_string(),
        format!("a{}", "b".repeat(128)),
    ];

    for name in invalid_names {
        let config = config_with_auth(UpstreamAuth {
            auth_token_ref: Some(native(name.clone())),
            ..UpstreamAuth::default()
        });
        let error = match validate_helper_config(&config) {
            Ok(()) => panic!("native name {name:?} must be rejected"),
            Err(error) => error,
        };
        let message = format!("{error:#}");
        assert!(message.contains("auth_token_ref"), "unexpected: {message}");
        assert!(message.contains("native"), "unexpected: {message}");
    }

    for name in ["a".to_string(), format!("a{}", "b".repeat(127))] {
        let config = config_with_auth(UpstreamAuth {
            auth_token_ref: Some(native(name)),
            ..UpstreamAuth::default()
        });
        validate_helper_config(&config).expect("valid boundary name");
    }
}

#[test]
fn secret_file_reference_requires_an_absolute_path_without_reading_it() {
    let relative = config_with_auth(UpstreamAuth {
        auth_token_ref: Some(CredentialRef::SecretFile {
            path: "relative/relay.token".to_string(),
        }),
        ..UpstreamAuth::default()
    });
    let error = validate_helper_config(&relative).expect_err("relative path must fail");
    let message = format!("{error:#}");
    assert!(message.contains("auth_token_ref"), "unexpected: {message}");
    assert!(message.contains("absolute"), "unexpected: {message}");

    let absolute = config_with_auth(UpstreamAuth {
        auth_token_ref: Some(CredentialRef::SecretFile {
            path: absolute_secret_path(),
        }),
        ..UpstreamAuth::default()
    });
    validate_helper_config(&absolute).expect("schema validation must not read the secret file");
}

#[test]
fn references_reject_same_kind_legacy_sources_across_auth_layers() {
    let cases = [
        (
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(native("relay.primary")),
                    auth_token_env: Some("RELAY_TOKEN".to_string()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
            "auth_token_ref",
        ),
        (
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    api_key_ref: Some(CredentialRef::SecretFile {
                        path: absolute_secret_path(),
                    }),
                    ..UpstreamAuth::default()
                },
                inline_auth: UpstreamAuth {
                    api_key_env: Some("RELAY_API_KEY".to_string()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
            "api_key_ref",
        ),
        (
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(native("relay.primary")),
                    ..UpstreamAuth::default()
                },
                inline_auth: UpstreamAuth {
                    auth_token: Some("legacy-inline".to_string().into()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
            "auth_token_ref",
        ),
    ];

    for (provider, field) in cases {
        let mut config = HelperConfig::default();
        config.codex.providers.insert("relay".to_string(), provider);
        let error = validate_helper_config(&config).expect_err("mixed bearer source must fail");
        let message = format!("{error:#}");
        assert!(message.contains("relay"), "unexpected: {message}");
        assert!(message.contains(field), "unexpected: {message}");
    }
}

#[test]
fn flattened_reference_can_replace_nested_reference_but_not_other_kinds() {
    let provider = ProviderConfig {
        base_url: Some("https://relay.example/v1".to_string()),
        auth: UpstreamAuth {
            auth_token_ref: Some(native("relay.base")),
            ..UpstreamAuth::default()
        },
        inline_auth: UpstreamAuth {
            auth_token_ref: Some(native("relay.override")),
            api_key_env: Some("RELAY_API_KEY".to_string()),
            ..UpstreamAuth::default()
        },
        ..ProviderConfig::default()
    };
    let mut config = HelperConfig::default();
    config
        .codex
        .providers
        .insert("relay".to_string(), provider.clone());

    validate_helper_config(&config).expect("different kinds and ref override are valid");
    let effective = provider.effective_auth();
    assert_eq!(effective.auth_token_ref, Some(native("relay.override")));
    assert_eq!(effective.api_key_env.as_deref(), Some("RELAY_API_KEY"));
}

#[test]
fn flattened_reference_replaces_nested_legacy_source_for_the_same_kind() {
    let provider = ProviderConfig {
        base_url: Some("https://relay.example/v1".to_string()),
        auth: UpstreamAuth {
            auth_token: Some("nested-inline".to_string().into()),
            auth_token_env: Some("NESTED_TOKEN".to_string()),
            ..UpstreamAuth::default()
        },
        inline_auth: UpstreamAuth {
            auth_token_ref: Some(native("relay.override")),
            ..UpstreamAuth::default()
        },
        ..ProviderConfig::default()
    };
    let mut config = HelperConfig::default();
    config
        .codex
        .providers
        .insert("relay".to_string(), provider.clone());

    validate_helper_config(&config).expect("flattened reference replaces the nested source");
    let effective = provider.effective_auth();
    assert_eq!(effective.auth_token_ref, Some(native("relay.override")));
    assert_eq!(effective.auth_token, None);
    assert_eq!(effective.auth_token_env, None);
}

#[test]
fn flattened_api_key_reference_replaces_nested_legacy_source() {
    let reference = CredentialRef::SecretFile {
        path: absolute_secret_path(),
    };
    let provider = ProviderConfig {
        base_url: Some("https://relay.example/v1".to_string()),
        auth: UpstreamAuth {
            api_key: Some("nested-inline".to_string().into()),
            api_key_env: Some("NESTED_API_KEY".to_string()),
            ..UpstreamAuth::default()
        },
        inline_auth: UpstreamAuth {
            api_key_ref: Some(reference.clone()),
            ..UpstreamAuth::default()
        },
        ..ProviderConfig::default()
    };
    let mut config = HelperConfig::default();
    config
        .codex
        .providers
        .insert("relay".to_string(), provider.clone());

    validate_helper_config(&config).expect("flattened API-key reference replaces nested source");
    let effective = provider.effective_auth();
    assert_eq!(effective.api_key_ref, Some(reference));
    assert_eq!(effective.api_key, None);
    assert_eq!(effective.api_key_env, None);
}

#[test]
fn legacy_inline_over_environment_precedence_is_unchanged_without_references() {
    let provider = ProviderConfig {
        base_url: Some("https://relay.example/v1".to_string()),
        auth: UpstreamAuth {
            auth_token_env: Some("RELAY_TOKEN".to_string()),
            ..UpstreamAuth::default()
        },
        inline_auth: UpstreamAuth {
            auth_token: Some("inline-token".to_string().into()),
            ..UpstreamAuth::default()
        },
        ..ProviderConfig::default()
    };
    let mut config = HelperConfig::default();
    config
        .codex
        .providers
        .insert("relay".to_string(), provider.clone());

    validate_helper_config(&config).expect("legacy sources remain valid");
    assert_eq!(
        provider.effective_auth().auth_token.as_deref(),
        Some("inline-token")
    );
}

#[test]
fn route_digest_changes_with_reference_kind_or_locator() {
    fn digest(reference: CredentialRef) -> String {
        let config = config_with_auth(UpstreamAuth {
            auth_token_ref: Some(reference),
            ..UpstreamAuth::default()
        });
        validate_helper_config(&config).expect("validate digest config");
        CompiledRouteGraph::compile("codex", &config.codex)
            .expect("compile route graph")
            .digest()
            .to_string()
    }

    let native_a = digest(native("relay.a"));
    let native_b = digest(native("relay.b"));
    let secret_file = digest(CredentialRef::SecretFile {
        path: absolute_secret_path(),
    });

    assert_ne!(native_a, native_b);
    assert_ne!(native_a, secret_file);
    assert!(!native_a.contains("relay.a"));
}

#[test]
fn legacy_affinity_route_key_is_unchanged_by_version_6_migration() {
    let mut version_5 = config_with_auth(UpstreamAuth {
        auth_token: Some("unchanged-legacy-secret".to_string().into()),
        ..UpstreamAuth::default()
    });
    version_5.version = 5;
    let mut version_6 = version_5.clone();
    version_6.version = CURRENT_CONFIG_VERSION;

    let before = compile_route_plan_template("codex", &version_5.codex)
        .expect("compile version 5 affinity route")
        .route_graph_key();
    let after = compile_route_plan_template("codex", &version_6.codex)
        .expect("compile migrated affinity route")
        .route_graph_key();

    assert_eq!(after, before);
}
