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
                    on_status: "429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
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
                    ],
                    strategy: RetryStrategy::Failover,
                },
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
                    on_status: "429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
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
