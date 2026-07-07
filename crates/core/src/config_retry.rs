use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RetryProfileName {
    Balanced,
    SameUpstream,
    AggressiveFailover,
    CostPrimary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRetryLayerConfig {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub backoff_max_ms: u64,
    pub jitter_ms: u64,
    pub on_status: String,
    pub on_class: Vec<String>,
    pub strategy: RetryStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRetryConfig {
    pub upstream: ResolvedRetryLayerConfig,
    pub route: ResolvedRetryLayerConfig,
    #[serde(default = "ReasoningGuardConfig::default_resolved")]
    pub reasoning_guard: ResolvedReasoningGuardConfig,
    /// Guarded cross-station failover before any upstream output is committed to the client.
    pub allow_cross_station_before_first_output: bool,
    pub never_on_status: String,
    pub never_on_class: Vec<String>,
    pub cloudflare_challenge_cooldown_secs: u64,
    pub cloudflare_timeout_cooldown_secs: u64,
    pub transport_cooldown_secs: u64,
    pub cooldown_backoff_factor: u64,
    pub cooldown_backoff_max_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RetryLayerConfig {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub backoff_max_ms: Option<u64>,
    #[serde(default)]
    pub jitter_ms: Option<u64>,
    #[serde(default)]
    pub on_status: Option<String>,
    #[serde(default)]
    pub on_class: Option<Vec<String>>,
    #[serde(default)]
    pub strategy: Option<RetryStrategy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    /// Curated retry policy preset. When set, codex-helper starts from the profile defaults,
    /// then applies any explicitly configured fields below as overrides.
    #[serde(default)]
    pub profile: Option<RetryProfileName>,
    #[serde(default)]
    pub upstream: Option<RetryLayerConfig>,
    #[serde(default)]
    pub provider: Option<RetryLayerConfig>,
    #[serde(default)]
    pub reasoning_guard: Option<ReasoningGuardConfig>,
    /// Allow automatic failover to another station, but only before any output has been
    /// committed to the client. Session-pinned routes remain sticky regardless of this setting.
    #[serde(default)]
    pub allow_cross_station_before_first_output: Option<bool>,
    #[serde(default)]
    pub never_on_status: Option<String>,
    #[serde(default)]
    pub never_on_class: Option<Vec<String>>,
    #[serde(default)]
    pub cloudflare_challenge_cooldown_secs: Option<u64>,
    #[serde(default)]
    pub cloudflare_timeout_cooldown_secs: Option<u64>,
    #[serde(default)]
    pub transport_cooldown_secs: Option<u64>,
    /// Optional exponential backoff for cooldown penalties.
    /// When factor > 1, repeated penalties will increase cooldown up to max_secs.
    #[serde(default)]
    pub cooldown_backoff_factor: Option<u64>,
    #[serde(default)]
    pub cooldown_backoff_max_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningGuardAction {
    /// Forward the matching response and only emit diagnostics.
    Observe,
    /// Convert the matching response to a local 502 without attempting a retry.
    Block,
    /// Convert the matching response to a retryable local 502 until the guard retry budget is used.
    #[default]
    Retry,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningGuardStreamMode {
    /// Do not inspect streaming responses.
    Off,
    /// Inspect buffered streaming responses when another path already buffered them.
    Observe,
    /// Buffer the full stream before forwarding so the terminal usage block can be inspected.
    #[default]
    StrictBuffer,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReasoningGuardConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub reasoning_equals: Option<Vec<i64>>,
    #[serde(default)]
    pub boundary_sequence_max_n: Option<u32>,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    #[serde(default)]
    pub action: Option<ReasoningGuardAction>,
    #[serde(default)]
    pub stream_mode: Option<ReasoningGuardStreamMode>,
    #[serde(default)]
    pub max_guard_retries: Option<u32>,
    #[serde(default)]
    pub log_matches: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedReasoningGuardConfig {
    pub enabled: bool,
    pub reasoning_equals: Vec<i64>,
    #[serde(default = "default_reasoning_guard_boundary_sequence_max_n")]
    pub boundary_sequence_max_n: u32,
    pub paths: Vec<String>,
    pub action: ReasoningGuardAction,
    pub stream_mode: ReasoningGuardStreamMode,
    pub max_guard_retries: u32,
    pub log_matches: bool,
}

fn default_reasoning_guard_boundary_sequence_max_n() -> u32 {
    4
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetryStrategy {
    /// Prefer switching to another upstream on retry (default).
    #[default]
    Failover,
    /// Prefer retrying the same upstream (opt-in).
    SameUpstream,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            profile: Some(RetryProfileName::Balanced),
            upstream: None,
            provider: None,
            reasoning_guard: None,
            allow_cross_station_before_first_output: None,
            never_on_status: None,
            never_on_class: None,
            cloudflare_challenge_cooldown_secs: None,
            cloudflare_timeout_cooldown_secs: None,
            transport_cooldown_secs: None,
            cooldown_backoff_factor: None,
            cooldown_backoff_max_secs: None,
        }
    }
}

impl RetryProfileName {
    pub fn defaults(self) -> ResolvedRetryConfig {
        match self {
            RetryProfileName::Balanced => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 200,
                    backoff_max_ms: 2_000,
                    jitter_ms: 100,
                    on_status: "429,500-502,504-528,530-599".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
                        "upstream_rate_limited".to_string(),
                        "upstream_overloaded".to_string(),
                    ],
                    strategy: RetryStrategy::SameUpstream,
                },
                route: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 0,
                    backoff_max_ms: 0,
                    jitter_ms: 0,
                    on_status: "401,403,404,408,429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "routing_mismatch_capability".to_string(),
                        "image_generation_missing_result".to_string(),
                        "upstream_rate_limited".to_string(),
                        "upstream_overloaded".to_string(),
                    ],
                    strategy: RetryStrategy::Failover,
                },
                reasoning_guard: ReasoningGuardConfig::default_resolved(),
                allow_cross_station_before_first_output: false,
                never_on_status: "413,415,422".to_string(),
                never_on_class: vec!["client_error_non_retryable".to_string()],
                cloudflare_challenge_cooldown_secs: 300,
                cloudflare_timeout_cooldown_secs: 60,
                transport_cooldown_secs: 30,
                cooldown_backoff_factor: 1,
                cooldown_backoff_max_secs: 600,
            },
            RetryProfileName::SameUpstream => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 3,
                    ..RetryProfileName::Balanced.defaults().upstream
                },
                route: ResolvedRetryLayerConfig {
                    max_attempts: 1,
                    ..RetryProfileName::Balanced.defaults().route
                },
                ..RetryProfileName::Balanced.defaults()
            },
            RetryProfileName::AggressiveFailover => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 200,
                    backoff_max_ms: 2_500,
                    jitter_ms: 150,
                    on_status: "429,500-502,504-528,530-599".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
                        "upstream_rate_limited".to_string(),
                        "upstream_overloaded".to_string(),
                    ],
                    strategy: RetryStrategy::SameUpstream,
                },
                route: ResolvedRetryLayerConfig {
                    max_attempts: 3,
                    backoff_ms: 0,
                    backoff_max_ms: 0,
                    jitter_ms: 0,
                    on_status: "401,403,404,408,429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "routing_mismatch_capability".to_string(),
                        "upstream_rate_limited".to_string(),
                        "upstream_overloaded".to_string(),
                    ],
                    strategy: RetryStrategy::Failover,
                },
                allow_cross_station_before_first_output: true,
                ..RetryProfileName::Balanced.defaults()
            },
            RetryProfileName::CostPrimary => ResolvedRetryConfig {
                route: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    ..RetryProfileName::Balanced.defaults().route
                },
                allow_cross_station_before_first_output: true,
                transport_cooldown_secs: 30,
                cooldown_backoff_factor: 2,
                cooldown_backoff_max_secs: 900,
                ..RetryProfileName::Balanced.defaults()
            },
        }
    }
}

impl ReasoningGuardConfig {
    pub fn default_resolved() -> ResolvedReasoningGuardConfig {
        ResolvedReasoningGuardConfig {
            enabled: false,
            reasoning_equals: vec![516, 1034, 1552],
            boundary_sequence_max_n: 4,
            paths: vec![
                "/responses".to_string(),
                "/v1/responses".to_string(),
                "/chat/completions".to_string(),
                "/v1/chat/completions".to_string(),
            ],
            action: ReasoningGuardAction::Retry,
            stream_mode: ReasoningGuardStreamMode::StrictBuffer,
            max_guard_retries: 1,
            log_matches: true,
        }
    }

    pub fn resolve(&self) -> ResolvedReasoningGuardConfig {
        let mut out = Self::default_resolved();
        if let Some(v) = self.enabled {
            out.enabled = v;
        }
        if let Some(v) = self.reasoning_equals.as_ref() {
            out.reasoning_equals = v.clone();
        }
        if let Some(v) = self.boundary_sequence_max_n {
            out.boundary_sequence_max_n = v.min(16);
        }
        if let Some(v) = self.paths.as_ref() {
            out.paths = v
                .iter()
                .map(|path| normalize_reasoning_guard_path(path))
                .filter(|path| !path.is_empty())
                .collect();
        }
        if let Some(v) = self.action {
            out.action = v;
        }
        if let Some(v) = self.stream_mode {
            out.stream_mode = v;
        }
        if let Some(v) = self.max_guard_retries {
            out.max_guard_retries = v.min(8);
        }
        if let Some(v) = self.log_matches {
            out.log_matches = v;
        }
        out
    }
}

fn normalize_reasoning_guard_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut normalized = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}

impl RetryConfig {
    pub fn resolve(&self) -> ResolvedRetryConfig {
        let mut out = self
            .profile
            .unwrap_or(RetryProfileName::Balanced)
            .defaults();

        if let Some(layer) = self.upstream.as_ref() {
            if let Some(v) = layer.max_attempts {
                out.upstream.max_attempts = v;
            }
            if let Some(v) = layer.backoff_ms {
                out.upstream.backoff_ms = v;
            }
            if let Some(v) = layer.backoff_max_ms {
                out.upstream.backoff_max_ms = v;
            }
            if let Some(v) = layer.jitter_ms {
                out.upstream.jitter_ms = v;
            }
            if let Some(v) = layer.on_status.as_deref() {
                out.upstream.on_status = v.to_string();
            }
            if let Some(v) = layer.on_class.as_ref() {
                out.upstream.on_class = v.clone();
            }
            if let Some(v) = layer.strategy {
                out.upstream.strategy = v;
            }
        }
        if let Some(layer) = self.provider.as_ref() {
            if let Some(v) = layer.max_attempts {
                out.route.max_attempts = v;
            }
            if let Some(v) = layer.backoff_ms {
                out.route.backoff_ms = v;
            }
            if let Some(v) = layer.backoff_max_ms {
                out.route.backoff_max_ms = v;
            }
            if let Some(v) = layer.jitter_ms {
                out.route.jitter_ms = v;
            }
            if let Some(v) = layer.on_status.as_deref() {
                out.route.on_status = v.to_string();
            }
            if let Some(v) = layer.on_class.as_ref() {
                out.route.on_class = v.clone();
            }
            if let Some(v) = layer.strategy {
                out.route.strategy = v;
            }
        }
        if let Some(v) = self.allow_cross_station_before_first_output {
            out.allow_cross_station_before_first_output = v;
        }
        if let Some(v) = self.never_on_status.as_deref() {
            out.never_on_status = v.to_string();
        }
        if let Some(v) = self.never_on_class.as_ref() {
            out.never_on_class = v.clone();
        }
        if let Some(v) = self.reasoning_guard.as_ref() {
            out.reasoning_guard = v.resolve();
        }
        if let Some(v) = self.cloudflare_challenge_cooldown_secs {
            out.cloudflare_challenge_cooldown_secs = v;
        }
        if let Some(v) = self.cloudflare_timeout_cooldown_secs {
            out.cloudflare_timeout_cooldown_secs = v;
        }
        if let Some(v) = self.transport_cooldown_secs {
            out.transport_cooldown_secs = v;
        }
        if let Some(v) = self.cooldown_backoff_factor {
            out.cooldown_backoff_factor = v;
        }
        if let Some(v) = self.cooldown_backoff_max_secs {
            out.cooldown_backoff_max_secs = v;
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_guard_defaults_are_disabled() {
        let resolved = RetryConfig::default().resolve();

        assert!(!resolved.reasoning_guard.enabled);
        assert_eq!(
            resolved.reasoning_guard.reasoning_equals,
            vec![516, 1034, 1552]
        );
        assert_eq!(resolved.reasoning_guard.boundary_sequence_max_n, 4);
        assert_eq!(
            resolved.reasoning_guard.stream_mode,
            ReasoningGuardStreamMode::StrictBuffer
        );
        assert_eq!(resolved.reasoning_guard.action, ReasoningGuardAction::Retry);
        assert_eq!(resolved.reasoning_guard.max_guard_retries, 1);
    }

    #[test]
    fn resolved_retry_deserializes_legacy_payload_without_reasoning_guard() {
        let resolved: ResolvedRetryConfig = serde_json::from_value(serde_json::json!({
            "upstream": {
                "max_attempts": 2,
                "backoff_ms": 200,
                "backoff_max_ms": 2000,
                "jitter_ms": 100,
                "on_status": "429,500-599,524",
                "on_class": ["upstream_transport_error"],
                "strategy": "same_upstream"
            },
            "route": {
                "max_attempts": 2,
                "backoff_ms": 0,
                "backoff_max_ms": 0,
                "jitter_ms": 0,
                "on_status": "401,403,404,408,429,500-599,524",
                "on_class": ["upstream_transport_error"],
                "strategy": "failover"
            },
            "allow_cross_station_before_first_output": true,
            "never_on_status": "413,415,422",
            "never_on_class": ["client_error_non_retryable"],
            "cloudflare_challenge_cooldown_secs": 300,
            "cloudflare_timeout_cooldown_secs": 12,
            "transport_cooldown_secs": 45,
            "cooldown_backoff_factor": 3,
            "cooldown_backoff_max_secs": 180
        }))
        .expect("legacy resolved retry payload should deserialize");

        assert_eq!(
            resolved.reasoning_guard,
            ReasoningGuardConfig::default_resolved()
        );
    }

    #[test]
    fn resolved_retry_deserializes_legacy_reasoning_guard_without_boundary_sequence() {
        let resolved: ResolvedRetryConfig = serde_json::from_value(serde_json::json!({
            "upstream": {
                "max_attempts": 2,
                "backoff_ms": 200,
                "backoff_max_ms": 2000,
                "jitter_ms": 100,
                "on_status": "429,500-599,524",
                "on_class": ["upstream_transport_error"],
                "strategy": "same_upstream"
            },
            "route": {
                "max_attempts": 2,
                "backoff_ms": 0,
                "backoff_max_ms": 0,
                "jitter_ms": 0,
                "on_status": "401,403,404,408,429,500-599,524",
                "on_class": ["upstream_transport_error"],
                "strategy": "failover"
            },
            "reasoning_guard": {
                "enabled": true,
                "reasoning_equals": [516, 1034, 1552],
                "paths": ["/responses", "/v1/responses"],
                "action": "retry",
                "stream_mode": "strict-buffer",
                "max_guard_retries": 1,
                "log_matches": true
            },
            "allow_cross_station_before_first_output": true,
            "never_on_status": "413,415,422",
            "never_on_class": ["client_error_non_retryable"],
            "cloudflare_challenge_cooldown_secs": 300,
            "cloudflare_timeout_cooldown_secs": 12,
            "transport_cooldown_secs": 45,
            "cooldown_backoff_factor": 3,
            "cooldown_backoff_max_secs": 180
        }))
        .expect("legacy resolved retry reasoning guard should deserialize");

        assert!(resolved.reasoning_guard.enabled);
        assert_eq!(resolved.reasoning_guard.boundary_sequence_max_n, 4);
    }

    #[test]
    fn reasoning_guard_toml_overrides_resolve() {
        let cfg: RetryConfig = toml::from_str(
            r#"
profile = "balanced"

[reasoning_guard]
enabled = true
reasoning_equals = [516, 777]
boundary_sequence_max_n = 0
paths = ["responses", "/v1/chat/completions/"]
action = "block"
stream_mode = "off"
max_guard_retries = 3
log_matches = false
"#,
        )
        .expect("parse retry config");

        let resolved = cfg.resolve();
        assert!(resolved.reasoning_guard.enabled);
        assert_eq!(resolved.reasoning_guard.reasoning_equals, vec![516, 777]);
        assert_eq!(resolved.reasoning_guard.boundary_sequence_max_n, 0);
        assert_eq!(
            resolved.reasoning_guard.paths,
            vec!["/responses".to_string(), "/v1/chat/completions".to_string()]
        );
        assert_eq!(resolved.reasoning_guard.action, ReasoningGuardAction::Block);
        assert_eq!(
            resolved.reasoning_guard.stream_mode,
            ReasoningGuardStreamMode::Off
        );
        assert_eq!(resolved.reasoning_guard.max_guard_retries, 3);
        assert!(!resolved.reasoning_guard.log_matches);
    }
}
