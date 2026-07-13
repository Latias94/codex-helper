use super::*;

#[test]
fn retry_profile_defaults_to_balanced_when_unset() {
    let cfg = RetryConfig::default();
    let resolved = cfg.resolve();
    assert_eq!(resolved.upstream.strategy, RetryStrategy::SameUpstream);
    assert_eq!(resolved.upstream.max_attempts, 2);
    assert_eq!(resolved.upstream.backoff_ms, 200);
    assert_eq!(resolved.upstream.backoff_max_ms, 2_000);
    assert_eq!(resolved.upstream.jitter_ms, 100);
    assert_eq!(resolved.upstream.on_status, "429,500-502,504-528,530-599");
    assert!(
        resolved
            .upstream
            .on_class
            .iter()
            .any(|c| c == "upstream_transport_error")
    );
    assert!(
        resolved
            .upstream
            .on_class
            .iter()
            .any(|c| c == "upstream_rate_limited")
    );
    assert!(
        resolved
            .upstream
            .on_class
            .iter()
            .any(|c| c == "upstream_overloaded")
    );

    assert_eq!(resolved.route.strategy, RetryStrategy::Failover);
    assert_eq!(resolved.route.max_attempts, 2);
    assert_eq!(resolved.route.on_status, "401,403,404,408,429,500-599,524");
    assert!(
        resolved
            .route
            .on_class
            .iter()
            .any(|c| c == "routing_mismatch_capability")
    );
    assert!(
        resolved
            .route
            .on_class
            .iter()
            .any(|c| c == "upstream_rate_limited")
    );
    assert!(
        resolved
            .route
            .on_class
            .iter()
            .any(|c| c == "upstream_overloaded")
    );
    assert_eq!(resolved.never_on_status, "413,415,422");
    assert!(
        resolved
            .never_on_class
            .iter()
            .any(|c| c == "client_error_non_retryable")
    );
    assert_eq!(resolved.cloudflare_challenge_cooldown_secs, 300);
    assert_eq!(resolved.cloudflare_timeout_cooldown_secs, 60);
    assert_eq!(resolved.transport_cooldown_secs, 30);
    assert_eq!(resolved.cooldown_backoff_factor, 1);
    assert_eq!(resolved.cooldown_backoff_max_secs, 600);
}

#[test]
fn retry_profile_cost_primary_sets_probe_back_defaults() {
    let cfg = RetryConfig {
        profile: Some(RetryProfileName::CostPrimary),
        ..RetryConfig::default()
    };
    let resolved = cfg.resolve();
    assert_eq!(resolved.route.strategy, RetryStrategy::Failover);
    assert_eq!(resolved.cooldown_backoff_factor, 2);
    assert_eq!(resolved.cooldown_backoff_max_secs, 900);
    assert_eq!(resolved.transport_cooldown_secs, 30);
}

#[test]
fn retry_profile_aggressive_failover_enables_broader_failover_with_guardrails() {
    let cfg = RetryConfig {
        profile: Some(RetryProfileName::AggressiveFailover),
        ..RetryConfig::default()
    };
    let resolved = cfg.resolve();
    assert_eq!(resolved.route.max_attempts, 3);
    assert_eq!(resolved.route.strategy, RetryStrategy::Failover);
    assert_eq!(resolved.route.on_status, "401,403,404,408,429,500-599,524");
    assert_eq!(resolved.upstream.on_status, "429,500-502,504-528,530-599");
    assert!(
        resolved
            .route
            .on_class
            .iter()
            .any(|c| c == "routing_mismatch_capability")
    );
    assert!(
        resolved
            .route
            .on_class
            .iter()
            .any(|c| c == "upstream_overloaded")
    );
    assert_eq!(resolved.never_on_status, "413,415,422");
    assert!(
        resolved
            .never_on_class
            .iter()
            .any(|c| c == "client_error_non_retryable")
    );
}

#[test]
fn retry_profile_allows_explicit_overrides() {
    let cfg = RetryConfig {
        profile: Some(RetryProfileName::SameUpstream),
        upstream: Some(RetryLayerConfig {
            max_attempts: Some(5),
            strategy: Some(RetryStrategy::Failover),
            ..RetryLayerConfig::default()
        }),
        ..RetryConfig::default()
    };
    let resolved = cfg.resolve();
    assert_eq!(resolved.upstream.max_attempts, 5);
    assert_eq!(resolved.upstream.strategy, RetryStrategy::Failover);
}

#[test]
fn retry_config_rejects_retired_cross_station_flag() {
    let text = r#"
version = 5

[retry]
allow_cross_station_before_first_output = true
"#;
    let err = toml::from_str::<HelperConfig>(text).expect_err("retired cross-station retry flag");

    assert!(
        err.to_string()
            .contains("unknown field `allow_cross_station_before_first_output`")
    );
}

#[test]
fn retry_profile_parses_from_toml_kebab_case() {
    let text = r#"
version = 5

[retry]
profile = "cost-primary"
"#;
    let cfg = toml::from_str::<HelperConfig>(text).expect("toml parse");
    assert_eq!(cfg.retry.profile, Some(RetryProfileName::CostPrimary));
}

#[test]
fn retry_config_rejects_retired_flat_fields() {
    let text = r#"
version = 5

[retry]
max_attempts = 5
"#;
    let err = toml::from_str::<HelperConfig>(text).expect_err("retired flat retry field");

    assert!(err.to_string().contains("unknown field `max_attempts`"));
}
