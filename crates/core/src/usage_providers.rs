use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::balance::{
    BalanceSnapshotStatus, ProviderBalanceSnapshot, ProviderUsageAlert, ProviderUsageAlertKind,
    ProviderUsageModelStat, ProviderUsageRateSnapshot, ProviderUsageWindow,
};
use crate::config::{
    HelperConfig, ProviderConfig, ProviderEndpointConfig, ServiceRouteConfig, UpstreamAuth,
    proxy_home_dir,
};
use crate::pricing::UsdAmount;
use crate::runtime_identity::{ProviderEndpointKey, RuntimeUpstreamIdentity};
use crate::runtime_store::{
    ProviderObservation, ProviderObservationAuthority, ProviderObservationReservation,
    ProviderObservationScope, ProviderPolicyEffect,
};
use crate::state::ProxyState;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum ProviderKind {
    /// 简单预算接口，返回 total/used，判断是否用尽
    BudgetHttpJson,
    /// YesCode 账户用量，基于 /api/v1/auth/profile 返回的余额信息
    YescodeProfile,
    /// OpenAI-compatible relay balance endpoint, defaulting to /user/balance.
    #[serde(
        rename = "openai_balance_http_json",
        alias = "open_ai_balance_http_json",
        alias = "relay_balance_http_json"
    )]
    OpenAiBalanceHttpJson,
    /// Sub2API API-key telemetry endpoint, defaulting to /v1/usage.
    #[serde(rename = "sub2api_usage", alias = "sub2api_usage_http_json")]
    Sub2ApiUsage,
    /// Sub2API dashboard JWT account endpoint, defaulting to /api/v1/auth/me.
    #[serde(rename = "sub2api_auth_me", alias = "sub2api_auth_me_http_json")]
    Sub2ApiAuthMe,
    /// New API-style model token quota endpoint, defaulting to /api/usage/token/.
    #[serde(
        rename = "new_api_token_usage",
        alias = "new_api_token_usage_http_json"
    )]
    NewApiTokenUsage,
    /// New API-style user quota endpoint, defaulting to /api/user/self.
    NewApiUserSelf,
    /// RightCode account summary endpoint, defaulting to /account/summary.
    #[serde(
        rename = "rightcode_account_summary",
        alias = "right_code_account_summary",
        alias = "rightcode"
    )]
    RightCodeAccountSummary,
    /// OpenAI official organization Costs API, defaulting to a rolling 30-day cost window.
    #[serde(
        rename = "openai_organization_costs",
        alias = "openai_org_costs",
        alias = "openai_costs"
    )]
    OpenAiOrganizationCosts,
}

impl ProviderKind {
    fn source_name(&self) -> &'static str {
        match self {
            ProviderKind::BudgetHttpJson => "usage_provider:budget_http_json",
            ProviderKind::YescodeProfile => "usage_provider:yescode_profile",
            ProviderKind::OpenAiBalanceHttpJson => "usage_provider:openai_balance_http_json",
            ProviderKind::Sub2ApiUsage => "usage_provider:sub2api_usage",
            ProviderKind::Sub2ApiAuthMe => "usage_provider:sub2api_auth_me",
            ProviderKind::NewApiTokenUsage => "usage_provider:new_api_token_usage",
            ProviderKind::NewApiUserSelf => "usage_provider:new_api_user_self",
            ProviderKind::RightCodeAccountSummary => "usage_provider:rightcode_account_summary",
            ProviderKind::OpenAiOrganizationCosts => "usage_provider:openai_organization_costs",
        }
    }

    fn default_endpoint(&self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAiBalanceHttpJson => Some("/user/balance"),
            ProviderKind::Sub2ApiUsage => Some("/v1/usage"),
            ProviderKind::Sub2ApiAuthMe => Some("/api/v1/auth/me"),
            ProviderKind::NewApiTokenUsage => Some("/api/usage/token/"),
            ProviderKind::NewApiUserSelf => Some("/api/user/self"),
            ProviderKind::RightCodeAccountSummary => {
                Some("https://www.right.codes/account/summary")
            }
            ProviderKind::OpenAiOrganizationCosts => Some("/v1/organization/costs"),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(default)]
struct UsageProviderExtractConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    remaining_balance_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    subscription_balance_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    paygo_balance_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    monthly_budget_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    monthly_spent_paths: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exhausted_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining_divisor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_budget_divisor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    monthly_spent_divisor: Option<u64>,
    #[serde(skip_serializing_if = "bool_is_false")]
    derive_budget_from_remaining_and_spent: bool,
    #[serde(skip_serializing_if = "bool_is_false")]
    derive_remaining_from_budget_and_spent: bool,
}

impl UsageProviderExtractConfig {
    fn is_empty(&self) -> bool {
        self.remaining_balance_paths.is_empty()
            && self.subscription_balance_paths.is_empty()
            && self.paygo_balance_paths.is_empty()
            && self.monthly_budget_paths.is_empty()
            && self.monthly_spent_paths.is_empty()
            && self.exhausted_paths.is_empty()
            && self.remaining_divisor.is_none()
            && self.monthly_budget_divisor.is_none()
            && self.monthly_spent_divisor.is_none()
            && !self.derive_budget_from_remaining_and_spent
            && !self.derive_remaining_from_budget_and_spent
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UsageProviderConfig {
    id: String,
    kind: ProviderKind,
    domains: Vec<String>,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    token_env: Option<String>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    require_token_env: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_api_user_id_env: Option<String>,
    #[serde(default)]
    poll_interval_secs: Option<u64>,
    #[serde(
        default = "default_refresh_on_request",
        skip_serializing_if = "bool_is_true"
    )]
    refresh_on_request: bool,
    #[serde(
        default = "default_trust_exhaustion_for_routing",
        skip_serializing_if = "bool_is_true"
    )]
    trust_exhaustion_for_routing: bool,
    #[serde(default, skip_serializing_if = "UsageProviderExtractConfig::is_empty")]
    extract: UsageProviderExtractConfig,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
struct UsageProvidersFile {
    #[serde(default)]
    providers: Vec<UsageProviderConfig>,
}

#[derive(Debug, Clone)]
struct UsageProviderEndpointRef {
    provider_endpoint: ProviderEndpointKey,
    catalog_index: usize,
}

#[derive(Debug, Clone)]
struct UsageProviderTarget {
    endpoint: UsageProviderEndpointRef,
    base_url: String,
    auth: UpstreamAuth,
    tags: BTreeMap<String, String>,
    supported_models: BTreeMap<String, bool>,
    model_mapping: BTreeMap<String, String>,
}

impl UsageProviderTarget {
    fn runtime_identity(&self) -> RuntimeUpstreamIdentity {
        RuntimeUpstreamIdentity::new_with_auth(
            self.endpoint.provider_endpoint.clone(),
            self.base_url.clone(),
            self.tags.get("continuity_domain").cloned(),
            &self.auth,
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageProviderRefreshSummary {
    pub providers_configured: usize,
    pub providers_matched: usize,
    pub upstreams_matched: usize,
    pub attempted: usize,
    pub refreshed: usize,
    pub failed: usize,
    pub missing_token: usize,
    #[serde(skip_serializing_if = "usize_is_zero")]
    pub auto_attempted: usize,
    #[serde(skip_serializing_if = "usize_is_zero")]
    pub auto_refreshed: usize,
    #[serde(skip_serializing_if = "usize_is_zero")]
    pub auto_failed: usize,
    #[serde(skip_serializing_if = "usize_is_zero")]
    pub deduplicated: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageProviderRefreshOutcome {
    Refreshed,
    Failed,
    MissingToken,
}

struct RefreshProviderTargetParams<'a> {
    client: &'a Client,
    provider: &'a UsageProviderConfig,
    target: &'a UsageProviderTarget,
    state: &'a Arc<ProxyState>,
    service_name: &'a str,
    interval_secs: u64,
    force: bool,
}

// 全局节流状态：按 provider.id 记录最近一次查询时间，避免高频请求。
static LAST_USAGE_POLL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
static REQUEST_BALANCE_QUEUE: OnceLock<Mutex<HashMap<ProviderEndpointKey, Instant>>> =
    OnceLock::new();
static AUTO_PROBE_KIND_HINTS: OnceLock<Mutex<HashMap<String, ProviderKind>>> = OnceLock::new();
static AUTO_PROBE_KIND_FAILURES: OnceLock<Mutex<HashMap<AutoProbeKindFailureKey, Instant>>> =
    OnceLock::new();
static USAGE_PROVIDER_TARGET_SUPPRESSIONS: OnceLock<
    Mutex<HashMap<ProviderTargetSuppressionKey, ProviderTargetSuppression>>,
> = OnceLock::new();

const DEFAULT_POLL_INTERVAL_SECS: u64 = 10 * 60;
// Minimal request-driven poll interval per provider to avoid hammering usage APIs.
const MIN_POLL_INTERVAL_SECS: u64 = 2 * 60;
pub const REQUEST_BALANCE_REFRESH_DELAY: Duration = Duration::from_secs(60);
const BALANCE_REFRESH_CONCURRENCY: usize = 6;
const BALANCE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const BALANCE_HTTP_ERROR_BODY_LIMIT: usize = 2_048;
const AUTO_PROBE_KIND_FAILURE_TTL: Duration = Duration::from_secs(10 * 60);
const USAGE_PROVIDER_TERMINAL_FAILURE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const USAGE_PROVIDER_EXHAUSTED_SUPPRESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const USAGE_PROVIDER_DAILY_RESET_SUPPRESSION_GRACE: Duration = Duration::from_secs(5 * 60);
const DAILY_QUOTA_RESET_TIME: &str = "00:00";
const DAILY_QUOTA_RESET_UTC_OFFSET: &str = "+08:00";
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
const LOW_BALANCE_ALERT_THRESHOLD_USD: &str = "10";
const EXPIRING_SOON_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;
const AUTO_PROBE_KINDS: [ProviderKind; 5] = [
    ProviderKind::RightCodeAccountSummary,
    ProviderKind::Sub2ApiUsage,
    ProviderKind::NewApiTokenUsage,
    ProviderKind::NewApiUserSelf,
    ProviderKind::OpenAiBalanceHttpJson,
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AutoProbeKindFailureKey {
    provider_id: String,
    target: AutoProbeTargetKey,
    kind: ProviderKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderTargetSuppressionKey {
    provider_id: String,
    target: AutoProbeTargetKey,
}

#[derive(Debug, Clone)]
struct ProviderTargetSuppression {
    until: Instant,
    reason: String,
}

#[derive(Debug, Clone)]
struct ProviderTargetSuppressionDecision {
    reason: String,
    ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AutoProbeTargetKey {
    provider_endpoint: ProviderEndpointKey,
    base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestBalanceQueueDue {
    Due,
    NotDue(Duration),
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestBalancePollOutcome {
    Attempted,
    Deferred(Duration),
    Skipped,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn bool_is_true(value: &bool) -> bool {
    *value
}

fn usize_is_zero(value: &usize) -> bool {
    *value == 0
}

fn default_refresh_on_request() -> bool {
    true
}

fn default_trust_exhaustion_for_routing() -> bool {
    true
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn stale_after_ms(fetched_at_ms: u64, interval_secs: u64) -> Option<u64> {
    fetched_at_ms.checked_add(interval_secs.saturating_mul(3).saturating_mul(1_000))
}

fn snapshot_refresh_interval_secs(provider: &UsageProviderConfig) -> u64 {
    let interval_secs = provider
        .poll_interval_secs
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECS);
    if interval_secs == 0 {
        DEFAULT_POLL_INTERVAL_SECS
    } else {
        interval_secs.max(MIN_POLL_INTERVAL_SECS)
    }
}

fn effective_poll_interval_secs(provider: &UsageProviderConfig) -> Option<u64> {
    if !provider.refresh_on_request {
        return None;
    }

    let interval_secs = provider
        .poll_interval_secs
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECS);
    if interval_secs == 0 {
        return None;
    }
    Some(interval_secs.max(MIN_POLL_INTERVAL_SECS))
}

fn remaining_poll_cooldown(last: Instant, interval_secs: u64, now: Instant) -> Option<Duration> {
    let interval = Duration::from_secs(interval_secs);
    let elapsed = now.saturating_duration_since(last);
    interval.checked_sub(elapsed).filter(|d| !d.is_zero())
}

fn usage_providers_path() -> std::path::PathBuf {
    proxy_home_dir().join("usage_providers.json")
}

fn service_route_config<'a>(cfg: &'a HelperConfig, service_name: &str) -> &'a ServiceRouteConfig {
    match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    }
}

#[derive(Debug)]
struct ProviderEndpointCatalogEntry<'a> {
    endpoint_id: String,
    base_url: String,
    priority: u32,
    endpoint: Option<&'a ProviderEndpointConfig>,
}

fn provider_endpoint_catalog(provider: &ProviderConfig) -> Vec<ProviderEndpointCatalogEntry<'_>> {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|base_url| !base_url.is_empty())
    {
        endpoints.push(ProviderEndpointCatalogEntry {
            endpoint_id: "default".to_string(),
            base_url: base_url.to_string(),
            priority: 0,
            endpoint: None,
        });
    }
    endpoints.extend(
        provider
            .endpoints
            .iter()
            .filter(|(_, endpoint)| endpoint.enabled && !endpoint.base_url.trim().is_empty())
            .map(|(endpoint_id, endpoint)| ProviderEndpointCatalogEntry {
                endpoint_id: endpoint_id.clone(),
                base_url: endpoint.base_url.trim().to_string(),
                priority: endpoint.priority,
                endpoint: Some(endpoint),
            }),
    );
    endpoints.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    endpoints
}

fn usage_provider_target(
    service_name: &str,
    provider_id: &str,
    provider: &ProviderConfig,
    catalog_index: usize,
    endpoint: ProviderEndpointCatalogEntry<'_>,
) -> UsageProviderTarget {
    let mut tags = provider.tags.clone();
    if let Some(endpoint) = endpoint.endpoint {
        tags.extend(endpoint.tags.clone());
    }
    tags.insert("provider_id".to_string(), provider_id.to_string());
    tags.insert("endpoint_id".to_string(), endpoint.endpoint_id.clone());

    let endpoint_continuity_domain = endpoint
        .endpoint
        .and_then(|endpoint| endpoint.continuity_domain.as_ref())
        .or(provider.continuity_domain.as_ref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    if let Some(value) = endpoint_continuity_domain {
        tags.insert("continuity_domain".to_string(), value.to_string());
    }
    if let Some(value) = provider
        .continuity_domain
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        tags.insert("provider_continuity_domain".to_string(), value.to_string());
    }

    let mut supported_models = provider.supported_models.clone();
    let mut model_mapping = provider.model_mapping.clone();
    if let Some(endpoint) = endpoint.endpoint {
        supported_models.extend(endpoint.supported_models.clone());
        model_mapping.extend(endpoint.model_mapping.clone());
    }

    UsageProviderTarget {
        endpoint: UsageProviderEndpointRef {
            provider_endpoint: ProviderEndpointKey::new(
                service_name,
                provider_id,
                endpoint.endpoint_id,
            ),
            catalog_index,
        },
        base_url: endpoint.base_url,
        auth: provider.effective_auth(),
        tags,
        supported_models,
        model_mapping,
    }
}

fn default_provider_config(
    id: &str,
    kind: ProviderKind,
    domains: Vec<&str>,
    endpoint: &str,
    extract: UsageProviderExtractConfig,
) -> UsageProviderConfig {
    UsageProviderConfig {
        id: id.to_string(),
        kind,
        domains: domains.into_iter().map(str::to_string).collect(),
        endpoint: endpoint.to_string(),
        token_env: None,
        require_token_env: false,
        new_api_user_id_env: None,
        poll_interval_secs: Some(DEFAULT_POLL_INTERVAL_SECS),
        refresh_on_request: true,
        trust_exhaustion_for_routing: true,
        extract,
    }
}

fn default_rightcode_provider_config(id: &str) -> UsageProviderConfig {
    let mut provider = default_provider_config(
        id,
        ProviderKind::RightCodeAccountSummary,
        vec!["www.right.codes", "right.codes"],
        "https://www.right.codes/account/summary",
        UsageProviderExtractConfig::default(),
    );
    // RightCode subscription windows are daily capacity signals. A zero daily
    // remainder can coexist with account balance or be reset lazily, so the
    // built-in adapter displays it without demoting routes by default.
    provider.trust_exhaustion_for_routing = false;
    provider
}

fn host_from_base_url(base_url: &str) -> Option<String> {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
}

fn is_official_openai_base_url(base_url: &str) -> bool {
    host_from_base_url(base_url).as_deref() == Some("api.openai.com")
}

fn is_rightcode_base_url(base_url: &str) -> bool {
    matches!(
        host_from_base_url(base_url).as_deref(),
        Some("www.right.codes" | "right.codes")
    )
}

fn auto_provider_id(target: &UsageProviderTarget) -> String {
    target.endpoint.provider_endpoint.provider_id.clone()
}

fn auto_usage_provider(target: &UsageProviderTarget, kind: ProviderKind) -> UsageProviderConfig {
    let mut provider = UsageProviderConfig {
        id: auto_provider_id(target),
        kind,
        domains: host_from_base_url(&target.base_url)
            .into_iter()
            .collect::<Vec<_>>(),
        endpoint: String::new(),
        token_env: None,
        require_token_env: false,
        new_api_user_id_env: None,
        poll_interval_secs: Some(DEFAULT_POLL_INTERVAL_SECS),
        refresh_on_request: true,
        trust_exhaustion_for_routing: true,
        extract: UsageProviderExtractConfig::default(),
    };
    if matches!(kind, ProviderKind::RightCodeAccountSummary) {
        provider.trust_exhaustion_for_routing = false;
    }
    provider
}

fn auto_target_matches_provider_id_filter(
    target: &UsageProviderTarget,
    provider_id_filter: Option<&str>,
) -> bool {
    match provider_id_filter {
        Some(filter) => auto_provider_id(target) == filter,
        None => true,
    }
}

fn first_auto_probe_kind(target: &UsageProviderTarget) -> ProviderKind {
    if is_rightcode_base_url(&target.base_url) {
        ProviderKind::RightCodeAccountSummary
    } else {
        ProviderKind::Sub2ApiUsage
    }
}

fn auto_probe_target_key(target: &UsageProviderTarget) -> AutoProbeTargetKey {
    AutoProbeTargetKey {
        provider_endpoint: target.endpoint.provider_endpoint.clone(),
        base_url: normalized_balance_base_url(&target.base_url)
            .unwrap_or_else(|| target.base_url.clone()),
    }
}

fn auto_probe_kind_order(
    provider_id: &str,
    target: &UsageProviderTarget,
    force: bool,
) -> Vec<ProviderKind> {
    let now = Instant::now();
    let target_key = auto_probe_target_key(target);
    let mut ordered = Vec::new();
    if let Some(kind) = remembered_auto_probe_kind(provider_id) {
        ordered.push(kind);
    }
    ordered.push(first_auto_probe_kind(target));
    ordered.extend(AUTO_PROBE_KINDS);

    let mut seen = HashSet::new();
    ordered
        .into_iter()
        .filter(|kind| {
            if *kind == ProviderKind::RightCodeAccountSummary
                && !is_rightcode_base_url(&target.base_url)
            {
                return false;
            }
            seen.insert(*kind)
                && (force || !auto_probe_kind_failure_active(provider_id, &target_key, *kind, now))
        })
        .collect()
}

fn remembered_auto_probe_kind(provider_id: &str) -> Option<ProviderKind> {
    AUTO_PROBE_KIND_HINTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|map| map.get(provider_id).copied())
}

fn remember_auto_probe_kind_success(
    provider_id: &str,
    target: &UsageProviderTarget,
    kind: ProviderKind,
) {
    if let Ok(mut hints) = AUTO_PROBE_KIND_HINTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        hints.insert(provider_id.to_string(), kind);
    }
    if let Ok(mut failures) = AUTO_PROBE_KIND_FAILURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        failures.remove(&AutoProbeKindFailureKey {
            provider_id: provider_id.to_string(),
            target: auto_probe_target_key(target),
            kind,
        });
    }
    clear_usage_provider_target_suppression(provider_id, target);
}

fn remember_auto_probe_kind_failure(
    provider_id: &str,
    target: &UsageProviderTarget,
    kind: ProviderKind,
    now: Instant,
) {
    if let Ok(mut failures) = AUTO_PROBE_KIND_FAILURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        failures.insert(
            AutoProbeKindFailureKey {
                provider_id: provider_id.to_string(),
                target: auto_probe_target_key(target),
                kind,
            },
            now,
        );
    }
}

fn auto_probe_kind_failure_active(
    provider_id: &str,
    target: &AutoProbeTargetKey,
    kind: ProviderKind,
    now: Instant,
) -> bool {
    let key = AutoProbeKindFailureKey {
        provider_id: provider_id.to_string(),
        target: target.clone(),
        kind,
    };
    let Ok(mut failures) = AUTO_PROBE_KIND_FAILURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    else {
        return false;
    };
    let Some(failed_at) = failures.get(&key).copied() else {
        return false;
    };
    if now.duration_since(failed_at) < AUTO_PROBE_KIND_FAILURE_TTL {
        true
    } else {
        failures.remove(&key);
        false
    }
}

fn usage_provider_target_suppression_key(
    provider_id: &str,
    target: &UsageProviderTarget,
) -> ProviderTargetSuppressionKey {
    ProviderTargetSuppressionKey {
        provider_id: provider_id.to_string(),
        target: auto_probe_target_key(target),
    }
}

fn remember_usage_provider_target_suppression(
    provider_id: &str,
    target: &UsageProviderTarget,
    ttl: Duration,
    reason: impl Into<String>,
    now: Instant,
) {
    if let Ok(mut suppressions) = USAGE_PROVIDER_TARGET_SUPPRESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        suppressions.insert(
            usage_provider_target_suppression_key(provider_id, target),
            ProviderTargetSuppression {
                until: now + ttl,
                reason: reason.into(),
            },
        );
    }
}

fn clear_usage_provider_target_suppression(provider_id: &str, target: &UsageProviderTarget) {
    if let Ok(mut suppressions) = USAGE_PROVIDER_TARGET_SUPPRESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        suppressions.remove(&usage_provider_target_suppression_key(provider_id, target));
    }
}

#[cfg(test)]
fn clear_usage_provider_target_suppressions_for_provider(provider_id: &str) {
    if let Some(suppressions) = USAGE_PROVIDER_TARGET_SUPPRESSIONS.get()
        && let Ok(mut suppressions) = suppressions.lock()
    {
        suppressions.retain(|key, _| key.provider_id != provider_id);
    }
}

fn usage_provider_target_suppression_active(
    provider_id: &str,
    target: &UsageProviderTarget,
    now: Instant,
) -> Option<ProviderTargetSuppression> {
    let key = usage_provider_target_suppression_key(provider_id, target);
    let Ok(mut suppressions) = USAGE_PROVIDER_TARGET_SUPPRESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    else {
        return None;
    };
    let suppression = suppressions.get(&key).cloned()?;
    if now < suppression.until {
        Some(suppression)
    } else {
        suppressions.remove(&key);
        None
    }
}

fn usage_provider_suppression_reason_is_refreshable_window(reason: &str) -> bool {
    let normalized = normalized_error_text(reason);
    normalized.contains("package quota exhausted for current period")
        || normalized.contains("usage window exhausted for current period")
}

fn force_can_bypass_active_suppression(
    force: bool,
    suppression: &ProviderTargetSuppression,
    snapshot_decision: Option<&ProviderTargetSuppressionDecision>,
) -> bool {
    force
        && snapshot_decision.is_none()
        && (!usage_provider_error_is_terminal(suppression.reason.as_str())
            || usage_provider_suppression_reason_is_refreshable_window(suppression.reason.as_str()))
}

fn active_usage_provider_target_suppression(
    provider_id: &str,
    target: &UsageProviderTarget,
    now: Instant,
) -> Option<ProviderTargetSuppression> {
    usage_provider_target_suppression_active(provider_id, target, now)
}

fn auto_openai_official_provider(target: &UsageProviderTarget) -> UsageProviderConfig {
    let mut provider = auto_usage_provider(target, ProviderKind::OpenAiOrganizationCosts);
    provider.token_env = Some("OPENAI_ADMIN_KEY".to_string());
    provider.require_token_env = true;
    provider.refresh_on_request = false;
    provider.trust_exhaustion_for_routing = false;
    provider
}

fn default_providers() -> UsageProvidersFile {
    let openrouter_extract = UsageProviderExtractConfig {
        monthly_budget_paths: vec!["data.total_credits".to_string()],
        monthly_spent_paths: vec!["data.total_usage".to_string()],
        derive_remaining_from_budget_and_spent: true,
        ..Default::default()
    };

    let novita_extract = UsageProviderExtractConfig {
        remaining_balance_paths: vec!["availableBalance".to_string()],
        remaining_divisor: Some(10_000),
        ..Default::default()
    };

    let mut openai_official = default_provider_config(
        "openai-official-costs",
        ProviderKind::OpenAiOrganizationCosts,
        vec!["api.openai.com"],
        "https://api.openai.com/v1/organization/costs",
        UsageProviderExtractConfig::default(),
    );
    openai_official.token_env = Some("OPENAI_ADMIN_KEY".to_string());
    openai_official.require_token_env = true;
    openai_official.refresh_on_request = false;
    openai_official.trust_exhaustion_for_routing = false;

    UsageProvidersFile {
        providers: vec![
            default_rightcode_provider_config("rightcode"),
            default_provider_config(
                "packycode",
                ProviderKind::BudgetHttpJson,
                vec!["packycode.com"],
                "https://www.packycode.com/api/backend/users/info",
                UsageProviderExtractConfig::default(),
            ),
            default_provider_config(
                "yescode",
                ProviderKind::YescodeProfile,
                // Match co.yes.vg, cotest.yes.vg, and sibling subdomains.
                vec!["yes.vg"],
                "https://co.yes.vg/api/v1/auth/profile",
                UsageProviderExtractConfig::default(),
            ),
            default_provider_config(
                "deepseek",
                ProviderKind::OpenAiBalanceHttpJson,
                vec!["api.deepseek.com"],
                "https://api.deepseek.com/user/balance",
                UsageProviderExtractConfig::default(),
            ),
            default_provider_config(
                "stepfun",
                ProviderKind::OpenAiBalanceHttpJson,
                vec!["api.stepfun.ai", "api.stepfun.com"],
                "https://api.stepfun.com/v1/accounts",
                UsageProviderExtractConfig::default(),
            ),
            default_provider_config(
                "siliconflow",
                ProviderKind::OpenAiBalanceHttpJson,
                vec!["api.siliconflow.cn", "api.siliconflow.com"],
                "/v1/user/info",
                UsageProviderExtractConfig::default(),
            ),
            default_provider_config(
                "openrouter",
                ProviderKind::OpenAiBalanceHttpJson,
                vec!["openrouter.ai"],
                "https://openrouter.ai/api/v1/credits",
                openrouter_extract,
            ),
            default_provider_config(
                "novita",
                ProviderKind::OpenAiBalanceHttpJson,
                vec!["api.novita.ai"],
                "https://api.novita.ai/v3/user/balance",
                novita_extract,
            ),
            openai_official,
        ],
    }
}

fn load_providers_from_path(path: &std::path::Path) -> Result<UsageProvidersFile> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_providers());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to read usage provider configuration {}",
                    path.display()
                )
            });
        }
    };

    let file: UsageProvidersFile = serde_json::from_str(&text).with_context(|| {
        format!(
            "failed to parse usage provider configuration {}",
            path.display()
        )
    })?;
    for provider in &file.providers {
        validate_usage_provider_config(provider).with_context(|| {
            format!(
                "invalid usage provider '{}' in {}",
                provider.id,
                path.display()
            )
        })?;
    }
    Ok(file)
}

fn load_providers() -> Result<UsageProvidersFile> {
    load_providers_from_path(&usage_providers_path())
}

fn domain_matches(base_url: &str, domains: &[String]) -> bool {
    let url = match reqwest::Url::parse(base_url) {
        Ok(u) => u,
        Err(_) => return false,
    };
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    let host = host.to_ascii_lowercase();
    for d in domains {
        let domain = d.trim().to_ascii_lowercase();
        if host == domain || host.ends_with(&format!(".{}", domain)) {
            return true;
        }
    }
    false
}

fn matching_provider_targets(
    cfg: &HelperConfig,
    service_name: &str,
    provider: &UsageProviderConfig,
    route_provider_id_filter: Option<&str>,
) -> Vec<UsageProviderTarget> {
    usage_provider_targets(cfg, service_name, route_provider_id_filter)
        .into_iter()
        .filter(|target| domain_matches(&target.base_url, &provider.domains))
        .collect()
}

fn usage_provider_targets(
    cfg: &HelperConfig,
    service_name: &str,
    route_provider_id_filter: Option<&str>,
) -> Vec<UsageProviderTarget> {
    let mut targets = Vec::new();
    for (provider_id, provider) in &service_route_config(cfg, service_name).providers {
        if !provider.enabled
            || route_provider_id_filter.is_some_and(|filter| filter != provider_id.as_str())
        {
            continue;
        }
        for (catalog_index, endpoint) in provider_endpoint_catalog(provider).into_iter().enumerate()
        {
            targets.push(usage_provider_target(
                service_name,
                provider_id,
                provider,
                catalog_index,
                endpoint,
            ));
        }
    }

    targets
}

fn target_key(target: &UsageProviderTarget) -> ProviderEndpointKey {
    target.endpoint.provider_endpoint.clone()
}

fn enqueue_request_balance_refresh(key: ProviderEndpointKey) -> Option<Duration> {
    let now = Instant::now();
    let queue = REQUEST_BALANCE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut queue = match queue.lock() {
        Ok(queue) => queue,
        Err(_) => return None,
    };

    match queue.get(&key).copied() {
        Some(due_at) if due_at > now => None,
        Some(_) => Some(Duration::ZERO),
        None => {
            queue.insert(key, now + REQUEST_BALANCE_REFRESH_DELAY);
            Some(REQUEST_BALANCE_REFRESH_DELAY)
        }
    }
}

fn schedule_request_balance_refresh_at(key: ProviderEndpointKey, due_at: Instant) {
    let queue = REQUEST_BALANCE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut queue) = queue.lock() {
        queue.insert(key, due_at);
    }
}

fn take_request_balance_refresh_if_due(key: &ProviderEndpointKey) -> RequestBalanceQueueDue {
    let now = Instant::now();
    let queue = REQUEST_BALANCE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut queue = match queue.lock() {
        Ok(queue) => queue,
        Err(_) => return RequestBalanceQueueDue::Missing,
    };

    match queue.get(key).copied() {
        Some(due_at) if due_at <= now => {
            queue.remove(key);
            RequestBalanceQueueDue::Due
        }
        Some(due_at) => RequestBalanceQueueDue::NotDue(due_at.saturating_duration_since(now)),
        None => RequestBalanceQueueDue::Missing,
    }
}

#[cfg(test)]
pub fn request_balance_refresh_queued_for_provider_endpoint(
    provider_endpoint: &ProviderEndpointKey,
) -> bool {
    let Some(queue) = REQUEST_BALANCE_QUEUE.get() else {
        return false;
    };
    match queue.lock() {
        Ok(guard) => guard.contains_key(provider_endpoint),
        Err(error) => error.into_inner().contains_key(provider_endpoint),
    }
}

fn usage_provider_target_for_provider_endpoint(
    cfg: &HelperConfig,
    service_name: &str,
    provider_endpoint: &ProviderEndpointKey,
) -> Option<UsageProviderTarget> {
    usage_provider_targets(cfg, service_name, Some(&provider_endpoint.provider_id))
        .into_iter()
        .find(|target| target.endpoint.provider_endpoint == *provider_endpoint)
}

#[cfg(test)]
fn configured_target_keys(
    cfg: &HelperConfig,
    service_name: &str,
    providers: &[UsageProviderConfig],
    route_provider_id_filter: Option<&str>,
) -> HashSet<ProviderEndpointKey> {
    providers
        .iter()
        .flat_map(|provider| {
            matching_provider_targets(cfg, service_name, provider, route_provider_id_filter)
        })
        .map(|target| target_key(&target))
        .collect()
}

fn resolve_token(provider: &UsageProviderConfig, target: &UsageProviderTarget) -> Option<String> {
    // 优先: token_env 环境变量
    if let Some(env_name) = &provider.token_env
        && let Ok(v) = std::env::var(env_name)
        && !v.trim().is_empty()
    {
        return Some(v);
    }

    if provider.require_token_env {
        return None;
    }

    if let Some(token) = target.auth.resolve_auth_token() {
        return Some(token);
    }
    target.auth.resolve_api_key()
}

fn new_api_user_id_env_name(provider: &UsageProviderConfig) -> Result<Option<&str>> {
    let Some(env_name) = provider.new_api_user_id_env.as_deref() else {
        return Ok(None);
    };
    if !matches!(provider.kind, ProviderKind::NewApiUserSelf) {
        anyhow::bail!(
            "usage provider '{}' may set new_api_user_id_env only for kind new_api_user_self",
            provider.id
        );
    }
    let env_name = env_name.trim();
    if env_name.is_empty() {
        anyhow::bail!(
            "usage provider '{}' new_api_user_id_env must name an environment variable",
            provider.id
        );
    }
    Ok(Some(env_name))
}

fn validate_usage_provider_config(provider: &UsageProviderConfig) -> Result<()> {
    new_api_user_id_env_name(provider)?;
    Ok(())
}

fn resolve_new_api_user_id_with(
    provider: &UsageProviderConfig,
    read_env: impl FnOnce(&str) -> Option<String>,
) -> Result<Option<String>> {
    let Some(env_name) = new_api_user_id_env_name(provider)? else {
        return Ok(None);
    };
    let user_id = read_env(env_name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "usage provider '{}' requires New-Api-User from environment variable '{}'",
                provider.id,
                env_name
            )
        })?;
    Ok(Some(user_id))
}

fn resolve_new_api_user_id(provider: &UsageProviderConfig) -> Result<Option<String>> {
    resolve_new_api_user_id_with(provider, |env_name| std::env::var(env_name).ok())
}

fn normalized_balance_base_url(base_url: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(base_url).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    if path.eq_ignore_ascii_case("/v1") {
        url.set_path("");
    } else if path.to_ascii_lowercase().ends_with("/v1") {
        let new_path = &path[..path.len().saturating_sub(3)];
        url.set_path(if new_path.is_empty() { "/" } else { new_path });
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

fn base_path_prefixes(base_url: &str) -> Vec<String> {
    let Some(normalized) = normalized_balance_base_url(base_url) else {
        return Vec::new();
    };
    let Ok(url) = reqwest::Url::parse(&normalized) else {
        return Vec::new();
    };
    let parts = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut prefixes = Vec::new();
    for len in (1..=parts.len()).rev() {
        prefixes.push(format!("/{}", parts[..len].join("/")));
    }
    if prefixes.is_empty() {
        prefixes.push("/".to_string());
    }
    prefixes
}

fn path_prefixes_match(provider_prefixes: &[String], available_prefixes: &[String]) -> bool {
    if provider_prefixes.is_empty() || available_prefixes.is_empty() {
        return false;
    }
    provider_prefixes.iter().any(|provider_prefix| {
        available_prefixes.iter().any(|available_prefix| {
            provider_prefix == available_prefix
                || provider_prefix
                    .strip_prefix(available_prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    })
}

fn provider_endpoint_is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn provider_query_name_is_credential(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "accesskey"
            | "apikey"
            | "apitoken"
            | "key"
            | "token"
            | "accesstoken"
            | "authtoken"
            | "xapikey"
            | "authorization"
            | "bearer"
            | "bearertoken"
            | "password"
            | "passwd"
            | "secret"
            | "clientsecret"
            | "secretkey"
            | "privatekey"
            | "credential"
    )
}

fn validate_provider_endpoint(
    provider: &UsageProviderConfig,
    endpoint: &str,
    token: &str,
) -> Result<String> {
    let url = reqwest::Url::parse(endpoint).with_context(|| {
        format!(
            "usage provider '{}' endpoint is not a valid URL",
            provider.id
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!(
            "usage provider '{}' endpoint scheme must be HTTP or HTTPS",
            provider.id
        );
    }
    if url.scheme() != "https" && !provider_endpoint_is_loopback(&url) {
        anyhow::bail!(
            "usage provider '{}' endpoint must use HTTPS outside loopback",
            provider.id
        );
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!(
            "usage provider '{}' endpoint must not contain userinfo",
            provider.id
        );
    }
    if url.fragment().is_some() {
        anyhow::bail!(
            "usage provider '{}' endpoint must not contain a fragment",
            provider.id
        );
    }
    if !domain_matches(url.as_str(), &provider.domains) {
        anyhow::bail!(
            "usage provider '{}' endpoint host is outside its matched domains",
            provider.id
        );
    }
    if url.query_pairs().any(|(name, value)| {
        provider_query_name_is_credential(&name) || (!token.is_empty() && value == token)
    }) {
        anyhow::bail!(
            "usage provider '{}' endpoint must not contain query credentials",
            provider.id
        );
    }
    Ok(url.into())
}

fn resolve_endpoint(
    provider: &UsageProviderConfig,
    upstream_base_url: &str,
    token: &str,
) -> Result<String> {
    let base_url = normalized_balance_base_url(upstream_base_url)
        .ok_or_else(|| anyhow::anyhow!("invalid upstream base_url for balance endpoint"))?;
    let endpoint = if provider.endpoint.trim().is_empty() {
        provider
            .kind
            .default_endpoint()
            .unwrap_or_default()
            .to_string()
    } else {
        provider.endpoint.trim().to_string()
    };
    if endpoint.is_empty() {
        anyhow::bail!(
            "usage provider '{}' has no endpoint and kind {:?} has no default endpoint",
            provider.id,
            provider.kind
        );
    }
    if endpoint.contains("{{") || endpoint.contains("}}") {
        anyhow::bail!(
            "usage provider '{}' endpoint templates are not supported; use an absolute URL or relative path",
            provider.id
        );
    }

    let resolved = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint
    } else {
        let path = if endpoint.starts_with('/') {
            endpoint
        } else {
            format!("/{endpoint}")
        };
        format!("{base_url}{path}")
    };
    let resolved = if matches!(provider.kind, ProviderKind::OpenAiOrganizationCosts) {
        let mut url = reqwest::Url::parse(&resolved).with_context(|| {
            format!(
                "usage provider '{}' endpoint is not a valid URL",
                provider.id
            )
        })?;
        let retained_query = url
            .query_pairs()
            .filter(|(name, _)| name != "start_time" && name != "limit")
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs(retained_query);
            query.append_pair(
                "start_time",
                &unix_now_secs()
                    .saturating_sub(30 * 24 * 60 * 60)
                    .to_string(),
            );
            query.append_pair("limit", "30");
        }
        url.into()
    } else {
        resolved
    };
    validate_provider_endpoint(provider, &resolved, token)
}

struct PreparedProviderPoll {
    endpoint: String,
    new_api_user_id: Option<String>,
    reservation: Option<ProviderObservationReservation>,
}

fn credential_safe_digest(domain: &[u8], values: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    for value in values {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn usage_provider_account_fingerprint(token: &str, new_api_user_id: Option<&str>) -> String {
    match new_api_user_id {
        Some(user_id) => credential_safe_digest(
            b"codex-helper:usage-provider-account:v1\0",
            &[token.as_bytes(), user_id.as_bytes()],
        ),
        None => credential_safe_digest(
            b"codex-helper:usage-provider-account:v1\0",
            &[token.as_bytes()],
        ),
    }
}

fn usage_provider_config_revision(
    provider: &UsageProviderConfig,
    target: &UsageProviderTarget,
) -> String {
    let provider_json = serde_json::to_vec(provider).unwrap_or_default();
    let endpoint_key = target.endpoint.provider_endpoint.stable_key();
    let endpoint_metadata = serde_json::to_vec(&(
        &target.tags,
        &target.supported_models,
        &target.model_mapping,
    ))
    .unwrap_or_default();
    credential_safe_digest(
        b"codex-helper:usage-provider-config:v1\0",
        &[
            provider_json.as_slice(),
            target.base_url.as_bytes(),
            endpoint_key.as_bytes(),
            endpoint_metadata.as_slice(),
        ],
    )
}

fn auto_usage_provider_config_revision(target: &UsageProviderTarget) -> String {
    let endpoint_key = target.endpoint.provider_endpoint.stable_key();
    credential_safe_digest(
        b"codex-helper:auto-usage-provider-config:v1\0",
        &[target.base_url.as_bytes(), endpoint_key.as_bytes()],
    )
}

async fn prepare_provider_poll(
    state: &ProxyState,
    provider: &UsageProviderConfig,
    target: &UsageProviderTarget,
    token: &str,
    adapter_code: &str,
    config_revision_override: Option<&str>,
    observed_at_ms: u64,
) -> Result<PreparedProviderPoll> {
    let endpoint = resolve_endpoint(provider, &target.base_url, token)?;
    let new_api_user_id = resolve_new_api_user_id(provider)?;
    let provider_endpoint = target.endpoint.provider_endpoint.clone();
    let account_fingerprint = usage_provider_account_fingerprint(token, new_api_user_id.as_deref());
    let route_scope = target.runtime_identity().policy_route_scope();
    let scope = match ProviderObservationScope::new(
        provider_endpoint,
        &target.base_url,
        route_scope,
        adapter_code,
        &endpoint,
        account_fingerprint,
        config_revision_override
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| usage_provider_config_revision(provider, target)),
    ) {
        Ok(scope) => scope,
        Err(error) => {
            warn!(error = %error, "failed to build provider observation scope");
            return Ok(PreparedProviderPoll {
                endpoint,
                new_api_user_id,
                reservation: None,
            });
        }
    };
    let reservation = match state
        .reserve_provider_observation(scope, observed_at_ms)
        .await
    {
        Ok(reservation) => Some(reservation),
        Err(error) => {
            warn!(error = %error, "failed to reserve provider observation");
            None
        }
    };
    Ok(PreparedProviderPoll {
        endpoint,
        new_api_user_id,
        reservation,
    })
}

async fn commit_provider_poll_observation(
    state: &ProxyState,
    reservation: Option<ProviderObservationReservation>,
    snapshot: &ProviderBalanceSnapshot,
    suppression: Option<&ProviderTargetSuppressionDecision>,
    completed_at_ms: u64,
) -> bool {
    let Some(reservation) = reservation else {
        return true;
    };
    let status = snapshot.status_at(completed_at_ms);
    let (authority, effect) = match status {
        BalanceSnapshotStatus::Exhausted if snapshot.exhaustion_affects_routing => {
            let reason = suppression
                .map(|decision| decision.reason.clone())
                .unwrap_or_else(|| "balance exhausted".to_string());
            let expires_at_unix_ms = suppression
                .map(|decision| completed_at_ms.saturating_add(duration_millis_u64(decision.ttl)));
            (
                ProviderObservationAuthority::Authoritative,
                ProviderPolicyEffect::Block {
                    action_kind: "balance_exhausted".to_string(),
                    code: Some("balance_exhausted".to_string()),
                    reason,
                    expires_at_unix_ms,
                },
            )
        }
        BalanceSnapshotStatus::Ok if snapshot.exhaustion_affects_routing => (
            ProviderObservationAuthority::Authoritative,
            ProviderPolicyEffect::Recover {
                reason: "authoritative balance available".to_string(),
            },
        ),
        _ => (
            ProviderObservationAuthority::Informational,
            ProviderPolicyEffect::ObserveOnly {
                reason: format!("balance snapshot status {status:?}"),
            },
        ),
    };
    let observation = ProviderObservation {
        observed_at_unix_ms: snapshot.fetched_at_ms,
        completed_at_unix_ms: completed_at_ms.max(snapshot.fetched_at_ms),
        authority,
        evidence: serde_json::to_value(snapshot).unwrap_or_else(|error| {
            serde_json::json!({
                "source": snapshot.source,
                "status": format!("{status:?}"),
                "serialization_error": error.to_string(),
            })
        }),
        effect,
    };
    if let Err(error) = state
        .commit_provider_observation(reservation, observation)
        .await
    {
        warn!(error = %error, "failed to commit provider balance observation");
        return false;
    }
    true
}

fn endpoint_origin(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?;
            let origin = match url.port() {
                Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
                None => format!("{}://{}", url.scheme(), host),
            };
            Some(origin)
        })
        .unwrap_or_else(|| "unknown-origin".to_string())
}

fn provider_request_headers(
    provider: &UsageProviderConfig,
    token: &str,
    new_api_user_id: Option<&str>,
) -> Result<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    match provider.kind {
        ProviderKind::YescodeProfile => {
            let value = reqwest::header::HeaderValue::from_str(token).with_context(|| {
                format!("usage provider '{}' has an invalid API key", provider.id)
            })?;
            headers.insert("X-API-Key", value);
        }
        _ => {
            let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .with_context(|| {
                    format!(
                        "usage provider '{}' has an invalid bearer token",
                        provider.id
                    )
                })?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    }

    if let Some(user_id) = new_api_user_id {
        if !matches!(provider.kind, ProviderKind::NewApiUserSelf) {
            anyhow::bail!(
                "usage provider '{}' cannot send New-Api-User for kind {:?}",
                provider.id,
                provider.kind
            );
        }
        let value = reqwest::header::HeaderValue::from_str(user_id).with_context(|| {
            format!(
                "usage provider '{}' has an invalid New-Api-User value",
                provider.id
            )
        })?;
        headers.insert("New-Api-User", value);
    }
    Ok(headers)
}

async fn poll_provider_http_json(
    client: &Client,
    provider: &UsageProviderConfig,
    endpoint: &str,
    new_api_user_id: Option<&str>,
    token: &str,
) -> Result<serde_json::Value> {
    let origin = endpoint_origin(endpoint);
    let mut req = client
        .get(endpoint)
        .timeout(BALANCE_HTTP_REQUEST_TIMEOUT)
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            concat!("codex-helper/", env!("CARGO_PKG_VERSION")),
        );

    req = req.headers(provider_request_headers(provider, token, new_api_user_id)?);

    let resp = req.send().await.with_context(|| {
        format!(
            "usage provider request failed for {} via {:?}",
            origin, provider.kind
        )
    })?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    if !status.is_success() {
        let text = resp.text().await.with_context(|| {
            format!(
                "usage provider error response read failed from {} via {:?}",
                origin, provider.kind
            )
        })?;
        let detail = usage_provider_http_error_detail(&text)
            .map(|detail| format!(": {detail}"))
            .unwrap_or_default();
        anyhow::bail!(
            "usage provider HTTP {} from {} via {:?}{}",
            status,
            origin,
            provider.kind,
            detail
        );
    }
    let text = resp.text().await.with_context(|| {
        format!(
            "usage provider response read failed from {} via {:?}",
            origin, provider.kind
        )
    })?;
    serde_json::from_str(&text).with_context(|| {
        format!(
            "usage provider returned non-JSON response from {} via {:?} (content-type {}, {} bytes)",
            origin,
            provider.kind,
            content_type,
            text.len()
        )
    })
}

fn truncate_error_detail(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn compact_error_detail(value: &str) -> Option<String> {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(truncate_error_detail(
            &compact,
            BALANCE_HTTP_ERROR_BODY_LIMIT,
        ))
    }
}

fn json_error_detail(value: &serde_json::Value) -> Option<String> {
    let code = first_string_from_paths(
        value,
        &[
            "code",
            "error.code",
            "error.type",
            "type",
            "data.code",
            "data.error.code",
        ],
    );
    let message = first_string_from_paths(
        value,
        &[
            "message",
            "msg",
            "detail",
            "error.message",
            "error_description",
            "error",
            "data.message",
            "data.error.message",
        ],
    );

    match (code, message) {
        (Some(code), Some(message)) if !message.eq_ignore_ascii_case(&code) => {
            Some(format!("{code}: {message}"))
        }
        (Some(code), _) => Some(code),
        (_, Some(message)) => Some(message),
        _ => None,
    }
}

fn usage_provider_http_error_detail(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(detail) = json_error_detail(&value)
    {
        return compact_error_detail(&detail);
    }

    compact_error_detail(trimmed)
}

fn amount_from_json(value: &serde_json::Value) -> Option<UsdAmount> {
    let raw = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_string(),
        _ => return None,
    };
    UsdAmount::from_decimal_str(raw.as_str())
}

fn decimal_string_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        _ => None,
    }
}

fn amount_from_json_with_divisor(
    value: &serde_json::Value,
    divisor: Option<u64>,
) -> Option<UsdAmount> {
    let amount = amount_from_json(value)?;
    match divisor {
        Some(divisor) => amount.checked_div_u64(divisor),
        None => Some(amount),
    }
}

fn json_value_at_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path
        .split('.')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
    {
        current = match current {
            serde_json::Value::Array(items) => {
                let index = segment.parse::<usize>().ok()?;
                items.get(index)?
            }
            _ => current.get(segment)?,
        };
    }
    Some(current)
}

fn first_amount_from_paths(
    value: &serde_json::Value,
    custom_paths: &[String],
    default_paths: &[&str],
    divisor: Option<u64>,
) -> Option<UsdAmount> {
    custom_paths
        .iter()
        .map(String::as_str)
        .chain(default_paths.iter().copied())
        .find_map(|path| {
            json_value_at_path(value, path)
                .and_then(|value| amount_from_json_with_divisor(value, divisor))
        })
}

fn bool_from_json(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::Number(number) => number.as_i64().map(|value| value != 0),
        serde_json::Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" | "exhausted" => Some(true),
            "false" | "no" | "0" | "ok" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn first_bool_from_paths(
    value: &serde_json::Value,
    custom_paths: &[String],
    default_paths: &[&str],
) -> Option<bool> {
    custom_paths
        .iter()
        .map(String::as_str)
        .chain(default_paths.iter().copied())
        .find_map(|path| json_value_at_path(value, path).and_then(bool_from_json))
}

fn first_decimal_string_from_paths(
    value: &serde_json::Value,
    default_paths: &[&str],
) -> Option<String> {
    default_paths
        .iter()
        .copied()
        .find_map(|path| json_value_at_path(value, path).and_then(decimal_string_from_json))
}

fn string_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        _ => None,
    }
}

fn first_string_from_paths(value: &serde_json::Value, default_paths: &[&str]) -> Option<String> {
    default_paths
        .iter()
        .copied()
        .find_map(|path| json_value_at_path(value, path).and_then(string_from_json))
}

fn u64_from_json(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                text.parse::<u64>().ok()
            }
        }
        _ => None,
    }
}

fn seconds_from_json(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64().map(|value| value.max(0.0) as u64),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else if let Ok(value) = text.parse::<f64>() {
                Some(value.max(0.0) as u64)
            } else {
                parse_timestamp_secs(text)
            }
        }
        _ => None,
    }
}

fn first_secs_from_paths(value: &serde_json::Value, default_paths: &[&str]) -> Option<u64> {
    default_paths
        .iter()
        .copied()
        .find_map(|path| json_value_at_path(value, path).and_then(seconds_from_json))
}

fn parse_timestamp_secs(value: &str) -> Option<u64> {
    parse_rfc3339_like_secs(value).or_else(|| {
        httpdate::parse_http_date(value).ok().and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
        })
    })
}

fn parse_rfc3339_like_secs(value: &str) -> Option<u64> {
    let value = value.trim();
    let datetime_sep = value.find('T').or_else(|| value.find(' '))?;
    let (datetime, offset_secs) = if let Some(datetime) = value.strip_suffix('Z') {
        (datetime, 0_i64)
    } else {
        let offset_pos = value[datetime_sep + 1..]
            .rfind(['+', '-'])
            .map(|pos| datetime_sep + 1 + pos)?;
        let (datetime, offset) = value.split_at(offset_pos);
        (datetime, parse_rfc3339_offset_secs(offset)?)
    };

    let (date, time) = datetime.split_at(datetime_sep);
    let time = time.get(1..)?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u32>().ok()?;
    let minute = time_parts.next()?.parse::<u32>().ok()?;
    let second_raw = time_parts.next().unwrap_or("0");
    if time_parts.next().is_some() {
        return None;
    }
    let second = second_raw
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())?;
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let local_secs = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    local_secs
        .checked_sub(offset_secs)
        .and_then(|utc_secs| u64::try_from(utc_secs).ok())
}

fn parse_rfc3339_offset_secs(offset: &str) -> Option<i64> {
    let sign = match offset.as_bytes().first().copied()? {
        b'+' => 1_i64,
        b'-' => -1_i64,
        _ => return None,
    };
    let raw = offset.get(1..)?;
    let (hours, minutes) = raw
        .split_once(':')
        .unwrap_or_else(|| raw.split_at(raw.len().min(2)));
    let hours = hours.parse::<i64>().ok()?;
    let minutes = minutes.parse::<i64>().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn first_u64_from_paths(value: &serde_json::Value, default_paths: &[&str]) -> Option<u64> {
    default_paths
        .iter()
        .copied()
        .find_map(|path| json_value_at_path(value, path).and_then(u64_from_json))
}

fn array_from_json_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    json_value_at_path(value, path).and_then(|value| value.as_array())
}

fn amount_to_string(amount: UsdAmount) -> String {
    amount.format_usd()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuotaWindowSnapshot {
    period: &'static str,
    remaining: UsdAmount,
    used: UsdAmount,
    limit: UsdAmount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RateLimitWindowSnapshot {
    period: String,
    reset_at_ms: Option<u64>,
}

fn base_snapshot(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let mut snapshot = ProviderBalanceSnapshot::new(
        provider.id.clone(),
        upstream.provider_endpoint.clone(),
        provider.kind.source_name(),
        fetched_at_ms,
        stale_after_ms,
    );
    snapshot.exhaustion_affects_routing = provider.trust_exhaustion_for_routing;
    snapshot
}

fn snapshot_error(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
    message: impl Into<String>,
) -> ProviderBalanceSnapshot {
    base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms).with_error(message)
}

fn budget_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let monthly_budget = first_amount_from_paths(
        value,
        &provider.extract.monthly_budget_paths,
        &["monthly_budget_usd", "data.monthly_budget_usd"],
        provider.extract.monthly_budget_divisor,
    );
    let monthly_spent = first_amount_from_paths(
        value,
        &provider.extract.monthly_spent_paths,
        &["monthly_spent_usd", "data.monthly_spent_usd"],
        provider.extract.monthly_spent_divisor,
    );
    let exhausted = match (monthly_budget, monthly_spent) {
        (Some(budget), Some(spent)) if !budget.is_zero() => Some(spent >= budget),
        (Some(_), Some(_)) => Some(false),
        _ => None,
    };

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.monthly_budget_usd = monthly_budget.map(amount_to_string);
    snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    snapshot.exhausted = exhausted;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn yescode_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let subscription_balance = first_amount_from_paths(
        value,
        &provider.extract.subscription_balance_paths,
        &["subscription_balance", "data.subscription_balance"],
        provider.extract.remaining_divisor,
    );
    let paygo_balance = first_amount_from_paths(
        value,
        &provider.extract.paygo_balance_paths,
        &[
            "pay_as_you_go_balance",
            "paygo_balance",
            "data.pay_as_you_go_balance",
            "data.paygo_balance",
        ],
        provider.extract.remaining_divisor,
    );
    let total_balance = match (subscription_balance, paygo_balance) {
        (Some(subscription), Some(paygo)) => Some(subscription.saturating_add(paygo)),
        (Some(subscription), None) => Some(subscription),
        (None, Some(paygo)) => Some(paygo),
        (None, None) => None,
    };

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.total_balance_usd = total_balance.map(amount_to_string);
    snapshot.subscription_balance_usd = subscription_balance.map(amount_to_string);
    snapshot.paygo_balance_usd = paygo_balance.map(amount_to_string);
    snapshot.exhausted = total_balance.map(UsdAmount::is_zero);
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn balance_http_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let remaining_balance = first_amount_from_paths(
        value,
        &provider.extract.remaining_balance_paths,
        &[
            "balance",
            "remaining",
            "remain",
            "available",
            "available_balance",
            "credit",
            "credits",
            "total_balance",
            "total_balance_usd",
            "totalBalance",
            "availableBalance",
            "available_balance_usd",
            "balance_infos.0.total_balance",
            "data.balance",
            "data.remaining",
            "data.available",
            "data.available_balance",
            "data.credit",
            "data.credits",
            "data.total_balance",
            "data.totalBalance",
            "data.availableBalance",
        ],
        provider.extract.remaining_divisor,
    );
    let subscription_balance = first_amount_from_paths(
        value,
        &provider.extract.subscription_balance_paths,
        &[
            "subscription_balance",
            "subscription_balance_usd",
            "subscriptionBalance",
            "data.subscription_balance",
            "data.subscription_balance_usd",
            "data.subscriptionBalance",
        ],
        provider.extract.remaining_divisor,
    );
    let paygo_balance = first_amount_from_paths(
        value,
        &provider.extract.paygo_balance_paths,
        &[
            "pay_as_you_go_balance",
            "paygo_balance",
            "paygo",
            "paygoBalance",
            "chargeBalance",
            "voucherBalance",
            "data.pay_as_you_go_balance",
            "data.paygo_balance",
            "data.paygo",
            "data.paygoBalance",
            "data.chargeBalance",
            "data.voucherBalance",
        ],
        provider.extract.remaining_divisor,
    );
    let component_remaining = match (subscription_balance, paygo_balance) {
        (Some(subscription), Some(paygo)) => Some(subscription.saturating_add(paygo)),
        (Some(subscription), None) => Some(subscription),
        (None, Some(paygo)) => Some(paygo),
        (None, None) => None,
    };
    let monthly_spent = first_amount_from_paths(
        value,
        &provider.extract.monthly_spent_paths,
        &[
            "monthly_spent_usd",
            "spent",
            "used",
            "used_balance",
            "usedBalance",
            "total_usage",
            "data.monthly_spent_usd",
            "data.spent",
            "data.used",
            "data.used_balance",
            "data.usedBalance",
            "data.total_usage",
        ],
        provider.extract.monthly_spent_divisor,
    );
    let monthly_budget = first_amount_from_paths(
        value,
        &provider.extract.monthly_budget_paths,
        &[
            "monthly_budget_usd",
            "budget",
            "limit",
            "quota_total",
            "creditLimit",
            "total_credits",
            "data.monthly_budget_usd",
            "data.budget",
            "data.limit",
            "data.quota_total",
            "data.creditLimit",
            "data.total_credits",
        ],
        provider.extract.monthly_budget_divisor,
    )
    .or_else(|| {
        if provider.extract.derive_budget_from_remaining_and_spent {
            match (remaining_balance.or(component_remaining), monthly_spent) {
                (Some(remaining), Some(spent)) => Some(remaining.saturating_add(spent)),
                _ => None,
            }
        } else {
            None
        }
    });
    let total_balance = remaining_balance.or(component_remaining).or_else(|| {
        match (
            provider.extract.derive_remaining_from_budget_and_spent,
            monthly_budget,
            monthly_spent,
        ) {
            (true, Some(budget), Some(spent)) => Some(budget.saturating_sub(spent)),
            _ => None,
        }
    });
    let exhausted = first_bool_from_paths(
        value,
        &provider.extract.exhausted_paths,
        &[
            "exhausted",
            "quota_exhausted",
            "balance_exhausted",
            "data.exhausted",
            "data.quota_exhausted",
            "data.balance_exhausted",
        ],
    )
    .or_else(|| total_balance.map(UsdAmount::is_zero));

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.total_balance_usd = total_balance.map(amount_to_string);
    snapshot.subscription_balance_usd = subscription_balance.map(amount_to_string);
    snapshot.paygo_balance_usd = paygo_balance.map(amount_to_string);
    snapshot.monthly_budget_usd = monthly_budget.map(amount_to_string);
    snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    snapshot.exhausted = exhausted;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn has_any_json_path(value: &serde_json::Value, paths: &[&str]) -> bool {
    paths
        .iter()
        .any(|path| json_value_at_path(value, path).is_some())
}

fn populate_sub2api_usage_fields(
    snapshot: &mut ProviderBalanceSnapshot,
    value: &serde_json::Value,
) {
    snapshot.plan_name = first_string_from_paths(
        value,
        &["planName", "plan_name", "data.planName", "data.plan_name"],
    );
    let remaining_balance = sub2api_remaining_balance(value);
    snapshot.total_balance_usd = snapshot
        .total_balance_usd
        .take()
        .or_else(|| remaining_balance.map(amount_to_string));
    snapshot.total_used_usd = first_amount_from_paths(
        value,
        &[],
        &[
            "usage.total.total_cost_usd",
            "usage.total.total_cost",
            "usage.total.cost",
            "data.usage.total.total_cost_usd",
            "data.usage.total.total_cost",
            "data.usage.total.cost",
        ],
        None,
    )
    .map(amount_to_string);
    snapshot.today_used_usd = first_amount_from_paths(
        value,
        &[],
        &[
            "usage.today.total_cost_usd",
            "usage.today.total_cost",
            "usage.today.cost",
            "data.usage.today.total_cost_usd",
            "data.usage.today.total_cost",
            "data.usage.today.cost",
        ],
        None,
    )
    .map(amount_to_string);
    snapshot.total_requests = first_u64_from_paths(
        value,
        &[
            "usage.total.request_count",
            "usage.total.requests",
            "usage.total.count",
            "data.usage.total.request_count",
            "data.usage.total.requests",
            "data.usage.total.count",
        ],
    );
    snapshot.today_requests = first_u64_from_paths(
        value,
        &[
            "usage.today.request_count",
            "usage.today.requests",
            "usage.today.count",
            "data.usage.today.request_count",
            "data.usage.today.requests",
            "data.usage.today.count",
        ],
    );
    snapshot.total_tokens = first_u64_from_paths(
        value,
        &[
            "usage.total.total_tokens",
            "usage.total.tokens",
            "usage.total.input_tokens",
            "usage.total.prompt_tokens",
            "data.usage.total.total_tokens",
            "data.usage.total.tokens",
        ],
    );
    snapshot.today_tokens = first_u64_from_paths(
        value,
        &[
            "usage.today.total_tokens",
            "usage.today.tokens",
            "usage.today.input_tokens",
            "usage.today.prompt_tokens",
            "data.usage.today.total_tokens",
            "data.usage.today.tokens",
        ],
    );
    snapshot.usage_rate = sub2api_usage_rate(value);
    snapshot.usage_windows = sub2api_usage_windows(value);
    snapshot.usage_model_stats = sub2api_model_stats(value);
    snapshot.subscription_expires_at = first_string_from_paths(
        value,
        &[
            "subscription.expires_at",
            "data.subscription.expires_at",
            "subscription.expiresAt",
            "data.subscription.expiresAt",
        ],
    );
    snapshot.usage_alerts = sub2api_usage_alerts(value);
}

fn sub2api_remaining_balance(value: &serde_json::Value) -> Option<UsdAmount> {
    let remaining = first_amount_from_paths(value, &[], &["remaining", "data.remaining"], None)?;
    if sub2api_has_subscription_windows(value)
        && sub2api_window_remaining_amounts(value).contains(&remaining)
    {
        return None;
    }
    Some(remaining)
}

fn sub2api_has_subscription_windows(value: &serde_json::Value) -> bool {
    has_any_json_path(
        value,
        &[
            "subscription.daily_usage_usd",
            "subscription.daily_limit_usd",
            "subscription.weekly_usage_usd",
            "subscription.weekly_limit_usd",
            "subscription.monthly_usage_usd",
            "subscription.monthly_limit_usd",
            "data.subscription.daily_usage_usd",
            "data.subscription.daily_limit_usd",
            "data.subscription.weekly_usage_usd",
            "data.subscription.weekly_limit_usd",
            "data.subscription.monthly_usage_usd",
            "data.subscription.monthly_limit_usd",
        ],
    )
}

fn sub2api_window_remaining_amounts(value: &serde_json::Value) -> Vec<UsdAmount> {
    ["daily", "weekly", "monthly"]
        .into_iter()
        .filter_map(|period| {
            let used = first_amount_from_paths(
                value,
                &[],
                &[
                    &format!("subscription.{period}_usage_usd"),
                    &format!("data.subscription.{period}_usage_usd"),
                ],
                None,
            );
            let limit = first_amount_from_paths(
                value,
                &[],
                &[
                    &format!("subscription.{period}_limit_usd"),
                    &format!("data.subscription.{period}_limit_usd"),
                ],
                None,
            );
            match (limit, used) {
                (Some(limit), Some(used)) if !limit.is_zero() => Some(limit.saturating_sub(used)),
                _ => None,
            }
        })
        .collect()
}

fn optional_amount_is_zero(value: Option<UsdAmount>) -> bool {
    value.map(UsdAmount::is_zero).unwrap_or(true)
}

fn optional_u64_is_zero(value: Option<u64>) -> bool {
    value.unwrap_or(0) == 0
}

fn sub2api_today_usage_is_zero(value: &serde_json::Value) -> bool {
    let today_cost = first_amount_from_paths(
        value,
        &[],
        &[
            "usage.today.actual_cost",
            "usage.today.total_cost_usd",
            "usage.today.total_cost",
            "usage.today.cost",
            "data.usage.today.actual_cost",
            "data.usage.today.total_cost_usd",
            "data.usage.today.total_cost",
            "data.usage.today.cost",
        ],
        None,
    );
    let today_requests = first_u64_from_paths(
        value,
        &[
            "usage.today.request_count",
            "usage.today.requests",
            "usage.today.count",
            "data.usage.today.request_count",
            "data.usage.today.requests",
            "data.usage.today.count",
        ],
    );
    let today_tokens = first_u64_from_paths(
        value,
        &[
            "usage.today.total_tokens",
            "usage.today.tokens",
            "usage.today.input_tokens",
            "usage.today.prompt_tokens",
            "data.usage.today.total_tokens",
            "data.usage.today.tokens",
        ],
    );
    let has_today_usage_data =
        today_cost.is_some() || today_requests.is_some() || today_tokens.is_some();
    has_today_usage_data
        && optional_amount_is_zero(today_cost)
        && optional_u64_is_zero(today_requests)
        && optional_u64_is_zero(today_tokens)
}

fn sub2api_daily_subscription_usage_is_lazy_stale(value: &serde_json::Value) -> bool {
    if first_string_from_paths(value, &["mode", "data.mode"]).as_deref() != Some("unrestricted") {
        return false;
    }

    let used = first_amount_from_paths(
        value,
        &[],
        &[
            "subscription.daily_usage_usd",
            "data.subscription.daily_usage_usd",
        ],
        None,
    );
    let limit = first_amount_from_paths(
        value,
        &[],
        &[
            "subscription.daily_limit_usd",
            "data.subscription.daily_limit_usd",
        ],
        None,
    );

    matches!(
        (used, limit),
        (Some(used), Some(limit))
            if !limit.is_zero() && used >= limit && sub2api_today_usage_is_zero(value)
    )
}

fn sub2api_usage_rate(value: &serde_json::Value) -> Option<ProviderUsageRateSnapshot> {
    let rate = ProviderUsageRateSnapshot {
        average_duration_ms: first_decimal_string_from_paths(
            value,
            &[
                "usage.average_duration_ms",
                "data.usage.average_duration_ms",
                "average_duration_ms",
                "data.average_duration_ms",
            ],
        ),
        rpm: first_decimal_string_from_paths(value, &["usage.rpm", "data.usage.rpm", "rpm"]),
        tpm: first_decimal_string_from_paths(value, &["usage.tpm", "data.usage.tpm", "tpm"]),
    };
    (!rate.is_empty()).then_some(rate)
}

fn sub2api_usage_windows(value: &serde_json::Value) -> Vec<ProviderUsageWindow> {
    ["daily", "weekly", "monthly"]
        .into_iter()
        .filter_map(|period| {
            let used = if period == "daily" && sub2api_daily_subscription_usage_is_lazy_stale(value)
            {
                Some(UsdAmount::ZERO)
            } else {
                first_amount_from_paths(
                    value,
                    &[],
                    &[
                        &format!("subscription.{period}_usage_usd"),
                        &format!("data.subscription.{period}_usage_usd"),
                    ],
                    None,
                )
            };
            let limit = first_amount_from_paths(
                value,
                &[],
                &[
                    &format!("subscription.{period}_limit_usd"),
                    &format!("data.subscription.{period}_limit_usd"),
                ],
                None,
            );
            if used.is_none() && limit.is_none() {
                return None;
            }
            let unlimited = limit.map(|limit| limit.is_zero());
            let remaining = match (limit, used) {
                (Some(limit), Some(used)) if !limit.is_zero() => Some(limit.saturating_sub(used)),
                _ => None,
            };
            Some(ProviderUsageWindow {
                period: period.to_string(),
                used_usd: used.map(amount_to_string),
                limit_usd: limit.map(amount_to_string),
                remaining_usd: remaining.map(amount_to_string),
                unlimited,
            })
        })
        .collect()
}

fn sub2api_rate_limit_window_from_json(
    value: &serde_json::Value,
) -> Option<RateLimitWindowSnapshot> {
    let period = first_string_from_paths(value, &["window", "period", "name"])?;
    let limit = first_u64_from_paths(value, &["limit"]);
    if limit == Some(0) {
        return None;
    }
    let remaining = first_u64_from_paths(value, &["remaining"])?;
    if remaining > 0 {
        return None;
    }
    let reset_at_ms = first_secs_from_paths(value, &["reset_at", "resets_at", "resetAt"])
        .map(|secs| secs.saturating_mul(1000));
    Some(RateLimitWindowSnapshot {
        period: format!("rate_limit:{period}"),
        reset_at_ms,
    })
}

fn sub2api_limiting_rate_limit_window(
    value: &serde_json::Value,
) -> Option<RateLimitWindowSnapshot> {
    ["rate_limits", "data.rate_limits"]
        .into_iter()
        .find_map(|path| array_from_json_path(value, path))
        .and_then(|items| {
            items
                .iter()
                .filter_map(sub2api_rate_limit_window_from_json)
                .max_by_key(|window| window.reset_at_ms.unwrap_or(0))
        })
}

fn sub2api_model_stats(value: &serde_json::Value) -> Vec<ProviderUsageModelStat> {
    [
        "model_stats",
        "data.model_stats",
        "modelStats",
        "data.modelStats",
    ]
    .into_iter()
    .find_map(|path| array_from_json_path(value, path))
    .map(|items| {
        items
            .iter()
            .filter_map(sub2api_model_stat_from_json)
            .collect::<Vec<_>>()
    })
    .unwrap_or_default()
}

fn sub2api_model_stat_from_json(value: &serde_json::Value) -> Option<ProviderUsageModelStat> {
    let model = first_string_from_paths(value, &["model", "model_name", "name"])?;
    let input_cost = first_amount_from_paths(value, &[], &["input_cost_usd", "input_cost"], None);
    let output_cost =
        first_amount_from_paths(value, &[], &["output_cost_usd", "output_cost"], None);
    let total_cost =
        first_amount_from_paths(value, &[], &["total_cost_usd", "total_cost", "cost"], None)
            .or_else(|| match (input_cost, output_cost) {
                (Some(input), Some(output)) => Some(input.saturating_add(output)),
                _ => None,
            });
    let input_tokens = first_u64_from_paths(value, &["input_tokens", "prompt_tokens"]);
    let output_tokens = first_u64_from_paths(value, &["output_tokens", "completion_tokens"]);
    let total_tokens =
        first_u64_from_paths(value, &["total_tokens", "tokens"]).or_else(|| {
            match (input_tokens, output_tokens) {
                (Some(input), Some(output)) => input.checked_add(output),
                _ => None,
            }
        });
    Some(ProviderUsageModelStat {
        model,
        request_count: first_u64_from_paths(value, &["request_count", "requests", "count"]),
        input_tokens,
        output_tokens,
        total_tokens,
        input_cost_usd: input_cost.map(amount_to_string),
        output_cost_usd: output_cost.map(amount_to_string),
        total_cost_usd: total_cost.map(amount_to_string),
    })
}

fn sub2api_usage_alerts(value: &serde_json::Value) -> Vec<ProviderUsageAlert> {
    let mut alerts = Vec::new();
    if let (Some(used), Some(limit)) = (
        first_amount_from_paths(
            value,
            &[],
            &[
                "subscription.daily_usage_usd",
                "data.subscription.daily_usage_usd",
            ],
            None,
        ),
        first_amount_from_paths(
            value,
            &[],
            &[
                "subscription.daily_limit_usd",
                "data.subscription.daily_limit_usd",
            ],
            None,
        ),
    ) && !limit.is_zero()
    {
        let used = if sub2api_daily_subscription_usage_is_lazy_stale(value) {
            UsdAmount::ZERO
        } else {
            used
        };
        let used_femto = used.femto_usd();
        let limit_femto = limit.femto_usd();
        if used_femto.saturating_mul(100) >= limit_femto.saturating_mul(95) {
            alerts.push(ProviderUsageAlert {
                kind: ProviderUsageAlertKind::DailyUsage95,
                message: "daily usage is at or above 95%".to_string(),
            });
        } else if used_femto.saturating_mul(100) >= limit_femto.saturating_mul(80) {
            alerts.push(ProviderUsageAlert {
                kind: ProviderUsageAlertKind::DailyUsage80,
                message: "daily usage is at or above 80%".to_string(),
            });
        }
    }

    if let Some(remaining) = sub2api_remaining_balance(value)
        && let Some(threshold) = UsdAmount::from_decimal_str(LOW_BALANCE_ALERT_THRESHOLD_USD)
        && remaining <= threshold
    {
        alerts.push(ProviderUsageAlert {
            kind: ProviderUsageAlertKind::LowBalance,
            message: "remaining balance is low".to_string(),
        });
    }

    if let Some(expires_at_secs) = first_secs_from_paths(
        value,
        &[
            "subscription.expires_at",
            "data.subscription.expires_at",
            "subscription.expiresAt",
            "data.subscription.expiresAt",
        ],
    ) {
        let now = unix_now_secs();
        if expires_at_secs <= now {
            alerts.push(ProviderUsageAlert {
                kind: ProviderUsageAlertKind::SubscriptionExpired,
                message: "subscription has expired".to_string(),
            });
        } else if expires_at_secs <= now.saturating_add(EXPIRING_SOON_WINDOW_SECS) {
            alerts.push(ProviderUsageAlert {
                kind: ProviderUsageAlertKind::SubscriptionExpiringSoon,
                message: "subscription expires within 7 days".to_string(),
            });
        }
    }

    alerts.sort_by_key(|alert| alert.kind);
    alerts.dedup_by_key(|alert| alert.kind);
    alerts
}

fn sub2api_subscription_limit_snapshot(
    value: &serde_json::Value,
    period: &'static str,
    limit_paths: &[&str],
    usage_paths: &[&str],
) -> Option<QuotaWindowSnapshot> {
    let budget = first_amount_from_paths(value, &[], limit_paths, None)?;
    if budget.is_zero() {
        return None;
    }
    let spent = if period == "daily" && sub2api_daily_subscription_usage_is_lazy_stale(value) {
        UsdAmount::ZERO
    } else {
        first_amount_from_paths(value, &[], usage_paths, None).unwrap_or(UsdAmount::ZERO)
    };
    let remaining = budget.saturating_sub(spent);
    Some(QuotaWindowSnapshot {
        period,
        remaining,
        used: spent,
        limit: budget,
    })
}

fn sub2api_limiting_subscription_window(value: &serde_json::Value) -> Option<QuotaWindowSnapshot> {
    let windows = [
        sub2api_subscription_limit_snapshot(
            value,
            "daily",
            &[
                "subscription.daily_limit_usd",
                "data.subscription.daily_limit_usd",
            ],
            &[
                "subscription.daily_usage_usd",
                "data.subscription.daily_usage_usd",
            ],
        ),
        sub2api_subscription_limit_snapshot(
            value,
            "weekly",
            &[
                "subscription.weekly_limit_usd",
                "data.subscription.weekly_limit_usd",
            ],
            &[
                "subscription.weekly_usage_usd",
                "data.subscription.weekly_usage_usd",
            ],
        ),
        sub2api_subscription_limit_snapshot(
            value,
            "monthly",
            &[
                "subscription.monthly_limit_usd",
                "data.subscription.monthly_limit_usd",
            ],
            &[
                "subscription.monthly_usage_usd",
                "data.subscription.monthly_usage_usd",
            ],
        ),
    ];

    windows
        .into_iter()
        .flatten()
        .min_by_key(|window| window.remaining)
}

fn sub2api_usage_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    if json_value_at_path(value, "isValid").and_then(bool_from_json) == Some(false) {
        return base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms)
            .with_error("sub2api usage response reported invalid API key");
    }

    let mode = first_string_from_paths(value, &["mode", "data.mode"]);
    let has_subscription = has_any_json_path(value, &["subscription", "data.subscription"]);

    if mode.as_deref() == Some("quota_limited") {
        let rate_limit_window = sub2api_limiting_rate_limit_window(value);
        let quota_remaining = first_amount_from_paths(
            value,
            &provider.extract.remaining_balance_paths,
            &[
                "quota.remaining",
                "data.quota.remaining",
                "remaining",
                "data.remaining",
            ],
            provider.extract.remaining_divisor,
        );
        let quota_limit = first_amount_from_paths(
            value,
            &provider.extract.monthly_budget_paths,
            &["quota.limit", "data.quota.limit"],
            provider.extract.monthly_budget_divisor,
        );
        let quota_used = first_amount_from_paths(
            value,
            &provider.extract.monthly_spent_paths,
            &["quota.used", "data.quota.used"],
            provider.extract.monthly_spent_divisor,
        );
        let quota_exhausted = first_bool_from_paths(
            value,
            &provider.extract.exhausted_paths,
            &[
                "exhausted",
                "data.exhausted",
                "quota_exhausted",
                "data.quota_exhausted",
            ],
        )
        .or_else(|| quota_remaining.map(UsdAmount::is_zero));
        let exhausted = Some(quota_exhausted.unwrap_or(false) || rate_limit_window.is_some());

        let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
        if let Some(rate_limit_window) = rate_limit_window.clone()
            && quota_exhausted != Some(true)
        {
            snapshot.quota_period = Some(rate_limit_window.period);
            snapshot.quota_resets_at_ms = rate_limit_window.reset_at_ms;
        } else {
            snapshot.quota_period = Some("quota".to_string());
            snapshot.quota_remaining_usd = quota_remaining.map(amount_to_string);
            snapshot.quota_limit_usd = quota_limit.map(amount_to_string);
            snapshot.quota_used_usd = quota_used.map(amount_to_string);
            snapshot.monthly_budget_usd = quota_limit.map(amount_to_string);
            snapshot.monthly_spent_usd = quota_used.map(amount_to_string);
        }
        snapshot.exhausted = exhausted;
        populate_sub2api_usage_fields(&mut snapshot, value);
        snapshot.refresh_status(fetched_at_ms);
        return snapshot;
    }

    if mode.as_deref() == Some("unrestricted") && has_subscription {
        let limiting_window = sub2api_limiting_subscription_window(value);
        let exhausted = first_bool_from_paths(
            value,
            &provider.extract.exhausted_paths,
            &[
                "exhausted",
                "data.exhausted",
                "quota_exhausted",
                "data.quota_exhausted",
            ],
        )
        .or_else(|| limiting_window.map(|window| window.remaining.is_zero()))
        .or(Some(false));

        let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
        if let Some(window) = limiting_window {
            snapshot.quota_period = Some(window.period.to_string());
            snapshot.quota_remaining_usd = Some(amount_to_string(window.remaining));
            snapshot.quota_limit_usd = Some(amount_to_string(window.limit));
            snapshot.quota_used_usd = Some(amount_to_string(window.used));
            snapshot.monthly_budget_usd = Some(amount_to_string(window.limit));
            snapshot.monthly_spent_usd = Some(amount_to_string(window.used));
        }
        snapshot.exhaustion_affects_routing = false;
        snapshot.exhausted = exhausted;
        populate_sub2api_usage_fields(&mut snapshot, value);
        snapshot.refresh_status(fetched_at_ms);
        return snapshot;
    }

    let mut snapshot =
        balance_http_snapshot_from_json(provider, upstream, value, fetched_at_ms, stale_after_ms);
    populate_sub2api_usage_fields(&mut snapshot, value);
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn sub2api_auth_me_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    if json_value_at_path(value, "code")
        .and_then(|value| value.as_i64())
        .is_some_and(|code| code != 0)
    {
        let message = json_value_at_path(value, "message")
            .and_then(|value| value.as_str())
            .unwrap_or("sub2api auth/me response reported failure");
        return base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms)
            .with_error(message.to_string());
    }

    balance_http_snapshot_from_json(provider, upstream, value, fetched_at_ms, stale_after_ms)
}

fn rightcode_available_prefixes(value: &serde_json::Value) -> Vec<String> {
    array_from_json_path(value, "available_prefixes")
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn rightcode_subscription_window(value: &serde_json::Value) -> Option<QuotaWindowSnapshot> {
    let limit = json_value_at_path(value, "total_quota").and_then(amount_from_json)?;
    if limit < UsdAmount::from_decimal_str("10").unwrap_or(UsdAmount::ZERO) {
        return None;
    }
    let raw_remaining = json_value_at_path(value, "remaining_quota").and_then(amount_from_json)?;
    let reset_today = json_value_at_path(value, "reset_today").and_then(bool_from_json);
    let remaining = if reset_today == Some(true) {
        raw_remaining
    } else {
        raw_remaining.saturating_add(limit)
    };
    let used = limit.saturating_sub(remaining);
    Some(QuotaWindowSnapshot {
        period: "daily",
        remaining,
        used,
        limit,
    })
}

fn rightcode_account_summary_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    upstream_base_url: &str,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let balance = json_value_at_path(value, "balance").and_then(amount_from_json);
    let provider_prefixes = base_path_prefixes(upstream_base_url);
    let mut matched_windows = Vec::new();
    let mut matched_plan_names = Vec::new();

    if let Some(subscriptions) = array_from_json_path(value, "subscriptions") {
        for subscription in subscriptions {
            let available_prefixes = rightcode_available_prefixes(subscription);
            if !path_prefixes_match(&provider_prefixes, &available_prefixes) {
                continue;
            }
            let Some(window) = rightcode_subscription_window(subscription) else {
                continue;
            };
            matched_windows.push(window);
            if let Some(name) = json_value_at_path(subscription, "name").and_then(string_from_json)
            {
                matched_plan_names.push(name);
            }
        }
    }

    if balance.is_none() && matched_windows.is_empty() {
        return snapshot_error(
            provider,
            upstream,
            fetched_at_ms,
            stale_after_ms,
            "rightcode account summary missing balance and matching subscription quota fields",
        );
    }

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.total_balance_usd = balance.map(amount_to_string);

    if !matched_windows.is_empty() {
        let mut remaining = UsdAmount::ZERO;
        let mut used = UsdAmount::ZERO;
        let mut limit = UsdAmount::ZERO;
        for window in matched_windows {
            remaining = remaining.saturating_add(window.remaining);
            used = used.saturating_add(window.used);
            limit = limit.saturating_add(window.limit);
        }
        snapshot.quota_period = Some("daily".to_string());
        snapshot.quota_remaining_usd = Some(amount_to_string(remaining));
        snapshot.quota_used_usd = Some(amount_to_string(used));
        snapshot.quota_limit_usd = Some(amount_to_string(limit));
        if !matched_plan_names.is_empty() {
            matched_plan_names.sort();
            matched_plan_names.dedup();
            snapshot.plan_name = Some(matched_plan_names.join(", "));
        }
        snapshot.exhausted = Some(remaining.is_zero() && balance.is_none_or(UsdAmount::is_zero));
    } else {
        snapshot.exhausted = balance.map(UsdAmount::is_zero);
    }

    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn new_api_token_usage_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    if json_value_at_path(value, "success").and_then(bool_from_json) == Some(false)
        || json_value_at_path(value, "code").and_then(bool_from_json) == Some(false)
    {
        let message = json_value_at_path(value, "message")
            .and_then(|value| value.as_str())
            .unwrap_or("new api token usage response reported failure");
        return base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms)
            .with_error(message.to_string());
    }

    let mut effective = provider.extract.clone();
    if effective.remaining_balance_paths.is_empty() {
        effective.remaining_balance_paths = vec![
            "data.total_available".to_string(),
            "data.remain_quota".to_string(),
            "total_available".to_string(),
            "remain_quota".to_string(),
        ];
    }
    if effective.monthly_spent_paths.is_empty() {
        effective.monthly_spent_paths = vec![
            "data.total_used".to_string(),
            "data.used_quota".to_string(),
            "total_used".to_string(),
            "used_quota".to_string(),
        ];
    }
    if effective.monthly_budget_paths.is_empty() {
        effective.monthly_budget_paths = vec![
            "data.total_granted".to_string(),
            "total_granted".to_string(),
        ];
    }
    effective.remaining_divisor = effective.remaining_divisor.or(Some(500_000));
    effective.monthly_spent_divisor = effective.monthly_spent_divisor.or(Some(500_000));
    effective.monthly_budget_divisor = effective.monthly_budget_divisor.or(Some(500_000));

    let unlimited_quota =
        first_bool_from_paths(value, &[], &["data.unlimited_quota", "unlimited_quota"])
            == Some(true);
    let remaining_balance = first_amount_from_paths(
        value,
        &effective.remaining_balance_paths,
        &[],
        effective.remaining_divisor,
    );
    let monthly_spent = first_amount_from_paths(
        value,
        &effective.monthly_spent_paths,
        &[],
        effective.monthly_spent_divisor,
    );
    let monthly_budget = first_amount_from_paths(
        value,
        &effective.monthly_budget_paths,
        &[],
        effective.monthly_budget_divisor,
    )
    .or_else(|| match (remaining_balance, monthly_spent) {
        (Some(remaining), Some(spent)) => Some(remaining.saturating_add(spent)),
        _ => None,
    });
    let exhausted = if unlimited_quota {
        Some(false)
    } else {
        first_bool_from_paths(
            value,
            &effective.exhausted_paths,
            &["data.exhausted", "exhausted"],
        )
        .or_else(|| remaining_balance.map(UsdAmount::is_zero))
    };

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.plan_name = first_string_from_paths(value, &["data.name", "name"]);
    snapshot.unlimited_quota = Some(unlimited_quota);
    if !unlimited_quota {
        snapshot.quota_period = Some("token".to_string());
        snapshot.quota_remaining_usd = remaining_balance.map(amount_to_string);
        snapshot.quota_limit_usd = monthly_budget.map(amount_to_string);
        snapshot.quota_used_usd = monthly_spent.map(amount_to_string);
        snapshot.monthly_budget_usd = monthly_budget.map(amount_to_string);
        snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    } else {
        snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    }
    snapshot.exhausted = exhausted;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn new_api_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    if json_value_at_path(value, "success").and_then(bool_from_json) == Some(false) {
        let message = json_value_at_path(value, "message")
            .and_then(|value| value.as_str())
            .unwrap_or("new api balance response reported failure");
        return base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms)
            .with_error(message.to_string());
    }

    let mut effective = provider.extract.clone();
    if effective.remaining_balance_paths.is_empty() {
        effective.remaining_balance_paths = vec!["data.quota".to_string(), "quota".to_string()];
    }
    if effective.monthly_spent_paths.is_empty() {
        effective.monthly_spent_paths =
            vec!["data.used_quota".to_string(), "used_quota".to_string()];
    }
    effective.remaining_divisor = effective.remaining_divisor.or(Some(500_000));
    effective.monthly_spent_divisor = effective.monthly_spent_divisor.or(Some(500_000));
    effective.monthly_budget_divisor = effective.monthly_budget_divisor.or(Some(500_000));

    let remaining_balance = first_amount_from_paths(
        value,
        &effective.remaining_balance_paths,
        &[],
        effective.remaining_divisor,
    );
    let monthly_spent = first_amount_from_paths(
        value,
        &effective.monthly_spent_paths,
        &[],
        effective.monthly_spent_divisor,
    );
    let monthly_budget = first_amount_from_paths(
        value,
        &effective.monthly_budget_paths,
        &["data.total_quota", "total_quota"],
        effective.monthly_budget_divisor,
    )
    .or_else(|| match (remaining_balance, monthly_spent) {
        (Some(remaining), Some(spent)) => Some(remaining.saturating_add(spent)),
        _ => None,
    });
    let unlimited_quota =
        first_bool_from_paths(value, &[], &["data.unlimited_quota", "unlimited_quota"])
            == Some(true);
    let exhausted = if unlimited_quota {
        Some(false)
    } else {
        first_bool_from_paths(
            value,
            &effective.exhausted_paths,
            &["data.exhausted", "exhausted"],
        )
        .or_else(|| remaining_balance.map(UsdAmount::is_zero))
    };

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.unlimited_quota = Some(unlimited_quota);
    if !unlimited_quota {
        snapshot.quota_period = Some("quota".to_string());
        snapshot.quota_remaining_usd = remaining_balance.map(amount_to_string);
        snapshot.quota_limit_usd = monthly_budget.map(amount_to_string);
        snapshot.quota_used_usd = monthly_spent.map(amount_to_string);
        snapshot.monthly_budget_usd = monthly_budget.map(amount_to_string);
        snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    } else {
        snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    }
    snapshot.exhausted = exhausted;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn openai_cost_result_usd_amount(result: &serde_json::Value) -> Option<UsdAmount> {
    let amount = json_value_at_path(result, "amount.value").and_then(amount_from_json)?;
    let currency = json_value_at_path(result, "amount.currency").and_then(|value| value.as_str());
    match currency {
        Some(currency) if currency.eq_ignore_ascii_case("usd") => Some(amount),
        None => Some(amount),
        _ => None,
    }
}

fn openai_organization_costs_total(value: &serde_json::Value) -> Option<UsdAmount> {
    let buckets = json_value_at_path(value, "data")?.as_array()?;
    let mut total = UsdAmount::ZERO;

    for bucket in buckets {
        let Some(results) =
            json_value_at_path(bucket, "results").and_then(|value| value.as_array())
        else {
            continue;
        };
        for result in results {
            if let Some(amount) = openai_cost_result_usd_amount(result) {
                total = total.saturating_add(amount);
            }
        }
    }

    Some(total)
}

fn openai_organization_costs_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let spent = first_amount_from_paths(
        value,
        &provider.extract.monthly_spent_paths,
        &[],
        provider.extract.monthly_spent_divisor,
    )
    .or_else(|| openai_organization_costs_total(value));

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.monthly_spent_usd = spent.map(amount_to_string);
    snapshot.exhausted = None;
    snapshot.exhaustion_affects_routing = false;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn provider_hosts_for_diagnostics(
    cfg: &HelperConfig,
    service_name: &str,
    provider: &UsageProviderConfig,
) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for target in usage_provider_targets(cfg, service_name, None) {
        if domain_matches(&target.base_url, &provider.domains)
            && let Ok(url) = reqwest::Url::parse(&target.base_url)
            && let Some(host) = url.host_str()
        {
            hosts.push(host.to_string());
        }
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

fn warn_if_provider_spans_hosts(
    cfg: &HelperConfig,
    service_name: &str,
    provider: &UsageProviderConfig,
) {
    let hosts = provider_hosts_for_diagnostics(cfg, service_name, provider);
    if hosts.len() > 1 {
        warn!(
            "usage provider '{}' is associated with multiple hosts: {:?}; \
将按统一额度处理这些 upstream，如需区分配额请拆分为多个 provider 配置",
            provider.id, hosts
        );
    }
}

fn snapshot_from_provider_json(
    provider: &UsageProviderConfig,
    upstream: &UsageProviderEndpointRef,
    value: &serde_json::Value,
    upstream_base_url: &str,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    match provider.kind {
        ProviderKind::BudgetHttpJson => {
            budget_snapshot_from_json(provider, upstream, value, fetched_at_ms, stale_after_ms)
        }
        ProviderKind::YescodeProfile => {
            yescode_snapshot_from_json(provider, upstream, value, fetched_at_ms, stale_after_ms)
        }
        ProviderKind::OpenAiBalanceHttpJson => balance_http_snapshot_from_json(
            provider,
            upstream,
            value,
            fetched_at_ms,
            stale_after_ms,
        ),
        ProviderKind::Sub2ApiUsage => sub2api_usage_snapshot_from_json(
            provider,
            upstream,
            value,
            fetched_at_ms,
            stale_after_ms,
        ),
        ProviderKind::Sub2ApiAuthMe => sub2api_auth_me_snapshot_from_json(
            provider,
            upstream,
            value,
            fetched_at_ms,
            stale_after_ms,
        ),
        ProviderKind::NewApiTokenUsage => new_api_token_usage_snapshot_from_json(
            provider,
            upstream,
            value,
            fetched_at_ms,
            stale_after_ms,
        ),
        ProviderKind::NewApiUserSelf => {
            new_api_snapshot_from_json(provider, upstream, value, fetched_at_ms, stale_after_ms)
        }
        ProviderKind::RightCodeAccountSummary => rightcode_account_summary_snapshot_from_json(
            provider,
            upstream,
            value,
            upstream_base_url,
            fetched_at_ms,
            stale_after_ms,
        ),
        ProviderKind::OpenAiOrganizationCosts => openai_organization_costs_snapshot_from_json(
            provider,
            upstream,
            value,
            fetched_at_ms,
            stale_after_ms,
        ),
    }
}

async fn refresh_provider_target(
    params: RefreshProviderTargetParams<'_>,
) -> UsageProviderRefreshOutcome {
    let RefreshProviderTargetParams {
        client,
        provider,
        target,
        state,
        service_name,
        interval_secs,
        force,
    } = params;

    let fetched_at_ms = unix_now_ms();
    let stale_after_ms = stale_after_ms(fetched_at_ms, interval_secs);
    let snapshot_decision = existing_usage_provider_target_suppression_decision(
        state,
        service_name,
        &provider.id,
        target,
        fetched_at_ms,
    )
    .await;
    let now = Instant::now();
    if let Some(suppression) = active_usage_provider_target_suppression(&provider.id, target, now)
        && !force_can_bypass_active_suppression(force, &suppression, snapshot_decision.as_ref())
    {
        warn!(
            "usage provider '{}' skipped {}[{}]: balance refresh suppressed: {}",
            provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            suppression.reason
        );
        return UsageProviderRefreshOutcome::Failed;
    }
    if let Some(decision) = snapshot_decision {
        remember_usage_provider_target_suppression(
            &provider.id,
            target,
            decision.ttl,
            decision.reason.clone(),
            Instant::now(),
        );
        warn!(
            "usage provider '{}' skipped {}[{}]: existing balance snapshot suppresses refresh: {}",
            provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            decision.reason
        );
        return UsageProviderRefreshOutcome::Failed;
    }

    let Some(token) = resolve_token(provider, target) else {
        let snapshot = if provider.kind == ProviderKind::OpenAiOrganizationCosts {
            base_snapshot(provider, &target.endpoint, fetched_at_ms, stale_after_ms)
        } else {
            base_snapshot(provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                .with_error("no usable token; checked provider token_env and upstream auth")
        };
        state.record_provider_balance_snapshot(snapshot).await;
        if provider.kind == ProviderKind::OpenAiOrganizationCosts {
            warn!(
                "usage provider '{}' is missing OPENAI_ADMIN_KEY; OpenAI official costs stay unknown",
                provider.id
            );
        } else {
            warn!(
                "usage provider '{}' has no usable token (checked token_env and associated upstream auth_token); \
跳过本次用量查询，请检查 usage_providers.json 和 ~/.codex-helper/config.toml",
                provider.id
            );
        }
        return UsageProviderRefreshOutcome::MissingToken;
    };

    let prepared_poll = match prepare_provider_poll(
        state.as_ref(),
        provider,
        target,
        &token,
        provider.kind.source_name(),
        None,
        fetched_at_ms,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let snapshot = base_snapshot(provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                .with_error(error.to_string());
            state.record_provider_balance_snapshot(snapshot).await;
            warn!(
                "usage provider '{}' could not prepare poll for {}[{}]: {}",
                provider.id,
                target.endpoint.provider_endpoint.provider_id,
                target.endpoint.catalog_index,
                error
            );
            return UsageProviderRefreshOutcome::Failed;
        }
    };

    match poll_provider_http_json(
        client,
        provider,
        &prepared_poll.endpoint,
        prepared_poll.new_api_user_id.as_deref(),
        &token,
    )
    .await
    {
        Ok(value) => {
            let snapshot = snapshot_from_provider_json(
                provider,
                &target.endpoint,
                &value,
                &target.base_url,
                fetched_at_ms,
                stale_after_ms,
            );
            let snapshot_error = usage_provider_snapshot_error(&snapshot).map(str::to_string);
            let suppression_decision =
                usage_provider_suppression_decision_from_snapshot(&snapshot, fetched_at_ms);
            let exhausted_for_routing = suppression_decision.is_some();
            let terminal_snapshot_error = snapshot_error
                .as_deref()
                .is_some_and(usage_provider_error_is_terminal);
            let observation_committed = commit_provider_poll_observation(
                state.as_ref(),
                prepared_poll.reservation,
                &snapshot,
                suppression_decision.as_ref(),
                unix_now_ms(),
            )
            .await;
            if observation_committed && let Some(decision) = suppression_decision.as_ref() {
                remember_usage_provider_target_suppression(
                    &provider.id,
                    target,
                    decision.ttl,
                    decision.reason.as_str(),
                    Instant::now(),
                );
            } else if observation_committed && terminal_snapshot_error {
                remember_usage_provider_target_suppression(
                    &provider.id,
                    target,
                    USAGE_PROVIDER_TERMINAL_FAILURE_TTL,
                    snapshot_error
                        .as_deref()
                        .unwrap_or("terminal provider error"),
                    Instant::now(),
                );
            } else if observation_committed {
                clear_usage_provider_target_suppression(&provider.id, target);
            }
            state.record_provider_balance_snapshot(snapshot).await;
            if let Some(error) = snapshot_error {
                warn!(
                    "usage provider '{}' returned error snapshot for {}[{}]: {}",
                    provider.id,
                    target.endpoint.provider_endpoint.provider_id,
                    target.endpoint.catalog_index,
                    error
                );
                return UsageProviderRefreshOutcome::Failed;
            }
            info!(
                "usage provider '{}' refreshed {}[{}], exhausted = {}, routing_trusted = {}",
                provider.id,
                target.endpoint.provider_endpoint.provider_id,
                target.endpoint.catalog_index,
                exhausted_for_routing,
                provider.trust_exhaustion_for_routing
            );
            UsageProviderRefreshOutcome::Refreshed
        }
        Err(err) => {
            let error = err.to_string();
            let terminal_failure = usage_provider_error_is_terminal(&error);
            if terminal_failure {
                remember_usage_provider_target_suppression(
                    &provider.id,
                    target,
                    USAGE_PROVIDER_TERMINAL_FAILURE_TTL,
                    error.clone(),
                    Instant::now(),
                );
            }
            let snapshot = base_snapshot(provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                .with_error(error.clone());
            commit_provider_poll_observation(
                state.as_ref(),
                prepared_poll.reservation,
                &snapshot,
                None,
                unix_now_ms(),
            )
            .await;
            state.record_provider_balance_snapshot(snapshot).await;
            warn!(
                "usage provider '{}' poll failed for {}[{}]: {}",
                provider.id,
                target.endpoint.provider_endpoint.provider_id,
                target.endpoint.catalog_index,
                error
            );
            UsageProviderRefreshOutcome::Failed
        }
    }
}

struct ConfiguredRefreshJob<'a> {
    provider: &'a UsageProviderConfig,
    target: UsageProviderTarget,
    interval_secs: u64,
    force: bool,
}

struct AutoRefreshJob {
    target: UsageProviderTarget,
    force: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UsageProviderRefreshOptions<'a> {
    pub route_provider_id_filter: Option<&'a str>,
    pub provider_id_filter: Option<&'a str>,
    pub force: bool,
}

async fn run_configured_refresh_job<'a>(
    client: &'a Client,
    job: ConfiguredRefreshJob<'a>,
    state: &'a Arc<ProxyState>,
    service_name: &'a str,
) -> (String, UsageProviderRefreshOutcome) {
    let provider_id = job.provider.id.clone();
    let outcome = refresh_provider_target(RefreshProviderTargetParams {
        client,
        provider: job.provider,
        target: &job.target,
        state,
        service_name,
        interval_secs: job.interval_secs,
        force: job.force,
    })
    .await;
    (provider_id, outcome)
}

async fn run_auto_refresh_job(
    client: &Client,
    job: AutoRefreshJob,
    state: &Arc<ProxyState>,
    service_name: &str,
) -> UsageProviderRefreshOutcome {
    auto_probe_provider_target(client, &job.target, state, service_name, job.force).await
}

async fn run_configured_refresh_jobs<'a>(
    client: &'a Client,
    jobs: Vec<ConfiguredRefreshJob<'a>>,
    state: &'a Arc<ProxyState>,
    service_name: &'a str,
) -> Vec<(String, UsageProviderRefreshOutcome)> {
    let mut pending = jobs.into_iter();
    let mut running = FuturesUnordered::new();
    let mut results = Vec::new();
    let concurrency = BALANCE_REFRESH_CONCURRENCY.max(1);

    for _ in 0..concurrency {
        let Some(job) = pending.next() else {
            break;
        };
        running.push(run_configured_refresh_job(client, job, state, service_name));
    }

    while let Some(result) = running.next().await {
        results.push(result);
        if let Some(job) = pending.next() {
            running.push(run_configured_refresh_job(client, job, state, service_name));
        }
    }

    results
}

async fn run_auto_refresh_jobs(
    client: &Client,
    jobs: Vec<AutoRefreshJob>,
    state: &Arc<ProxyState>,
    service_name: &str,
) -> Vec<UsageProviderRefreshOutcome> {
    let mut pending = jobs.into_iter();
    let mut running = FuturesUnordered::new();
    let mut results = Vec::new();
    let concurrency = BALANCE_REFRESH_CONCURRENCY.max(1);

    for _ in 0..concurrency {
        let Some(job) = pending.next() else {
            break;
        };
        running.push(run_auto_refresh_job(client, job, state, service_name));
    }

    while let Some(result) = running.next().await {
        results.push(result);
        if let Some(job) = pending.next() {
            running.push(run_auto_refresh_job(client, job, state, service_name));
        }
    }

    results
}

fn auto_snapshot_is_usable(snapshot: &ProviderBalanceSnapshot) -> bool {
    snapshot.error.is_none()
        && matches!(
            snapshot.status,
            BalanceSnapshotStatus::Ok | BalanceSnapshotStatus::Exhausted
        )
}

fn normalized_error_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '_' | '-' | '.' | '/' | ':' | '[' | ']' | '(' | ')' | ',' | ';' => ' ',
            _ => ch.to_ascii_lowercase(),
        })
        .collect::<String>()
}

fn usage_provider_error_is_terminal(error: &str) -> bool {
    let normalized = normalized_error_text(error);
    let terminal_markers = [
        "user inactive",
        "user account is not active",
        "account is not active",
        "account inactive",
        "account disabled",
        "user disabled",
        "api key disabled",
        "api key is disabled",
        "api key inactive",
        "api key is not active",
        "key inactive",
        "key disabled",
        "invalid api key",
        "invalid token",
        "invalid bearer token",
        "token invalid",
        "unauthorized api key",
        "insufficient balance",
        "balance insufficient",
        "insufficient quota",
        "quota exhausted",
        "quota exceeded",
        "no balance",
        "余额不足",
        "额度不足",
        "配额不足",
        "账户未激活",
        "账号未激活",
        "用户未激活",
        "账户已禁用",
        "账号已禁用",
        "用户已禁用",
        "密钥无效",
        "令牌无效",
    ];
    terminal_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn quota_period_is_current_day(period: &str) -> bool {
    let normalized = period.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "daily" | "day" | "today" | "current_day" | "current-day" | "1d" | "24h" | "今日" | "今天"
    )
}

fn quota_period_is_refreshable_window(period: &str) -> bool {
    let normalized = period.trim().to_ascii_lowercase();
    quota_period_is_current_day(&normalized)
        || matches!(
            normalized.as_str(),
            "weekly" | "week" | "7d" | "monthly" | "month"
        )
        || normalized.starts_with("rate_limit:")
}

fn snapshot_has_current_day_quota_exhaustion(snapshot: &ProviderBalanceSnapshot) -> bool {
    snapshot.exhausted == Some(true)
        && snapshot
            .quota_period
            .as_deref()
            .is_some_and(quota_period_is_current_day)
}

fn snapshot_has_refreshable_window_exhaustion(snapshot: &ProviderBalanceSnapshot) -> bool {
    snapshot.exhausted == Some(true)
        && snapshot
            .quota_period
            .as_deref()
            .is_some_and(quota_period_is_refreshable_window)
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn duration_until_ms(deadline_ms: u64, now_ms: u64) -> Option<Duration> {
    deadline_ms
        .checked_sub(now_ms)
        .filter(|remaining_ms| *remaining_ms > 0)
        .map(Duration::from_millis)
}

fn next_daily_quota_reset_at_ms(now_ms: u64) -> Option<u64> {
    next_fixed_offset_reset_at_ms(now_ms, DAILY_QUOTA_RESET_UTC_OFFSET, DAILY_QUOTA_RESET_TIME)
}

fn next_fixed_offset_reset_at_ms(now_ms: u64, utc_offset: &str, reset_time: &str) -> Option<u64> {
    let offset_ms = parse_utc_offset_ms(utc_offset)?;
    let reset_ms = parse_hh_mm_ms(reset_time)?;
    let local_ms = i128::from(now_ms) + offset_ms;
    let local_day_start = div_floor(local_ms, i128::from(DAY_MS)) * i128::from(DAY_MS);
    let mut reset_local = local_day_start + i128::from(reset_ms);
    if reset_local <= local_ms {
        reset_local += i128::from(DAY_MS);
    }
    let reset_utc = reset_local - offset_ms;
    u64::try_from(reset_utc).ok()
}

fn parse_hh_mm_ms(value: &str) -> Option<u64> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour = hour.parse::<u64>().ok()?;
    let minute = minute.parse::<u64>().ok()?;
    if hour >= 24 || minute >= 60 {
        return None;
    }
    Some(
        hour.saturating_mul(HOUR_MS)
            .saturating_add(minute.saturating_mul(MINUTE_MS)),
    )
}

fn parse_utc_offset_ms(value: &str) -> Option<i128> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("z") || value == "+00:00" || value == "-00:00" {
        return Some(0);
    }
    let sign = match value.as_bytes().first().copied()? {
        b'+' => 1_i128,
        b'-' => -1_i128,
        _ => return None,
    };
    let (hour, minute) = value[1..].split_once(':')?;
    let hour = hour.parse::<i128>().ok()?;
    let minute = minute.parse::<i128>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (hour * i128::from(HOUR_MS) + minute * i128::from(MINUTE_MS)))
}

fn div_floor(dividend: i128, divisor: i128) -> i128 {
    let quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder != 0 && ((remainder > 0) != (divisor > 0)) {
        quotient - 1
    } else {
        quotient
    }
}

fn snapshot_freshness_ttl(snapshot: &ProviderBalanceSnapshot, now_ms: u64) -> Option<Duration> {
    snapshot
        .stale_after_ms
        .and_then(|stale_after_ms| duration_until_ms(stale_after_ms, now_ms))
}

fn current_day_quota_suppression_ttl(
    snapshot: &ProviderBalanceSnapshot,
    now_ms: u64,
) -> Option<Duration> {
    let reset_at_ms = next_daily_quota_reset_at_ms(snapshot.fetched_at_ms)?;
    duration_until_ms(reset_at_ms, now_ms)
}

fn usage_provider_snapshot_suppression_ttl(
    snapshot: &ProviderBalanceSnapshot,
    now_ms: u64,
) -> Option<Duration> {
    if let Some(reset_at_ms) = snapshot.quota_resets_at_ms {
        let suppress_until_ms = reset_at_ms.saturating_add(duration_millis_u64(
            USAGE_PROVIDER_DAILY_RESET_SUPPRESSION_GRACE,
        ));
        return duration_until_ms(suppress_until_ms, now_ms);
    }

    if snapshot_has_current_day_quota_exhaustion(snapshot) {
        return current_day_quota_suppression_ttl(snapshot, now_ms);
    }

    if snapshot_has_refreshable_window_exhaustion(snapshot) {
        return snapshot_freshness_ttl(snapshot, now_ms);
    }

    if snapshot.stale_at(now_ms) {
        None
    } else {
        Some(USAGE_PROVIDER_EXHAUSTED_SUPPRESSION_TTL)
    }
}

fn usage_provider_snapshot_suppression_reason(
    snapshot: &ProviderBalanceSnapshot,
) -> Option<String> {
    if snapshot.status_at(snapshot.fetched_at_ms) != BalanceSnapshotStatus::Exhausted {
        return None;
    }

    if snapshot_has_current_day_quota_exhaustion(snapshot) {
        let period = snapshot.quota_period.as_deref().unwrap_or("daily");
        return Some(format!(
            "{period} package quota exhausted for current period"
        ));
    }

    if snapshot_has_refreshable_window_exhaustion(snapshot) {
        let period = snapshot.quota_period.as_deref().unwrap_or("usage");
        return Some(format!(
            "{period} usage window exhausted for current period"
        ));
    }

    if snapshot.routing_exhausted() {
        return Some("balance exhausted".to_string());
    }

    None
}

fn usage_provider_snapshot_error(snapshot: &ProviderBalanceSnapshot) -> Option<&str> {
    snapshot
        .error
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn usage_provider_suppression_decision_from_snapshot(
    snapshot: &ProviderBalanceSnapshot,
    now_ms: u64,
) -> Option<ProviderTargetSuppressionDecision> {
    let reason = usage_provider_snapshot_suppression_reason(snapshot)?;
    usage_provider_snapshot_suppression_ttl(snapshot, now_ms)
        .map(|ttl| ProviderTargetSuppressionDecision { reason, ttl })
}

fn provider_balance_snapshot_matches_target(
    snapshot: &ProviderBalanceSnapshot,
    provider_id: &str,
    target: &UsageProviderTarget,
) -> bool {
    if snapshot.observation_provider_id != provider_id {
        return false;
    }
    snapshot.provider_endpoint == target.endpoint.provider_endpoint
}

async fn existing_usage_provider_target_suppression_decision(
    state: &Arc<ProxyState>,
    service_name: &str,
    provider_id: &str,
    target: &UsageProviderTarget,
    now_ms: u64,
) -> Option<ProviderTargetSuppressionDecision> {
    let view = state.get_provider_balance_view(service_name).await;
    view.iter()
        .filter(|snapshot| provider_balance_snapshot_matches_target(snapshot, provider_id, target))
        .max_by_key(|snapshot| snapshot.fetched_at_ms)
        .and_then(|snapshot| usage_provider_suppression_decision_from_snapshot(snapshot, now_ms))
}

fn auto_probe_error_summary(probe_errors: &[String]) -> Option<String> {
    (!probe_errors.is_empty()).then(|| format!("attempts failed: {}", probe_errors.join("; ")))
}

async fn auto_probe_provider_target(
    client: &Client,
    target: &UsageProviderTarget,
    state: &Arc<ProxyState>,
    service_name: &str,
    force: bool,
) -> UsageProviderRefreshOutcome {
    let fetched_at_ms = unix_now_ms();
    let interval_secs = DEFAULT_POLL_INTERVAL_SECS;
    let stale_after_ms = stale_after_ms(fetched_at_ms, interval_secs);

    if is_official_openai_base_url(&target.base_url) {
        let provider = auto_openai_official_provider(target);
        let Some(token) = resolve_token(&provider, target) else {
            state
                .record_provider_balance_snapshot(base_snapshot(
                    &provider,
                    &target.endpoint,
                    fetched_at_ms,
                    stale_after_ms,
                ))
                .await;
            warn!(
                "OpenAI organization costs require OPENAI_ADMIN_KEY; balance stays unknown for {}[{}]",
                target.endpoint.provider_endpoint.provider_id, target.endpoint.catalog_index
            );
            return UsageProviderRefreshOutcome::MissingToken;
        };

        let prepared_poll = match prepare_provider_poll(
            state.as_ref(),
            &provider,
            target,
            &token,
            provider.kind.source_name(),
            None,
            fetched_at_ms,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let snapshot =
                    base_snapshot(&provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                        .with_error(error.to_string());
                state.record_provider_balance_snapshot(snapshot).await;
                return UsageProviderRefreshOutcome::Failed;
            }
        };

        return match poll_provider_http_json(
            client,
            &provider,
            &prepared_poll.endpoint,
            prepared_poll.new_api_user_id.as_deref(),
            &token,
        )
        .await
        {
            Ok(value) => {
                let snapshot = snapshot_from_provider_json(
                    &provider,
                    &target.endpoint,
                    &value,
                    &target.base_url,
                    fetched_at_ms,
                    stale_after_ms,
                );
                commit_provider_poll_observation(
                    state.as_ref(),
                    prepared_poll.reservation,
                    &snapshot,
                    None,
                    unix_now_ms(),
                )
                .await;
                state.record_provider_balance_snapshot(snapshot).await;
                UsageProviderRefreshOutcome::Refreshed
            }
            Err(err) => {
                let snapshot =
                    base_snapshot(&provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                        .with_error(err.to_string());
                commit_provider_poll_observation(
                    state.as_ref(),
                    prepared_poll.reservation,
                    &snapshot,
                    None,
                    unix_now_ms(),
                )
                .await;
                state.record_provider_balance_snapshot(snapshot).await;
                warn!(
                    "OpenAI organization costs poll failed for {}[{}]: {}",
                    target.endpoint.provider_endpoint.provider_id,
                    target.endpoint.catalog_index,
                    err
                );
                UsageProviderRefreshOutcome::Failed
            }
        };
    }

    let first_provider = auto_usage_provider(target, first_auto_probe_kind(target));
    let provider_id = first_provider.id.clone();
    let snapshot_decision = existing_usage_provider_target_suppression_decision(
        state,
        service_name,
        &provider_id,
        target,
        fetched_at_ms,
    )
    .await;
    let now = Instant::now();
    if let Some(suppression) = active_usage_provider_target_suppression(&provider_id, target, now)
        && !force_can_bypass_active_suppression(force, &suppression, snapshot_decision.as_ref())
    {
        warn!(
            "auto usage provider '{}' skipped {}[{}]: balance refresh suppressed: {}",
            first_provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            suppression.reason
        );
        return UsageProviderRefreshOutcome::Failed;
    }
    if let Some(decision) = snapshot_decision {
        remember_usage_provider_target_suppression(
            &provider_id,
            target,
            decision.ttl,
            decision.reason.clone(),
            Instant::now(),
        );
        warn!(
            "auto usage provider '{}' skipped {}[{}]: existing balance snapshot suppresses refresh: {}",
            first_provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            decision.reason
        );
        return UsageProviderRefreshOutcome::Failed;
    }

    let Some(token) = resolve_token(&first_provider, target) else {
        state
            .record_provider_balance_snapshot(
                base_snapshot(
                    &first_provider,
                    &target.endpoint,
                    fetched_at_ms,
                    stale_after_ms,
                )
                .with_error("no usable token; checked upstream auth"),
            )
            .await;
        return UsageProviderRefreshOutcome::MissingToken;
    };

    let probe_order = auto_probe_kind_order(&provider_id, target, force);
    if probe_order.is_empty() {
        let error = "all balance probe kinds are temporarily suppressed";
        warn!(
            "auto usage provider '{}' skipped {}[{}]: {}",
            first_provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            error
        );
        state
            .record_provider_balance_snapshot(
                base_snapshot(
                    &first_provider,
                    &target.endpoint,
                    fetched_at_ms,
                    stale_after_ms,
                )
                .with_error(error),
            )
            .await;
        return UsageProviderRefreshOutcome::Failed;
    }

    let auto_config_revision = auto_usage_provider_config_revision(target);
    let mut probe_errors = Vec::new();
    for kind in probe_order {
        let provider = auto_usage_provider(target, kind);
        let prepared_poll = match prepare_provider_poll(
            state.as_ref(),
            &provider,
            target,
            &token,
            "usage_provider:auto_balance",
            Some(auto_config_revision.as_str()),
            fetched_at_ms,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                remember_auto_probe_kind_failure(&provider_id, target, kind, Instant::now());
                probe_errors.push(format!("{kind:?}: {error}"));
                continue;
            }
        };
        match poll_provider_http_json(
            client,
            &provider,
            &prepared_poll.endpoint,
            prepared_poll.new_api_user_id.as_deref(),
            &token,
        )
        .await
        {
            Ok(value) => {
                let snapshot = snapshot_from_provider_json(
                    &provider,
                    &target.endpoint,
                    &value,
                    &target.base_url,
                    fetched_at_ms,
                    stale_after_ms,
                );
                if auto_snapshot_is_usable(&snapshot) {
                    remember_auto_probe_kind_success(&provider_id, target, kind);
                    let suppression_decision =
                        usage_provider_suppression_decision_from_snapshot(&snapshot, fetched_at_ms);
                    let exhausted_for_routing = suppression_decision.is_some();
                    let observation_committed = commit_provider_poll_observation(
                        state.as_ref(),
                        prepared_poll.reservation,
                        &snapshot,
                        suppression_decision.as_ref(),
                        unix_now_ms(),
                    )
                    .await;
                    if observation_committed && let Some(decision) = suppression_decision.as_ref() {
                        remember_usage_provider_target_suppression(
                            &provider_id,
                            target,
                            decision.ttl,
                            decision.reason.as_str(),
                            Instant::now(),
                        );
                    } else if observation_committed {
                        clear_usage_provider_target_suppression(&provider_id, target);
                    }
                    state.record_provider_balance_snapshot(snapshot).await;
                    info!(
                        "auto usage provider '{}' refreshed {}[{}] via {:?}, exhausted = {}",
                        provider.id,
                        target.endpoint.provider_endpoint.provider_id,
                        target.endpoint.catalog_index,
                        kind,
                        exhausted_for_routing
                    );
                    return UsageProviderRefreshOutcome::Refreshed;
                }
                commit_provider_poll_observation(
                    state.as_ref(),
                    prepared_poll.reservation,
                    &snapshot,
                    None,
                    unix_now_ms(),
                )
                .await;
                remember_auto_probe_kind_failure(&provider_id, target, kind, Instant::now());
                let error = snapshot.error.unwrap_or_else(|| {
                    format!("auto probe {:?} returned no usable balance fields", kind)
                });
                probe_errors.push(format!("{:?}: {}", kind, error));
            }
            Err(err) => {
                let snapshot =
                    base_snapshot(&provider, &target.endpoint, fetched_at_ms, stale_after_ms)
                        .with_error(err.to_string());
                commit_provider_poll_observation(
                    state.as_ref(),
                    prepared_poll.reservation,
                    &snapshot,
                    None,
                    unix_now_ms(),
                )
                .await;
                remember_auto_probe_kind_failure(&provider_id, target, kind, Instant::now());
                probe_errors.push(format!("{:?}: {}", kind, err));
            }
        }
    }

    if let Some(error) = auto_probe_error_summary(&probe_errors) {
        warn!(
            "auto usage provider '{}' found no usable balance endpoint for {}[{}]: {}",
            first_provider.id,
            target.endpoint.provider_endpoint.provider_id,
            target.endpoint.catalog_index,
            error
        );
        state
            .record_provider_balance_snapshot(
                base_snapshot(
                    &first_provider,
                    &target.endpoint,
                    fetched_at_ms,
                    stale_after_ms,
                )
                .with_error(error.clone()),
            )
            .await;
        let terminal_failure = usage_provider_error_is_terminal(error.as_str());
        if terminal_failure {
            remember_usage_provider_target_suppression(
                &provider_id,
                target,
                USAGE_PROVIDER_TERMINAL_FAILURE_TTL,
                error.clone(),
                Instant::now(),
            );
        }
    }
    UsageProviderRefreshOutcome::Failed
}

pub async fn refresh_balances_for_service(
    client: &Client,
    cfg: Arc<HelperConfig>,
    state: Arc<ProxyState>,
    service_name: &str,
    options: UsageProviderRefreshOptions<'_>,
) -> Result<UsageProviderRefreshSummary> {
    // Tests should be hermetic and must not depend on real user `usage_providers.json`.
    if cfg!(test) {
        return Ok(UsageProviderRefreshSummary::default());
    }

    let UsageProviderRefreshOptions {
        route_provider_id_filter,
        provider_id_filter,
        force,
    } = options;
    let route_provider_id_filter = route_provider_id_filter
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let provider_id_filter = provider_id_filter
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let providers_file = load_providers()?;
    let mut summary = UsageProviderRefreshSummary {
        providers_configured: providers_file.providers.len(),
        ..UsageProviderRefreshSummary::default()
    };

    let poll_map = LAST_USAGE_POLL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut configured_jobs = Vec::new();
    let mut configured_job_keys = HashSet::new();
    for provider in &providers_file.providers {
        if provider_id_filter.is_some_and(|filter| filter != provider.id.as_str()) {
            continue;
        }

        let targets =
            matching_provider_targets(&cfg, service_name, provider, route_provider_id_filter);
        if targets.is_empty() {
            continue;
        }

        summary.providers_matched += 1;
        summary.upstreams_matched += targets.len();
        warn_if_provider_spans_hosts(&cfg, service_name, provider);

        let interval_secs = snapshot_refresh_interval_secs(provider);
        for target in targets {
            summary.attempted += 1;
            configured_job_keys.insert(target_key(&target));
            configured_jobs.push(ConfiguredRefreshJob {
                provider,
                target,
                interval_secs,
                force,
            });
        }
    }

    let mut refreshed_provider_ids = HashSet::new();
    if !configured_jobs.is_empty() {
        for (provider_id, outcome) in
            run_configured_refresh_jobs(client, configured_jobs, &state, service_name).await
        {
            match outcome {
                UsageProviderRefreshOutcome::Refreshed => {
                    summary.refreshed += 1;
                    refreshed_provider_ids.insert(provider_id);
                }
                UsageProviderRefreshOutcome::Failed => summary.failed += 1,
                UsageProviderRefreshOutcome::MissingToken => summary.missing_token += 1,
            }
        }
    }

    let mut auto_jobs = Vec::new();
    for target in usage_provider_targets(&cfg, service_name, route_provider_id_filter) {
        if configured_job_keys.contains(&target_key(&target)) {
            continue;
        }
        if !auto_target_matches_provider_id_filter(&target, provider_id_filter) {
            continue;
        }

        summary.attempted += 1;
        summary.auto_attempted += 1;
        auto_jobs.push(AutoRefreshJob { target, force });
    }

    if !auto_jobs.is_empty() {
        for outcome in run_auto_refresh_jobs(client, auto_jobs, &state, service_name).await {
            match outcome {
                UsageProviderRefreshOutcome::Refreshed => {
                    summary.refreshed += 1;
                    summary.auto_refreshed += 1;
                }
                UsageProviderRefreshOutcome::Failed => {
                    summary.failed += 1;
                    summary.auto_failed += 1;
                }
                UsageProviderRefreshOutcome::MissingToken => {
                    summary.missing_token += 1;
                }
            }
        }
    }

    if !refreshed_provider_ids.is_empty()
        && let Ok(mut map) = poll_map.lock()
    {
        let now = Instant::now();
        for provider_id in refreshed_provider_ids {
            map.insert(provider_id, now);
        }
    }

    Ok(summary)
}

/// Queues an identity-scoped provider observation refresh for the route graph executor.
pub fn enqueue_poll_for_codex_provider_endpoint(
    client: Client,
    cfg: Arc<HelperConfig>,
    state: Arc<ProxyState>,
    service_name: &str,
    provider_endpoint: ProviderEndpointKey,
) {
    let key = provider_endpoint.clone();
    let Some(initial_sleep_for) = enqueue_request_balance_refresh(key.clone()) else {
        return;
    };

    let service_name = service_name.to_string();
    tokio::spawn(async move {
        let mut sleep_for = initial_sleep_for;
        loop {
            tokio::time::sleep(sleep_for).await;
            match take_request_balance_refresh_if_due(&key) {
                RequestBalanceQueueDue::Due => {}
                RequestBalanceQueueDue::NotDue(delay) => {
                    sleep_for = delay;
                    continue;
                }
                RequestBalanceQueueDue::Missing => return,
            }

            let current_target = usage_provider_target_for_provider_endpoint(
                &cfg,
                service_name.as_str(),
                &provider_endpoint,
            );
            match poll_for_codex_target(
                client.clone(),
                cfg.clone(),
                state.clone(),
                service_name.as_str(),
                current_target,
            )
            .await
            {
                RequestBalancePollOutcome::Attempted | RequestBalancePollOutcome::Skipped => {
                    return;
                }
                RequestBalancePollOutcome::Deferred(delay) => {
                    schedule_request_balance_refresh_at(key.clone(), Instant::now() + delay);
                    sleep_for = delay;
                }
            }
        }
    });
}

async fn poll_for_codex_target(
    client: Client,
    cfg: Arc<HelperConfig>,
    state: Arc<ProxyState>,
    service_name: &str,
    current_target: Option<UsageProviderTarget>,
) -> RequestBalancePollOutcome {
    // Tests should be hermetic and should not depend on any real user `usage_providers.json` on
    // the machine running the suite. Disable provider polling during tests to avoid flakiness.
    if cfg!(test) {
        return RequestBalancePollOutcome::Skipped;
    }

    let providers_file = match load_providers() {
        Ok(providers_file) => providers_file,
        Err(error) => {
            warn!(
                "skipping request-driven usage provider poll because configuration could not be loaded: {error:#}"
            );
            return RequestBalancePollOutcome::Skipped;
        }
    };
    let Some(current_target) = current_target else {
        return RequestBalancePollOutcome::Skipped;
    };

    let now = Instant::now();
    let poll_map = LAST_USAGE_POLL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut matched_configured_provider = false;
    let mut configured_jobs = Vec::new();
    let mut next_cooldown = None::<Duration>;

    for provider in &providers_file.providers {
        if !domain_matches(&current_target.base_url, &provider.domains) {
            continue;
        }
        matched_configured_provider = true;

        let Some(interval_secs) = effective_poll_interval_secs(provider) else {
            continue;
        };

        {
            let mut map = match poll_map.lock() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Some(last) = map.get(&provider.id)
                && let Some(cooldown) = remaining_poll_cooldown(*last, interval_secs, now)
            {
                next_cooldown =
                    Some(next_cooldown.map_or(cooldown, |existing| existing.min(cooldown)));
                continue;
            }
            map.insert(provider.id.clone(), now);
        }

        warn_if_provider_spans_hosts(&cfg, service_name, provider);
        configured_jobs.push(ConfiguredRefreshJob {
            provider,
            target: current_target.clone(),
            interval_secs,
            force: false,
        });
    }

    if !configured_jobs.is_empty() {
        let _ = run_configured_refresh_jobs(&client, configured_jobs, &state, service_name).await;
        return RequestBalancePollOutcome::Attempted;
    }

    if matched_configured_provider {
        return next_cooldown
            .map(RequestBalancePollOutcome::Deferred)
            .unwrap_or(RequestBalancePollOutcome::Skipped);
    }

    let auto_provider = if is_official_openai_base_url(&current_target.base_url) {
        auto_openai_official_provider(&current_target)
    } else {
        auto_usage_provider(&current_target, first_auto_probe_kind(&current_target))
    };
    let Some(interval_secs) = effective_poll_interval_secs(&auto_provider) else {
        return RequestBalancePollOutcome::Skipped;
    };

    {
        let mut map = match poll_map.lock() {
            Ok(m) => m,
            Err(_) => return RequestBalancePollOutcome::Skipped,
        };
        if let Some(last) = map.get(&auto_provider.id)
            && let Some(cooldown) = remaining_poll_cooldown(*last, interval_secs, now)
        {
            return RequestBalancePollOutcome::Deferred(cooldown);
        }
        map.insert(auto_provider.id.clone(), now);
    }

    let _ = auto_probe_provider_target(&client, &current_target, &state, service_name, false).await;
    RequestBalancePollOutcome::Attempted
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::balance::BalanceSnapshotStatus;
    use crate::config::{ProviderConfig, ProviderEndpointConfig, UpstreamAuth};
    use axum::routing::get;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    async fn spawn_axum_server(app: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (addr, handle)
    }

    fn provider(id: &str, kind: ProviderKind) -> UsageProviderConfig {
        UsageProviderConfig {
            id: id.to_string(),
            kind,
            domains: vec!["example.com".to_string()],
            endpoint: "https://example.com/usage".to_string(),
            token_env: None,
            require_token_env: false,
            new_api_user_id_env: None,
            poll_interval_secs: Some(60),
            refresh_on_request: true,
            trust_exhaustion_for_routing: true,
            extract: UsageProviderExtractConfig::default(),
        }
    }

    fn upstream() -> UsageProviderEndpointRef {
        UsageProviderEndpointRef {
            provider_endpoint: ProviderEndpointKey::new("codex", "right", "default"),
            catalog_index: 1,
        }
    }

    fn provider_config(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            ..ProviderConfig::default()
        }
    }

    fn provider_endpoint_config(base_url: &str, endpoint_id: &str) -> ProviderConfig {
        ProviderConfig {
            endpoints: BTreeMap::from([(endpoint_id.to_string(), endpoint_config(base_url))]),
            ..ProviderConfig::default()
        }
    }

    fn endpoint_config(base_url: &str) -> ProviderEndpointConfig {
        ProviderEndpointConfig {
            base_url: base_url.to_string(),
            continuity_domain: None,
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            limits: crate::config::ProviderConcurrencyLimits::default(),
        }
    }

    fn helper_config(providers: Vec<(&str, ProviderConfig)>) -> HelperConfig {
        let mut cfg = HelperConfig::default();
        cfg.codex.providers = providers
            .into_iter()
            .map(|(provider_id, provider)| (provider_id.to_string(), provider))
            .collect();
        cfg
    }

    fn usage_provider_target(base_url: &str, provider_id: &str) -> UsageProviderTarget {
        UsageProviderTarget {
            endpoint: UsageProviderEndpointRef {
                provider_endpoint: ProviderEndpointKey::new("codex", provider_id, "default"),
                catalog_index: 0,
            },
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        }
    }

    fn usage_provider_target_at(
        endpoint_id: &str,
        catalog_index: usize,
        base_url: &str,
        provider_id: &str,
    ) -> UsageProviderTarget {
        UsageProviderTarget {
            endpoint: UsageProviderEndpointRef {
                provider_endpoint: ProviderEndpointKey::new("codex", provider_id, endpoint_id),
                catalog_index,
            },
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        }
    }

    fn isolated_usage_provider_path(test_name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "codex-helper-usage-provider-{test_name}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create isolated test directory");
        let path = directory.join("usage_providers.json");
        (directory, path)
    }

    fn clear_auto_probe_kind_state(provider_id: &str) {
        if let Some(hints) = AUTO_PROBE_KIND_HINTS.get()
            && let Ok(mut hints) = hints.lock()
        {
            hints.remove(provider_id);
        }
        if let Some(failures) = AUTO_PROBE_KIND_FAILURES.get()
            && let Ok(mut failures) = failures.lock()
        {
            failures.retain(|key, _| key.provider_id != provider_id);
        }
        clear_usage_provider_target_suppressions_for_provider(provider_id);
    }

    async fn target_automatic_eligibility(
        state: &ProxyState,
        target: &UsageProviderTarget,
    ) -> Option<crate::runtime_store::ProviderAutomaticEligibility> {
        let endpoint = &target.endpoint.provider_endpoint;
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .iter()
            .find(|projection| &projection.provider_endpoint == endpoint)
            .map(|projection| projection.automatic)
    }

    #[test]
    fn missing_usage_provider_file_uses_builtins_without_creating_a_file() {
        let (directory, path) = isolated_usage_provider_path("missing");

        let providers = load_providers_from_path(&path).expect("load in-memory built-ins");

        assert!(!providers.providers.is_empty());
        assert!(!path.exists(), "missing operator config must stay missing");
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }

    #[test]
    fn invalid_usage_provider_file_returns_error_without_rewriting_input() {
        let (directory, path) = isolated_usage_provider_path("invalid");
        let original = br#"{
  "providers": [{
    "id": "legacy-template",
    "kind": "new_api_user_self",
    "domains": ["example.com"],
    "headers": {"New-Api-User": "42"}
  }]
}"#;
        std::fs::write(&path, original).expect("write invalid operator config");

        let error = load_providers_from_path(&path).expect_err("invalid config must fail");

        let detail = format!("{error:#}");
        assert!(detail.contains("failed to parse usage provider configuration"));
        assert!(detail.contains("unknown field `headers`"));
        assert_eq!(
            std::fs::read(&path)
                .expect("read preserved operator config")
                .as_slice(),
            original
        );
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }

    #[test]
    fn unreadable_usage_provider_path_returns_contextual_error_without_replacement() {
        let (directory, path) = isolated_usage_provider_path("unreadable");
        std::fs::create_dir(&path).expect("create directory at config path");

        let error = load_providers_from_path(&path).expect_err("directory is not a readable file");

        assert!(
            format!("{error:#}").contains("failed to read usage provider configuration"),
            "unexpected error: {error:#}"
        );
        assert!(
            path.is_dir(),
            "loader must not replace operator-owned input"
        );
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }

    #[test]
    fn valid_usage_provider_file_is_loaded_without_rewriting_it() {
        let (directory, path) = isolated_usage_provider_path("valid");
        let original = br#"{
  "providers": [{
    "id": "newapi-dashboard",
    "kind": "new_api_user_self",
    "domains": ["example.com"],
    "new_api_user_id_env": "NEW_API_USER_ID"
  }]
}"#;
        std::fs::write(&path, original).expect("write operator config");

        let providers = load_providers_from_path(&path).expect("load operator config");

        assert_eq!(providers.providers.len(), 1);
        assert_eq!(
            providers.providers[0].new_api_user_id_env.as_deref(),
            Some("NEW_API_USER_ID")
        );
        assert_eq!(
            std::fs::read(&path)
                .expect("read preserved operator config")
                .as_slice(),
            original
        );
        std::fs::remove_dir_all(directory).expect("remove isolated test directory");
    }

    #[test]
    fn budget_snapshot_reports_monthly_budget_and_exhaustion() {
        let snapshot = budget_snapshot_from_json(
            &provider("packycode", ProviderKind::BudgetHttpJson),
            &upstream(),
            &serde_json::json!({
                "monthly_budget_usd": "10.50",
                "monthly_spent_usd": 10.5
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("10.5"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("10.5"));
    }

    #[test]
    fn budget_snapshot_keeps_missing_amounts_unknown() {
        let snapshot = budget_snapshot_from_json(
            &provider("packycode", ProviderKind::BudgetHttpJson),
            &upstream(),
            &serde_json::json!({}),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Unknown);
        assert_eq!(snapshot.exhausted, None);
    }

    #[test]
    fn yescode_snapshot_sums_subscription_and_paygo_balances() {
        let snapshot = yescode_snapshot_from_json(
            &provider("yescode", ProviderKind::YescodeProfile),
            &upstream(),
            &serde_json::json!({
                "subscription_balance": "1.25",
                "pay_as_you_go_balance": 2.5
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("3.75"));
        assert_eq!(snapshot.subscription_balance_usd.as_deref(), Some("1.25"));
        assert_eq!(snapshot.paygo_balance_usd.as_deref(), Some("2.5"));
    }

    #[test]
    fn openai_balance_endpoint_defaults_to_base_user_balance_without_v1() {
        let mut provider = provider("sub2api", ProviderKind::OpenAiBalanceHttpJson);
        provider.endpoint.clear();

        let endpoint =
            resolve_endpoint(&provider, "https://relay.example.com/v1", "token").expect("endpoint");

        assert_eq!(endpoint, "https://relay.example.com/user/balance");
    }

    #[test]
    fn sub2api_usage_endpoint_defaults_to_upstream_usage_under_v1() {
        let mut provider = provider("sub2api", ProviderKind::Sub2ApiUsage);
        provider.endpoint.clear();

        let endpoint =
            resolve_endpoint(&provider, "https://relay.example.com/v1", "token").expect("endpoint");

        assert_eq!(endpoint, "https://relay.example.com/v1/usage");
    }

    #[test]
    fn sub2api_auth_me_endpoint_defaults_to_dashboard_path_without_v1() {
        let mut provider = provider("sub2api-auth", ProviderKind::Sub2ApiAuthMe);
        provider.endpoint.clear();

        let endpoint =
            resolve_endpoint(&provider, "https://relay.example.com/v1", "token").expect("endpoint");

        assert_eq!(endpoint, "https://relay.example.com/api/v1/auth/me");
    }

    #[test]
    fn provider_relative_endpoint_resolves_against_normalized_base_url() {
        let mut provider = provider("newapi", ProviderKind::NewApiUserSelf);
        provider.endpoint = "/api/user/self?user=42".to_string();

        let endpoint = resolve_endpoint(&provider, "https://newapi.example.com/v1", "token")
            .expect("endpoint");

        assert_eq!(endpoint, "https://newapi.example.com/api/user/self?user=42");
    }

    #[test]
    fn new_api_token_usage_endpoint_defaults_to_model_key_usage_path() {
        let mut provider = provider("newapi-token", ProviderKind::NewApiTokenUsage);
        provider.endpoint.clear();

        let endpoint = resolve_endpoint(&provider, "https://newapi.example.com/v1", "token")
            .expect("endpoint");

        assert_eq!(endpoint, "https://newapi.example.com/api/usage/token/");
    }

    #[test]
    fn openai_organization_costs_endpoint_defaults_to_official_v1_costs_window() {
        let mut provider = provider("openai", ProviderKind::OpenAiOrganizationCosts);
        provider.endpoint.clear();
        provider.domains = vec!["api.openai.com".to_string()];

        let endpoint =
            resolve_endpoint(&provider, "https://api.openai.com/v1", "token").expect("endpoint");

        assert!(endpoint.starts_with("https://api.openai.com/v1/organization/costs?start_time="));
        assert!(endpoint.ends_with("&limit=30"));
        let start_time = endpoint
            .split("start_time=")
            .nth(1)
            .and_then(|value| value.split('&').next())
            .and_then(|value| value.parse::<u64>().ok())
            .expect("numeric start_time");
        assert!(start_time > 0);
    }

    #[test]
    fn openai_organization_costs_adapter_owns_its_query_window() {
        let mut provider = provider("openai", ProviderKind::OpenAiOrganizationCosts);
        provider.domains = vec!["api.openai.com".to_string()];
        provider.endpoint =
            "https://api.openai.com/v1/organization/costs?bucket=day&start_time=1&limit=1"
                .to_string();

        let endpoint =
            resolve_endpoint(&provider, "https://api.openai.com/v1", "token").expect("endpoint");
        let url = reqwest::Url::parse(&endpoint).expect("resolved endpoint URL");
        let query = url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();

        assert_eq!(query.get("bucket").map(String::as_str), Some("day"));
        assert_eq!(query.get("limit").map(String::as_str), Some("30"));
        assert!(
            query
                .get("start_time")
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|value| value > 1)
        );
    }

    #[test]
    fn provider_endpoint_rejects_untrusted_or_credential_bearing_urls() {
        let cases = [
            (
                "http://example.com/usage",
                "must use HTTPS outside loopback",
            ),
            (
                "https://user:password@example.com/usage",
                "must not contain userinfo",
            ),
            (
                "https://example.com/usage#private",
                "must not contain a fragment",
            ),
            (
                "https://untrusted.example.net/usage",
                "outside its matched domains",
            ),
            (
                "{{unsupported}}/usage",
                "endpoint templates are not supported",
            ),
            (
                "https://example.com/usage?api_key=literal-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?access_key=independent-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?api_token=independent-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?x-api-key=independent-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?secret_key=independent-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?private_key=independent-secret",
                "must not contain query credentials",
            ),
            (
                "https://example.com/usage?user=model-secret-token",
                "must not contain query credentials",
            ),
        ];

        for (endpoint, expected) in cases {
            let mut provider = provider("credential-boundary", ProviderKind::NewApiUserSelf);
            provider.endpoint = endpoint.to_string();
            let error = resolve_endpoint(&provider, "https://example.com/v1", "model-secret-token")
                .expect_err(endpoint);
            assert!(
                error.to_string().contains(expected),
                "unexpected error for {endpoint}: {error:#}"
            );
        }
    }

    #[test]
    fn provider_endpoint_allows_plain_http_only_for_loopback() {
        let mut provider = provider("loopback", ProviderKind::NewApiUserSelf);
        provider.domains = vec!["127.0.0.1".to_string()];
        provider.endpoint = "http://127.0.0.1:8080/usage?user=42".to_string();

        let endpoint = resolve_endpoint(&provider, "http://127.0.0.1:8080/v1", "model-secret")
            .expect("loopback endpoint");

        assert_eq!(endpoint, "http://127.0.0.1:8080/usage?user=42");
    }

    #[test]
    fn usage_provider_config_rejects_generic_headers_and_variables() {
        for field in ["headers", "variables"] {
            let mut input = serde_json::json!({
                "id": "closed-config",
                "kind": "new_api_user_self",
                "domains": ["example.com"]
            });
            input
                .as_object_mut()
                .expect("provider config object")
                .insert(field.to_string(), serde_json::json!({"arbitrary": "value"}));

            let error = serde_json::from_value::<UsageProviderConfig>(input)
                .expect_err("generic request customization must be rejected");
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "unexpected error for {field}: {error}"
            );
        }
    }

    #[test]
    fn new_api_user_self_uses_typed_user_id_and_fixed_headers() {
        let mut provider = provider("newapi", ProviderKind::NewApiUserSelf);
        provider.new_api_user_id_env = Some("NEW_API_USER_ID".to_string());

        let user_id = resolve_new_api_user_id_with(&provider, |name| {
            (name == "NEW_API_USER_ID").then(|| "42".to_string())
        })
        .expect("typed user id")
        .expect("configured user id");
        let headers = provider_request_headers(&provider, "dashboard-token", Some(&user_id))
            .expect("fixed provider headers");

        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer dashboard-token")
        );
        assert_eq!(
            headers
                .get("New-Api-User")
                .and_then(|value| value.to_str().ok()),
            Some("42")
        );
    }

    #[test]
    fn new_api_user_id_field_is_kind_scoped_and_required_when_configured() {
        let mut provider = provider("wrong-kind", ProviderKind::OpenAiBalanceHttpJson);
        provider.new_api_user_id_env = Some("NEW_API_USER_ID".to_string());
        let error = resolve_new_api_user_id_with(&provider, |_| Some("42".to_string()))
            .expect_err("field must be kind scoped");
        assert!(
            error
                .to_string()
                .contains("only for kind new_api_user_self")
        );

        provider.kind = ProviderKind::NewApiUserSelf;
        let error = resolve_new_api_user_id_with(&provider, |_| None)
            .expect_err("configured user id must be available");
        assert!(
            error
                .to_string()
                .contains("requires New-Api-User from environment variable 'NEW_API_USER_ID'")
        );
    }

    #[test]
    fn new_api_user_id_partitions_the_account_fingerprint() {
        let first = usage_provider_account_fingerprint("shared-token", Some("42"));
        let second = usage_provider_account_fingerprint("shared-token", Some("43"));
        let token_only = usage_provider_account_fingerprint("shared-token", None);

        assert_ne!(first, second);
        assert_ne!(first, token_only);
        assert!(first.starts_with("sha256:"));
    }

    #[tokio::test]
    async fn upstream_client_refuses_cross_origin_usage_provider_redirects() {
        let target_headers = Arc::new(Mutex::new(Vec::<axum::http::HeaderMap>::new()));
        let target_headers_for_route = target_headers.clone();
        let target = axum::Router::new().fallback(get(move |headers: axum::http::HeaderMap| {
            let target_headers = target_headers_for_route.clone();
            async move {
                target_headers.lock().expect("target headers").push(headers);
                axum::Json(serde_json::json!({"balance": 1}))
            }
        }));
        let (target_addr, target_handle) = spawn_axum_server(target).await;

        let source_hits = Arc::new(AtomicUsize::new(0));
        let source_hits_for_route = source_hits.clone();
        let redirect_location = format!("http://{target_addr}/capture");
        let source = axum::Router::new().fallback(get(move || {
            let source_hits = source_hits_for_route.clone();
            let redirect_location = redirect_location.clone();
            async move {
                source_hits.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::FOUND,
                    [(axum::http::header::LOCATION, redirect_location)],
                )
            }
        }));
        let (source_addr, source_handle) = spawn_axum_server(source).await;
        let endpoint = format!("http://{source_addr}/usage");
        let client = crate::proxy::upstream_http_client_builder()
            .build()
            .expect("build production upstream client");

        for kind in [
            ProviderKind::OpenAiBalanceHttpJson,
            ProviderKind::YescodeProfile,
        ] {
            let provider = provider("redirect-boundary", kind);
            let error =
                poll_provider_http_json(&client, &provider, &endpoint, None, "model-secret")
                    .await
                    .expect_err("redirect must remain an upstream response");
            assert!(
                error.to_string().contains("HTTP 302"),
                "unexpected redirect error: {error:#}"
            );
        }

        assert_eq!(source_hits.load(Ordering::SeqCst), 2);
        assert!(
            target_headers.lock().expect("target headers").is_empty(),
            "redirect target received provider credentials or metadata"
        );
        source_handle.abort();
        target_handle.abort();
    }

    #[test]
    fn require_token_env_prevents_upstream_model_key_fallback() {
        let mut target = usage_provider_target("https://api.openai.com/v1", "right");
        target.auth.auth_token = Some("model-key".to_string());

        let mut provider = provider("openai", ProviderKind::OpenAiOrganizationCosts);
        provider.token_env = Some("__CODEX_HELPER_TEST_MISSING_TOKEN_ENV__".to_string());
        provider.require_token_env = true;

        assert_eq!(resolve_token(&provider, &target), None);

        provider.require_token_env = false;
        assert_eq!(
            resolve_token(&provider, &target).as_deref(),
            Some("model-key")
        );
    }

    #[test]
    fn effective_poll_interval_respects_disable_flag_zero_and_minimum() {
        let mut provider = provider("sub2api", ProviderKind::OpenAiBalanceHttpJson);

        provider.poll_interval_secs = Some(0);
        assert_eq!(effective_poll_interval_secs(&provider), None);

        provider.poll_interval_secs = Some(10);
        assert_eq!(
            effective_poll_interval_secs(&provider),
            Some(MIN_POLL_INTERVAL_SECS)
        );

        provider.poll_interval_secs = None;
        assert_eq!(
            effective_poll_interval_secs(&provider),
            Some(DEFAULT_POLL_INTERVAL_SECS)
        );

        provider.refresh_on_request = false;
        assert_eq!(effective_poll_interval_secs(&provider), None);
    }

    #[test]
    fn usage_provider_http_error_detail_extracts_json_code_and_message() {
        let detail = usage_provider_http_error_detail(
            r#"{"code":"USER_INACTIVE","message":"User account is not active"}"#,
        )
        .expect("error detail");

        assert_eq!(detail, "USER_INACTIVE: User account is not active");
        assert!(usage_provider_error_is_terminal(&detail));
    }

    #[test]
    fn terminal_provider_error_does_not_become_routing_exhaustion() {
        let snapshot = ProviderBalanceSnapshot {
            fetched_at_ms: 1_000,
            stale_after_ms: Some(2_000),
            error: Some("USER_INACTIVE: User account is not active".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, 1_500).is_none(),
            "poll failures may back off polling but must not create route eligibility effects"
        );
    }

    #[test]
    fn current_day_quota_exhaustion_blocks_followup_usage_even_when_display_only() {
        let mut snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            ..ProviderBalanceSnapshot::default()
        };
        snapshot.refresh_status(0);

        assert!(!snapshot.routing_exhausted());
        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, snapshot.fetched_at_ms)
                .is_some()
        );
        assert_eq!(
            usage_provider_snapshot_suppression_reason(&snapshot).as_deref(),
            Some("daily package quota exhausted for current period")
        );
    }

    #[test]
    fn daily_quota_reset_uses_default_fixed_offset_midnight() {
        let now_ms = 10 * HOUR_MS;

        let reset_at_ms = next_daily_quota_reset_at_ms(now_ms).expect("default reset");

        assert_eq!(reset_at_ms, 16 * HOUR_MS);
    }

    #[test]
    fn fixed_offset_reset_uses_configured_clock_and_timezone() {
        let now_ms = 10 * HOUR_MS;

        let reset_at_ms =
            next_fixed_offset_reset_at_ms(now_ms, "-05:00", "06:30").expect("fixed reset");

        assert_eq!(reset_at_ms, 11 * HOUR_MS + 30 * MINUTE_MS);
    }

    #[test]
    fn current_day_quota_suppression_expires_after_default_reset() {
        let fetched_at_ms = 1_700_000_000_000;
        let reset_at_ms =
            next_daily_quota_reset_at_ms(fetched_at_ms).expect("default reset is valid");
        let suppress_until_ms = reset_at_ms;

        let mut snapshot = ProviderBalanceSnapshot {
            fetched_at_ms,
            stale_after_ms: Some(suppress_until_ms + 60_000),
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            ..ProviderBalanceSnapshot::default()
        };
        snapshot.refresh_status(fetched_at_ms);

        let decision =
            usage_provider_suppression_decision_from_snapshot(&snapshot, suppress_until_ms - 1)
                .expect("daily exhaustion should suppress until default reset");
        assert_eq!(decision.ttl, Duration::from_millis(1));

        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, suppress_until_ms)
                .is_none(),
            "a fresh-looking daily exhaustion snapshot must not suppress refresh after reset"
        );
    }

    #[test]
    fn stale_non_daily_exhaustion_snapshot_does_not_renew_suppression() {
        let mut snapshot = ProviderBalanceSnapshot {
            fetched_at_ms: 1_000,
            stale_after_ms: Some(2_000),
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            quota_period: Some("quota".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            ..ProviderBalanceSnapshot::default()
        };
        snapshot.refresh_status(1_000);

        let fresh_decision = usage_provider_suppression_decision_from_snapshot(&snapshot, 1_500)
            .expect("fresh exhausted quota should suppress follow-up polling");
        assert_eq!(fresh_decision.ttl, USAGE_PROVIDER_EXHAUSTED_SUPPRESSION_TTL);

        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, 2_001).is_none(),
            "stale exhausted quota snapshots must not be used to renew suppression forever"
        );
    }

    #[test]
    fn refreshable_weekly_window_exhaustion_suppresses_only_while_snapshot_is_fresh() {
        let mut snapshot = ProviderBalanceSnapshot {
            fetched_at_ms: 1_000,
            stale_after_ms: Some(10_000),
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            quota_period: Some("weekly".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            ..ProviderBalanceSnapshot::default()
        };
        snapshot.refresh_status(1_000);

        let decision = usage_provider_suppression_decision_from_snapshot(&snapshot, 4_000)
            .expect("fresh weekly window exhaustion should block follow-up usage");
        assert_eq!(decision.ttl, Duration::from_millis(6_000));

        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, 10_001).is_none(),
            "weekly/monthly windows without explicit reset_at should be re-queried after staleness"
        );
    }

    #[test]
    fn rate_limit_reset_at_drives_suppression_ttl() {
        let reset_at_ms = 120_000;
        let suppress_until_ms =
            reset_at_ms + duration_millis_u64(USAGE_PROVIDER_DAILY_RESET_SUPPRESSION_GRACE);
        let mut snapshot = ProviderBalanceSnapshot {
            fetched_at_ms: 1_000,
            stale_after_ms: Some(10_000),
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            quota_period: Some("rate_limit:5h".to_string()),
            quota_resets_at_ms: Some(reset_at_ms),
            ..ProviderBalanceSnapshot::default()
        };
        snapshot.refresh_status(1_000);

        let decision =
            usage_provider_suppression_decision_from_snapshot(&snapshot, suppress_until_ms - 1)
                .expect("rate limit should suppress until reset grace expires");
        assert_eq!(decision.ttl, Duration::from_millis(1));

        assert!(
            usage_provider_suppression_decision_from_snapshot(&snapshot, suppress_until_ms)
                .is_none()
        );
    }

    #[test]
    fn auto_provider_uses_canonical_provider_id_across_probe_kinds() {
        let target =
            usage_provider_target_at("secondary", 2, "https://ai.input.im/v1", "input/sub");

        let sub2api = auto_usage_provider(&target, ProviderKind::Sub2ApiUsage);
        let newapi_token = auto_usage_provider(&target, ProviderKind::NewApiTokenUsage);
        let newapi = auto_usage_provider(&target, ProviderKind::NewApiUserSelf);

        assert_eq!(sub2api.id, "input/sub");
        assert_eq!(sub2api.id, newapi_token.id);
        assert_eq!(sub2api.id, newapi.id);
        assert_eq!(sub2api.domains, vec!["ai.input.im".to_string()]);
        assert_eq!(
            resolve_endpoint(&sub2api, &target.base_url, "token").unwrap(),
            "https://ai.input.im/v1/usage"
        );
        assert_eq!(
            resolve_endpoint(&newapi_token, &target.base_url, "token").unwrap(),
            "https://ai.input.im/api/usage/token/"
        );
    }

    #[test]
    fn auto_probe_prefers_rightcode_adapter_for_rightcode_hosts() {
        let target = usage_provider_target("https://www.right.codes/codex/v1", "right");

        assert_eq!(
            first_auto_probe_kind(&target),
            ProviderKind::RightCodeAccountSummary
        );
        assert_eq!(
            resolve_endpoint(
                &auto_usage_provider(&target, ProviderKind::RightCodeAccountSummary),
                &target.base_url,
                "token"
            )
            .unwrap(),
            "https://www.right.codes/account/summary"
        );
        assert_eq!(
            auto_usage_provider(&target, ProviderKind::RightCodeAccountSummary)
                .token_env
                .as_deref(),
            None
        );
    }

    #[test]
    fn auto_provider_id_uses_canonical_endpoint_identity() {
        let target = usage_provider_target("https://ai.input.im/v1", "input");

        let provider = auto_usage_provider(&target, ProviderKind::Sub2ApiUsage);

        assert_eq!(provider.id, "input");
    }

    #[test]
    fn auto_target_provider_id_filter_matches_canonical_provider_id() {
        let target = usage_provider_target_at("default", 6, "https://input.9z1.me/v1", "input6");

        assert!(auto_target_matches_provider_id_filter(&target, None));
        assert!(auto_target_matches_provider_id_filter(
            &target,
            Some("input6")
        ));
        assert!(!auto_target_matches_provider_id_filter(
            &target,
            Some("input5")
        ));
    }

    #[test]
    fn auto_target_provider_id_filter_does_not_generate_compatibility_id() {
        let target = usage_provider_target_at("default", 6, "https://input.9z1.me/v1", "input6");

        assert!(auto_target_matches_provider_id_filter(
            &target,
            Some("input6")
        ));
        assert!(!auto_target_matches_provider_id_filter(
            &target,
            Some("auto:balance:routing:6")
        ));
    }

    #[test]
    fn provider_endpoint_target_lookup_uses_endpoint_identity() {
        let cfg = helper_config(vec![
            ("input", provider_config("https://input.example/v1")),
            ("right", provider_config("https://right.example/v1")),
        ]);

        let target = usage_provider_target_for_provider_endpoint(
            &cfg,
            "codex",
            &ProviderEndpointKey::new("codex", "right", "default"),
        )
        .expect("provider endpoint target");

        assert_eq!(target.endpoint.provider_endpoint.provider_id, "right");
        assert_eq!(target.endpoint.catalog_index, 0);
        assert_eq!(
            target.endpoint.provider_endpoint.stable_key(),
            "codex/right/default"
        );
        assert_eq!(target.base_url, "https://right.example/v1");
    }

    #[test]
    fn canonical_targets_preserve_auth_endpoint_overrides_and_priority_order() {
        let mut provider = provider_endpoint_config("https://slow.example/v1", "slow");
        provider.auth.auth_token = Some("block-token".to_string());
        provider.inline_auth.auth_token = Some("inline-token".to_string());
        provider
            .tags
            .insert("region".to_string(), "provider".to_string());
        provider
            .tags
            .insert("tier".to_string(), "provider".to_string());
        provider.supported_models.insert("gpt-*".to_string(), true);
        provider
            .model_mapping
            .insert("gpt-external".to_string(), "gpt-provider".to_string());
        let slow = provider.endpoints.get_mut("slow").expect("slow endpoint");
        slow.priority = 20;
        slow.tags.insert("tier".to_string(), "slow".to_string());
        slow.supported_models.insert("gpt-*".to_string(), false);
        slow.model_mapping
            .insert("gpt-external".to_string(), "gpt-slow".to_string());
        provider.endpoints.insert(
            "fast".to_string(),
            ProviderEndpointConfig {
                priority: 10,
                tags: BTreeMap::from([("tier".to_string(), "fast".to_string())]),
                ..endpoint_config("https://fast.example/v1")
            },
        );
        provider.endpoints.insert(
            "disabled".to_string(),
            ProviderEndpointConfig {
                enabled: false,
                ..endpoint_config("https://disabled.example/v1")
            },
        );
        let cfg = helper_config(vec![("relay", provider)]);

        let targets = usage_provider_targets(&cfg, "codex", Some("relay"));

        assert_eq!(
            targets
                .iter()
                .map(|target| target.endpoint.provider_endpoint.endpoint_id.as_str())
                .collect::<Vec<_>>(),
            vec!["fast", "slow"]
        );
        assert_eq!(
            targets[0].auth.resolve_auth_token().as_deref(),
            Some("inline-token")
        );
        assert_eq!(
            targets[0].tags.get("region").map(String::as_str),
            Some("provider")
        );
        assert_eq!(
            targets[0].tags.get("tier").map(String::as_str),
            Some("fast")
        );
        assert_eq!(targets[1].supported_models.get("gpt-*"), Some(&false));
        assert_eq!(
            targets[1]
                .model_mapping
                .get("gpt-external")
                .map(String::as_str),
            Some("gpt-slow")
        );
    }

    #[test]
    fn request_balance_queue_deduplicates_until_due() {
        let key = ProviderEndpointKey::new("codex", "input", "default");
        let queue = REQUEST_BALANCE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()));
        {
            let mut queue = queue.lock().expect("queue");
            queue.remove(&key);
        }

        assert_eq!(
            enqueue_request_balance_refresh(key.clone()),
            Some(REQUEST_BALANCE_REFRESH_DELAY)
        );
        assert_eq!(enqueue_request_balance_refresh(key.clone()), None);
        assert!(matches!(
            take_request_balance_refresh_if_due(&key),
            RequestBalanceQueueDue::NotDue(_)
        ));

        {
            let mut queue = queue.lock().expect("queue");
            queue.insert(key.clone(), Instant::now() - Duration::from_secs(1));
        }

        assert_eq!(
            take_request_balance_refresh_if_due(&key),
            RequestBalanceQueueDue::Due
        );
        assert_eq!(
            take_request_balance_refresh_if_due(&key),
            RequestBalanceQueueDue::Missing
        );
        assert_eq!(
            enqueue_request_balance_refresh(key.clone()),
            Some(REQUEST_BALANCE_REFRESH_DELAY)
        );

        queue.lock().expect("queue").remove(&key);
    }

    #[test]
    fn request_balance_queue_does_not_extend_due_refresh() {
        let key = ProviderEndpointKey::new("codex", "input", "default");
        let queue = REQUEST_BALANCE_QUEUE.get_or_init(|| Mutex::new(HashMap::new()));
        {
            let mut queue = queue.lock().expect("queue");
            queue.insert(key.clone(), Instant::now() - Duration::from_secs(1));
        }

        assert_eq!(
            enqueue_request_balance_refresh(key.clone()),
            Some(Duration::ZERO)
        );
        assert_eq!(
            take_request_balance_refresh_if_due(&key),
            RequestBalanceQueueDue::Due
        );
        assert_eq!(
            take_request_balance_refresh_if_due(&key),
            RequestBalanceQueueDue::Missing
        );

        queue.lock().expect("queue").remove(&key);
    }

    #[test]
    fn remaining_poll_cooldown_returns_only_unexpired_window() {
        let now = Instant::now();
        assert_eq!(
            remaining_poll_cooldown(now - Duration::from_secs(30), 60, now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            remaining_poll_cooldown(now - Duration::from_secs(60), 60, now),
            None
        );
        assert_eq!(
            remaining_poll_cooldown(now - Duration::from_secs(61), 60, now),
            None
        );
    }

    #[test]
    fn configured_target_keys_prevent_auto_probe_for_explicit_balance_domains() {
        let cfg = helper_config(vec![
            ("explicit", provider_config("https://example.com/v1")),
            ("auto", provider_config("https://ai.input.im/v1")),
        ]);
        let configured = configured_target_keys(
            &cfg,
            "codex",
            &[provider("relay", ProviderKind::OpenAiBalanceHttpJson)],
            None,
        );
        let auto_targets = usage_provider_targets(&cfg, "codex", None)
            .into_iter()
            .filter(|target| !configured.contains(&target_key(target)))
            .map(|target| target.endpoint.provider_endpoint.provider_id)
            .collect::<Vec<_>>();

        assert_eq!(auto_targets, vec!["auto".to_string()]);
    }

    #[test]
    fn auto_probe_accepts_only_usable_balance_snapshots() {
        let usable = sub2api_usage_snapshot_from_json(
            &provider("auto", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "remaining": 1
            }),
            100,
            Some(1_000),
        );
        let unusable = balance_http_snapshot_from_json(
            &provider("auto", ProviderKind::OpenAiBalanceHttpJson),
            &upstream(),
            &serde_json::json!({ "ok": true }),
            100,
            Some(1_000),
        );

        assert!(auto_snapshot_is_usable(&usable));
        assert!(!auto_snapshot_is_usable(&unusable));
    }

    #[test]
    fn auto_probe_error_summary_keeps_all_attempt_failures() {
        let errors = vec![
            "Sub2ApiUsage: HTTP 404".to_string(),
            "NewApiTokenUsage: missing quota fields".to_string(),
            "OpenAiBalanceHttpJson: non-JSON response".to_string(),
        ];

        let summary = auto_probe_error_summary(&errors).expect("summary");

        assert!(summary.contains("Sub2ApiUsage: HTTP 404"));
        assert!(summary.contains("NewApiTokenUsage: missing quota fields"));
        assert!(summary.contains("OpenAiBalanceHttpJson: non-JSON response"));
    }

    #[test]
    fn auto_probe_kind_order_prioritizes_remembered_success() {
        let provider_id = "input-order-success";
        clear_auto_probe_kind_state(provider_id);
        let target = usage_provider_target("https://relay.example.com/v1", provider_id);

        remember_auto_probe_kind_success(provider_id, &target, ProviderKind::NewApiUserSelf);

        let order = auto_probe_kind_order(provider_id, &target, false);

        assert_eq!(order.first(), Some(&ProviderKind::NewApiUserSelf));
        assert_eq!(
            order
                .iter()
                .filter(|kind| **kind == ProviderKind::Sub2ApiUsage)
                .count(),
            1
        );
        clear_auto_probe_kind_state(provider_id);
    }

    #[test]
    fn auto_probe_kind_order_temporarily_skips_recent_failures() {
        let provider_id = "input-order-failure";
        clear_auto_probe_kind_state(provider_id);
        let target = usage_provider_target("https://relay.example.com/v1", provider_id);
        let now = Instant::now();

        remember_auto_probe_kind_failure(provider_id, &target, ProviderKind::Sub2ApiUsage, now);

        let order = auto_probe_kind_order(provider_id, &target, false);

        assert!(!order.contains(&ProviderKind::Sub2ApiUsage));
        assert!(order.contains(&ProviderKind::NewApiTokenUsage));

        if let Some(failures) = AUTO_PROBE_KIND_FAILURES.get()
            && let Ok(mut failures) = failures.lock()
        {
            failures.insert(
                AutoProbeKindFailureKey {
                    provider_id: provider_id.to_string(),
                    target: auto_probe_target_key(&target),
                    kind: ProviderKind::Sub2ApiUsage,
                },
                now - AUTO_PROBE_KIND_FAILURE_TTL - Duration::from_secs(1),
            );
        }

        let order = auto_probe_kind_order(provider_id, &target, false);
        assert!(order.contains(&ProviderKind::Sub2ApiUsage));
        clear_auto_probe_kind_state(provider_id);
    }

    #[test]
    fn auto_probe_kind_order_can_be_empty_when_all_kinds_are_suppressed() {
        let provider_id = "input-order-all-suppressed";
        clear_auto_probe_kind_state(provider_id);
        let target = usage_provider_target("https://relay.example.com/v1", provider_id);
        let now = Instant::now();

        for kind in AUTO_PROBE_KINDS {
            if kind == ProviderKind::RightCodeAccountSummary {
                continue;
            }
            remember_auto_probe_kind_failure(provider_id, &target, kind, now);
        }

        assert!(auto_probe_kind_order(provider_id, &target, false).is_empty());
        assert!(
            !auto_probe_kind_order(provider_id, &target, true).is_empty(),
            "force refresh should bypass temporary probe-kind failures"
        );
        clear_auto_probe_kind_state(provider_id);
    }

    #[test]
    fn auto_probe_kind_failures_do_not_suppress_distinct_targets_with_same_provider_id() {
        let provider_id = "input-shared-provider";
        clear_auto_probe_kind_state(provider_id);
        let routing_target =
            usage_provider_target_at("routing", 0, "https://relay.example.com/v1", provider_id);
        let catalog_target =
            usage_provider_target_at("input", 0, "https://relay.example.com/v1", provider_id);
        let now = Instant::now();

        for kind in AUTO_PROBE_KINDS {
            if kind == ProviderKind::RightCodeAccountSummary {
                continue;
            }
            remember_auto_probe_kind_failure(provider_id, &routing_target, kind, now);
        }

        assert!(auto_probe_kind_order(provider_id, &routing_target, false).is_empty());
        assert!(
            !auto_probe_kind_order(provider_id, &catalog_target, false).is_empty(),
            "a routing target's temporary failures must not hide catalog balance probes for the same provider"
        );
        clear_auto_probe_kind_state(provider_id);
    }

    #[tokio::test]
    async fn auto_probe_suppressed_order_records_error_snapshot() {
        let provider_id = "input-suppressed-snapshot";
        clear_auto_probe_kind_state(provider_id);
        let state = ProxyState::new();
        let mut target = usage_provider_target("https://relay.example.com/v1", provider_id);
        target.auth.auth_token = Some("model-key".to_string());
        let now = Instant::now();

        for kind in AUTO_PROBE_KINDS {
            if kind == ProviderKind::RightCodeAccountSummary {
                continue;
            }
            remember_auto_probe_kind_failure(provider_id, &target, kind, now);
        }

        let outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(outcome, UsageProviderRefreshOutcome::Failed);
        let view = state.get_provider_balance_view("codex").await;
        let snapshot = view
            .iter()
            .find(|snapshot| {
                provider_balance_snapshot_matches_target(snapshot, provider_id, &target)
            })
            .expect("suppressed auto probe snapshot");
        assert_eq!(snapshot.status, BalanceSnapshotStatus::Error);
        assert_eq!(
            snapshot.error.as_deref(),
            Some("all balance probe kinds are temporarily suppressed")
        );
        assert_eq!(snapshot.provider_endpoint.endpoint_id, "default");
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            None
        );
        clear_auto_probe_kind_state(provider_id);
    }

    #[tokio::test]
    async fn auto_probe_terminal_auth_failure_does_not_change_eligibility_during_backoff() {
        let provider_id = "input-terminal-auth";
        clear_auto_probe_kind_state(provider_id);
        let request_count = Arc::new(AtomicUsize::new(0));
        let counter = request_count.clone();
        let app = axum::Router::new().fallback(get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({
                        "code": "USER_INACTIVE",
                        "message": "User account is not active"
                    })),
                )
            }
        }));
        let (addr, handle) = spawn_axum_server(app).await;
        let base_url = format!("http://{addr}/v1");
        let state = ProxyState::new();
        let mut target = usage_provider_target(&base_url, provider_id);
        target.auth.auth_token = Some("model-key".to_string());

        let outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(outcome, UsageProviderRefreshOutcome::Failed);
        assert!(request_count.load(Ordering::SeqCst) > 0);
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            Some(crate::runtime_store::ProviderAutomaticEligibility::Eligible)
        );
        let view = state.get_provider_balance_view("codex").await;
        let snapshot = view
            .iter()
            .find(|snapshot| {
                provider_balance_snapshot_matches_target(snapshot, provider_id, &target)
            })
            .expect("terminal auth failure snapshot");
        assert!(
            snapshot
                .error
                .as_deref()
                .unwrap_or("")
                .contains("User account is not active")
        );

        let requests_after_first_probe = request_count.load(Ordering::SeqCst);
        let suppressed_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(suppressed_outcome, UsageProviderRefreshOutcome::Failed);
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            requests_after_first_probe
        );
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            Some(crate::runtime_store::ProviderAutomaticEligibility::Eligible)
        );

        let forced_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", true).await;

        assert_eq!(forced_outcome, UsageProviderRefreshOutcome::Failed);
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            requests_after_first_probe,
            "force refresh must not bypass terminal auth/balance suppression"
        );
        clear_auto_probe_kind_state(provider_id);
        handle.abort();
    }

    #[tokio::test]
    async fn auto_probe_daily_package_exhaustion_suppresses_poll_without_blocking_route() {
        let provider_id = "input-daily-exhausted";
        clear_auto_probe_kind_state(provider_id);
        let request_count = Arc::new(AtomicUsize::new(0));
        let counter = request_count.clone();
        let app = axum::Router::new().fallback(get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "isValid": true,
                    "mode": "unrestricted",
                    "planName": "CodeX Lite",
                    "subscription": {
                        "daily_usage_usd": 100,
                        "daily_limit_usd": 100,
                        "weekly_usage_usd": 100,
                        "weekly_limit_usd": 0,
                        "monthly_usage_usd": 100,
                        "monthly_limit_usd": 0
                    }
                }))
            }
        }));
        let (addr, handle) = spawn_axum_server(app).await;
        let base_url = format!("http://{addr}/v1");
        let state = ProxyState::new();
        let mut target = usage_provider_target(&base_url, provider_id);
        target.auth.auth_token = Some("model-key".to_string());

        let outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(outcome, UsageProviderRefreshOutcome::Refreshed);
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            Some(crate::runtime_store::ProviderAutomaticEligibility::Eligible)
        );
        let view = state.get_provider_balance_view("codex").await;
        let snapshot = view
            .iter()
            .find(|snapshot| {
                provider_balance_snapshot_matches_target(snapshot, provider_id, &target)
            })
            .expect("daily exhausted snapshot");
        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
        assert!(snapshot.routing_ignored_exhaustion());

        let suppressed_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(suppressed_outcome, UsageProviderRefreshOutcome::Failed);
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            Some(crate::runtime_store::ProviderAutomaticEligibility::Eligible)
        );

        let forced_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", true).await;

        assert_eq!(forced_outcome, UsageProviderRefreshOutcome::Failed);
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            1,
            "force refresh must not re-query a snapshot that still proves today's package is exhausted"
        );
        clear_auto_probe_kind_state(provider_id);
        handle.abort();
    }

    #[tokio::test]
    async fn force_auto_probe_refreshes_orphaned_active_daily_suppression() {
        let provider_id = "input-orphaned-daily-suppression";
        clear_auto_probe_kind_state(provider_id);
        let request_count = Arc::new(AtomicUsize::new(0));
        let counter = request_count.clone();
        let app = axum::Router::new().fallback(get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "isValid": true,
                    "mode": "unrestricted",
                    "planName": "CodeX Lite",
                    "remaining": 12.5,
                    "subscription": {
                        "daily_usage_usd": 0,
                        "daily_limit_usd": 100,
                        "weekly_usage_usd": 0,
                        "weekly_limit_usd": 0,
                        "monthly_usage_usd": 0,
                        "monthly_limit_usd": 0
                    }
                }))
            }
        }));
        let (addr, handle) = spawn_axum_server(app).await;
        let base_url = format!("http://{addr}/v1");
        let state = ProxyState::new();
        let mut target = usage_provider_target(&base_url, provider_id);
        target.auth.auth_token = Some("model-key".to_string());

        remember_usage_provider_target_suppression(
            provider_id,
            &target,
            Duration::from_secs(60),
            "daily package quota exhausted for current period",
            Instant::now(),
        );

        let suppressed_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", false).await;

        assert_eq!(suppressed_outcome, UsageProviderRefreshOutcome::Failed);
        assert_eq!(request_count.load(Ordering::SeqCst), 0);

        let forced_outcome =
            auto_probe_provider_target(&Client::new(), &target, &state, "codex", true).await;

        assert_eq!(forced_outcome, UsageProviderRefreshOutcome::Refreshed);
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            target_automatic_eligibility(state.as_ref(), &target).await,
            Some(crate::runtime_store::ProviderAutomaticEligibility::Eligible)
        );
        clear_auto_probe_kind_state(provider_id);
        handle.abort();
    }

    #[test]
    fn openai_balance_snapshot_reads_common_sub2api_balance_shape() {
        let snapshot = balance_http_snapshot_from_json(
            &provider("sub2api", ProviderKind::OpenAiBalanceHttpJson),
            &upstream(),
            &serde_json::json!({
                "balance": "1.25"
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("1.25"));
    }

    #[test]
    fn json_path_supports_array_indices_for_official_balance_shapes() {
        let value = serde_json::json!({
            "balance_infos": [
                { "currency": "CNY", "total_balance": "3.25" }
            ]
        });

        assert_eq!(
            json_value_at_path(&value, "balance_infos.0.total_balance")
                .and_then(|value| value.as_str()),
            Some("3.25")
        );
    }

    #[test]
    fn openai_balance_snapshot_reads_cc_switch_official_balance_shapes() {
        let snapshot = balance_http_snapshot_from_json(
            &provider("deepseek", ProviderKind::OpenAiBalanceHttpJson),
            &upstream(),
            &serde_json::json!({
                "balance_infos": [
                    { "currency": "CNY", "total_balance": "3.25" }
                ],
                "is_available": true
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("3.25"));

        let snapshot = balance_http_snapshot_from_json(
            &provider("siliconflow", ProviderKind::OpenAiBalanceHttpJson),
            &upstream(),
            &serde_json::json!({
                "code": 20000,
                "data": {
                    "totalBalance": "8.5",
                    "chargeBalance": "2.5"
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("8.5"));
        assert_eq!(snapshot.paygo_balance_usd.as_deref(), Some("2.5"));
    }

    #[test]
    fn openai_balance_snapshot_can_derive_remaining_from_total_and_used() {
        let mut provider = provider("openrouter", ProviderKind::OpenAiBalanceHttpJson);
        provider.extract.monthly_budget_paths = vec!["data.total_credits".to_string()];
        provider.extract.monthly_spent_paths = vec!["data.total_usage".to_string()];
        provider.extract.derive_remaining_from_budget_and_spent = true;

        let snapshot = balance_http_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "data": {
                    "total_credits": "10",
                    "total_usage": "4"
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("6"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("10"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("4"));
    }

    #[test]
    fn openai_balance_snapshot_supports_divisor_for_minor_units() {
        let mut provider = provider("novita", ProviderKind::OpenAiBalanceHttpJson);
        provider.extract.remaining_balance_paths = vec!["availableBalance".to_string()];
        provider.extract.remaining_divisor = Some(10_000);

        let snapshot = balance_http_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "availableBalance": 12345
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("1.2345"));
    }

    #[test]
    fn sub2api_usage_snapshot_reads_all_api_hub_usage_shape() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "unrestricted",
                "planName": "CodeX Air",
                "remaining": 165.0877165,
                "usage": {
                    "today": {
                        "cost": 0,
                        "requests": 0,
                        "total_tokens": 0
                    },
                    "total": {
                        "cost": 354.194748,
                        "requests": 2691,
                        "total_tokens": 384084697
                    }
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.plan_name.as_deref(), Some("CodeX Air"));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("165.0877165"));
        assert_eq!(snapshot.total_used_usd.as_deref(), Some("354.194748"));
        assert_eq!(snapshot.today_used_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.total_requests, Some(2691));
        assert_eq!(snapshot.today_requests, Some(0));
        assert_eq!(snapshot.total_tokens, Some(384084697));
        assert_eq!(snapshot.today_tokens, Some(0));
    }

    #[test]
    fn sub2api_usage_snapshot_reads_rates_model_stats_windows_and_alerts() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "unrestricted",
                "plan_name": "CodeX Pro",
                "remaining": 9,
                "subscription": {
                    "daily_usage_usd": 95,
                    "daily_limit_usd": 100,
                    "weekly_usage_usd": "120.5",
                    "weekly_limit_usd": 0,
                    "monthly_usage_usd": 300.25,
                    "monthly_limit_usd": 1000,
                    "expires_at": "2026-05-09T12:00:00.000Z"
                },
                "usage": {
                    "today": {
                        "request_count": "7",
                        "input_tokens": 100,
                        "output_tokens": 25,
                        "total_cost_usd": "1.5"
                    },
                    "total": {
                        "requests": 42,
                        "tokens": 1234,
                        "cost": 9.25
                    },
                    "average_duration_ms": "842.7",
                    "rpm": "0.7",
                    "tpm": 85.3
                },
                "model_stats": [
                    {
                        "model": "gpt-4o-mini",
                        "request_count": "7",
                        "prompt_tokens": 100,
                        "completion_tokens": 25,
                        "input_cost": "0.12",
                        "output_cost": "0.34"
                    }
                ]
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.plan_name.as_deref(), Some("CodeX Pro"));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("9"));
        assert_eq!(snapshot.today_requests, Some(7));
        assert_eq!(snapshot.today_tokens, Some(100));
        assert_eq!(snapshot.today_used_usd.as_deref(), Some("1.5"));
        assert_eq!(snapshot.total_requests, Some(42));
        assert_eq!(snapshot.total_tokens, Some(1234));
        assert_eq!(snapshot.total_used_usd.as_deref(), Some("9.25"));
        let rate = snapshot.usage_rate.expect("rate");
        assert_eq!(rate.average_duration_ms.as_deref(), Some("842.7"));
        assert_eq!(rate.rpm.as_deref(), Some("0.7"));
        assert_eq!(rate.tpm.as_deref(), Some("85.3"));
        assert_eq!(snapshot.usage_windows.len(), 3);
        assert_eq!(snapshot.usage_windows[0].period, "daily");
        assert_eq!(
            snapshot.usage_windows[0].remaining_usd.as_deref(),
            Some("5")
        );
        assert_eq!(snapshot.usage_windows[1].unlimited, Some(true));
        assert_eq!(snapshot.usage_model_stats.len(), 1);
        assert_eq!(snapshot.usage_model_stats[0].model, "gpt-4o-mini");
        assert_eq!(snapshot.usage_model_stats[0].request_count, Some(7));
        assert_eq!(snapshot.usage_model_stats[0].total_tokens, Some(125));
        assert_eq!(
            snapshot.usage_model_stats[0].total_cost_usd.as_deref(),
            Some("0.46")
        );
        assert_eq!(
            snapshot
                .usage_alerts
                .iter()
                .map(|alert| alert.kind)
                .collect::<Vec<_>>(),
            vec![
                ProviderUsageAlertKind::DailyUsage95,
                ProviderUsageAlertKind::LowBalance,
                ProviderUsageAlertKind::SubscriptionExpired,
            ]
        );
    }

    #[test]
    fn sub2api_subscription_lazy_daily_reset_projects_today_capacity() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "unrestricted",
                "planName": "CodeX Lite 年度",
                "remaining": 0,
                "subscription": {
                    "daily_usage_usd": 100.468025,
                    "daily_limit_usd": 100,
                    "weekly_usage_usd": 401.441684,
                    "weekly_limit_usd": 0,
                    "monthly_usage_usd": 401.441684,
                    "monthly_limit_usd": 0
                },
                "usage": {
                    "today": { "cost": 0, "requests": 0, "total_tokens": 0 },
                    "total": { "cost": 702.492098, "requests": 42, "total_tokens": 1234 }
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.plan_name.as_deref(), Some("CodeX Lite 年度"));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("100"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("100"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("100"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.total_used_usd.as_deref(), Some("702.492098"));
        assert_eq!(snapshot.today_used_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.today_requests, Some(0));
        assert_eq!(snapshot.today_tokens, Some(0));
        assert_eq!(snapshot.usage_windows[0].period, "daily");
        assert_eq!(snapshot.usage_windows[0].used_usd.as_deref(), Some("0"));
        assert_eq!(
            snapshot.usage_windows[0].remaining_usd.as_deref(),
            Some("100")
        );
        assert!(
            !snapshot
                .usage_alerts
                .iter()
                .any(|alert| alert.kind == ProviderUsageAlertKind::DailyUsage95)
        );
        assert!(
            !snapshot.routing_exhausted(),
            "sub2api /v1/usage skips billing checks; subscription windows are reset lazily on real requests"
        );
    }

    #[test]
    fn sub2api_subscription_same_day_daily_exhaustion_remains_exhausted() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "unrestricted",
                "planName": "CodeX Lite 年度",
                "remaining": 0,
                "subscription": {
                    "daily_usage_usd": 100.468025,
                    "daily_limit_usd": 100,
                    "weekly_usage_usd": 401.441684,
                    "weekly_limit_usd": 0,
                    "monthly_usage_usd": 401.441684,
                    "monthly_limit_usd": 0
                },
                "usage": {
                    "today": { "cost": 100.468025, "requests": 8, "total_tokens": 1234 },
                    "total": { "cost": 702.492098, "requests": 42, "total_tokens": 1234 }
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("100.468025"));
        assert_eq!(snapshot.today_used_usd.as_deref(), Some("100.468025"));
        assert!(!snapshot.routing_exhausted());
    }

    #[test]
    fn sub2api_quota_limited_zero_remaining_still_marks_exhausted() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "quota_limited",
                "quota": {
                    "limit": 10,
                    "used": 10,
                    "remaining": 0,
                    "unit": "USD"
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.quota_period.as_deref(), Some("quota"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("10"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("10"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("10"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("10"));
        assert!(snapshot.routing_exhausted());
    }

    #[test]
    fn sub2api_quota_limited_rate_limit_exhaustion_marks_temporary_window() {
        let reset_at = "2026-01-02T03:04:05Z";
        let reset_at_ms = parse_timestamp_secs(reset_at).expect("timestamp") * 1000;
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "quota_limited",
                "rate_limits": [
                    {
                        "window": "5h",
                        "limit": 100,
                        "used": 100,
                        "remaining": 0,
                        "reset_at": reset_at
                    }
                ]
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.quota_period.as_deref(), Some("rate_limit:5h"));
        assert_eq!(snapshot.quota_resets_at_ms, Some(reset_at_ms));
        assert_eq!(snapshot.quota_remaining_usd, None);
        assert!(snapshot.routing_exhausted());
    }

    #[test]
    fn sub2api_quota_limited_total_quota_exhaustion_wins_over_rate_limit_window() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": true,
                "mode": "quota_limited",
                "quota": {
                    "limit": 10,
                    "used": 10,
                    "remaining": 0,
                    "unit": "USD"
                },
                "rate_limits": [
                    {
                        "window": "5h",
                        "limit": 100,
                        "used": 100,
                        "remaining": 0,
                        "reset_at": "2026-01-02T03:04:05Z"
                    }
                ]
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.quota_period.as_deref(), Some("quota"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.quota_resets_at_ms, None);
    }

    #[test]
    fn sub2api_usage_snapshot_marks_invalid_key_as_error() {
        let snapshot = sub2api_usage_snapshot_from_json(
            &provider("sub2api", ProviderKind::Sub2ApiUsage),
            &upstream(),
            &serde_json::json!({
                "isValid": false
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Error);
        assert_eq!(
            snapshot.error.as_deref(),
            Some("sub2api usage response reported invalid API key")
        );
    }

    #[test]
    fn sub2api_auth_me_snapshot_reads_dashboard_balance_envelope() {
        let snapshot = sub2api_auth_me_snapshot_from_json(
            &provider("sub2api-auth", ProviderKind::Sub2ApiAuthMe),
            &upstream(),
            &serde_json::json!({
                "code": 0,
                "message": "ok",
                "data": {
                    "id": 42,
                    "username": "demo",
                    "balance": "12.5"
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("12.5"));
    }

    #[test]
    fn rightcode_endpoint_defaults_to_account_summary() {
        let mut provider = provider("rightcode", ProviderKind::RightCodeAccountSummary);
        provider.endpoint.clear();
        provider.domains = vec!["right.codes".to_string()];

        let endpoint = resolve_endpoint(&provider, "https://www.right.codes/codex/v1", "token")
            .expect("endpoint");

        assert_eq!(endpoint, "https://www.right.codes/account/summary");
    }

    #[test]
    fn rightcode_account_summary_reads_matching_subscription_and_balance() {
        let mut provider = provider("rightcode", ProviderKind::RightCodeAccountSummary);
        provider.trust_exhaustion_for_routing = false;

        let snapshot = rightcode_account_summary_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "balance": 3.25,
                "subscriptions": [
                    {
                        "name": "Daily",
                        "total_quota": 20,
                        "remaining_quota": 7.5,
                        "reset_today": true,
                        "available_prefixes": ["/codex"]
                    },
                    {
                        "name": "Other",
                        "total_quota": 99,
                        "remaining_quota": 99,
                        "reset_today": true,
                        "available_prefixes": ["/claude"]
                    },
                    {
                        "name": "Badge",
                        "total_quota": 5,
                        "remaining_quota": 5,
                        "reset_today": true,
                        "available_prefixes": ["/codex"]
                    }
                ]
            }),
            "https://www.right.codes/codex/v1",
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert!(!snapshot.routing_exhausted());
        assert_eq!(snapshot.plan_name.as_deref(), Some("Daily"));
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("3.25"));
        assert_eq!(snapshot.paygo_balance_usd, None);
        assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("7.5"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("20"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("12.5"));
    }

    #[test]
    fn rightcode_account_summary_accounts_for_not_reset_today() {
        let provider = provider("rightcode", ProviderKind::RightCodeAccountSummary);

        let snapshot = rightcode_account_summary_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "subscriptions": [
                    {
                        "name": "Daily",
                        "total_quota": 20,
                        "remaining_quota": 7.5,
                        "reset_today": false,
                        "available_prefixes": ["/codex"]
                    }
                ]
            }),
            "https://www.right.codes/codex/v1",
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("27.5"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("20"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("0"));
    }

    #[test]
    fn rightcode_zero_daily_quota_without_balance_is_display_only_exhaustion_by_default() {
        let mut provider = provider("rightcode", ProviderKind::RightCodeAccountSummary);
        provider.trust_exhaustion_for_routing = false;

        let snapshot = rightcode_account_summary_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "balance": 0,
                "subscriptions": [
                    {
                        "name": "Daily",
                        "total_quota": 20,
                        "remaining_quota": 0,
                        "reset_today": true,
                        "available_prefixes": ["/codex"]
                    }
                ]
            }),
            "https://www.right.codes/codex/v1",
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("0"));
        assert!(!snapshot.routing_exhausted());
        assert!(snapshot.routing_ignored_exhaustion());
    }

    #[test]
    fn sub2api_auth_me_snapshot_marks_business_error() {
        let snapshot = sub2api_auth_me_snapshot_from_json(
            &provider("sub2api-auth", ProviderKind::Sub2ApiAuthMe),
            &upstream(),
            &serde_json::json!({
                "code": 401,
                "message": "login required"
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Error);
        assert_eq!(snapshot.error.as_deref(), Some("login required"));
    }

    #[test]
    fn provider_can_disable_routing_trust_for_exhausted_balance() {
        let mut provider = provider("sub2api", ProviderKind::OpenAiBalanceHttpJson);
        provider.trust_exhaustion_for_routing = false;

        let snapshot = balance_http_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "balance": "0"
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert!(!snapshot.exhaustion_affects_routing);
        assert!(!snapshot.routing_exhausted());
    }

    #[test]
    fn provider_exhaustion_trust_defaults_to_enabled_when_omitted() {
        let provider: UsageProviderConfig = serde_json::from_value(serde_json::json!({
            "id": "sub2api",
            "kind": "openai_balance_http_json",
            "domains": ["example.com"]
        }))
        .expect("provider config");

        assert!(provider.trust_exhaustion_for_routing);
    }

    #[tokio::test]
    async fn provider_missing_token_does_not_recover_committed_eligibility() {
        let target = usage_provider_target_at("backup", 1, "https://backup.example/v1", "right");
        let state = ProxyState::new();
        let endpoint = target.endpoint.provider_endpoint.clone();
        state
            .set_provider_automatic_block_for_runtime_identity_for_test(
                target.runtime_identity(),
                true,
                1_000,
            )
            .await;

        let outcome = refresh_provider_target(RefreshProviderTargetParams {
            client: &Client::new(),
            provider: &provider("sub2api", ProviderKind::OpenAiBalanceHttpJson),
            target: &target,
            state: &state,
            service_name: "codex",
            interval_secs: 60,
            force: false,
        })
        .await;

        assert_eq!(outcome, UsageProviderRefreshOutcome::MissingToken);
        let runtime = state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await;
        assert!(runtime.provider_endpoint(&endpoint).usage_exhausted);
    }

    #[tokio::test]
    async fn authoritative_balance_poll_blocks_and_recovers_durable_policy() {
        let provider = provider("sub2api", ProviderKind::OpenAiBalanceHttpJson);
        let target =
            usage_provider_target_at("default", 1, "https://relay.example.com/v1", "right");
        let state = ProxyState::new();
        let prepared = prepare_provider_poll(
            state.as_ref(),
            &provider,
            &target,
            "model-key",
            provider.kind.source_name(),
            None,
            1_000,
        )
        .await
        .expect("prepare exhausted observation");
        let exhausted = ProviderBalanceSnapshot {
            observation_provider_id: provider.id.clone(),
            provider_endpoint: target.endpoint.provider_endpoint.clone(),
            source: provider.kind.source_name().to_string(),
            fetched_at_ms: 1_000,
            stale_after_ms: Some(60_000),
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: true,
            ..ProviderBalanceSnapshot::default()
        };
        let suppression = ProviderTargetSuppressionDecision {
            reason: "balance exhausted".to_string(),
            ttl: Duration::from_secs(30),
        };

        assert!(
            commit_provider_poll_observation(
                state.as_ref(),
                prepared.reservation,
                &exhausted,
                Some(&suppression),
                1_001,
            )
            .await
        );
        let blocked = state.capture_provider_policy_snapshot().await;
        let projection = blocked
            .projections
            .first()
            .expect("blocked provider projection");
        let action = projection.active_action.as_ref().expect("active action");
        assert_eq!(action.action_kind, "balance_exhausted");
        assert_eq!(action.expires_at_unix_ms, Some(31_001));

        let prepared = prepare_provider_poll(
            state.as_ref(),
            &provider,
            &target,
            "model-key",
            provider.kind.source_name(),
            None,
            2_000,
        )
        .await
        .expect("prepare recovery observation");
        let recovered = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            exhausted: Some(false),
            exhaustion_affects_routing: true,
            fetched_at_ms: 2_000,
            ..exhausted
        };

        assert!(
            commit_provider_poll_observation(
                state.as_ref(),
                prepared.reservation,
                &recovered,
                None,
                2_001,
            )
            .await
        );
        let recovered = state.capture_provider_policy_snapshot().await;
        let projection = recovered
            .projections
            .first()
            .expect("recovered provider projection");
        assert!(projection.active_action.is_none());
        assert_eq!(
            projection.automatic,
            crate::runtime_store::ProviderAutomaticEligibility::Eligible
        );
    }

    #[test]
    fn openai_balance_snapshot_supports_custom_paths_and_derived_budget() {
        let mut provider = provider("custom", ProviderKind::OpenAiBalanceHttpJson);
        provider.extract.remaining_balance_paths = vec!["payload.remaining_usd".to_string()];
        provider.extract.monthly_spent_paths = vec!["payload.used_usd".to_string()];
        provider.extract.derive_budget_from_remaining_and_spent = true;

        let snapshot = balance_http_snapshot_from_json(
            &provider,
            &upstream(),
            &serde_json::json!({
                "payload": {
                    "remaining_usd": "2",
                    "used_usd": "0.5"
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("2"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("2.5"));
        assert_eq!(snapshot.exhausted, Some(false));
    }

    #[test]
    fn new_api_snapshot_converts_quota_units_like_cc_switch_template() {
        let snapshot = new_api_snapshot_from_json(
            &provider("newapi", ProviderKind::NewApiUserSelf),
            &upstream(),
            &serde_json::json!({
                "success": true,
                "data": {
                    "quota": 500000,
                    "used_quota": 250000
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.quota_period.as_deref(), Some("quota"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("1"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("1.5"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("1.5"));
    }

    #[test]
    fn new_api_user_self_honors_unlimited_quota_flag() {
        let snapshot = new_api_snapshot_from_json(
            &provider("newapi", ProviderKind::NewApiUserSelf),
            &upstream(),
            &serde_json::json!({
                "success": true,
                "data": {
                    "quota": 0,
                    "used_quota": 250000,
                    "unlimited_quota": true
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.monthly_budget_usd, None);
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.unlimited_quota, Some(true));
    }

    #[test]
    fn new_api_token_usage_honors_unlimited_quota_flag() {
        let snapshot = new_api_token_usage_snapshot_from_json(
            &provider("newapi-token", ProviderKind::NewApiTokenUsage),
            &upstream(),
            &serde_json::json!({
                "code": true,
                "message": "ok",
                "data": {
                    "object": "token_usage",
                    "name": "demo-token",
                    "total_granted": 0,
                    "total_used": 250000,
                    "total_available": 0,
                    "unlimited_quota": true
                }
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, Some(false));
        assert_eq!(snapshot.plan_name.as_deref(), Some("demo-token"));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.monthly_budget_usd, None);
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.unlimited_quota, Some(true));
    }

    #[test]
    fn openai_organization_costs_sums_official_cost_buckets_without_exhaustion() {
        let snapshot = openai_organization_costs_snapshot_from_json(
            &provider("openai", ProviderKind::OpenAiOrganizationCosts),
            &upstream(),
            &serde_json::json!({
                "object": "page",
                "data": [
                    {
                        "object": "bucket",
                        "start_time": 1710000000,
                        "end_time": 1710086400,
                        "results": [
                            {
                                "object": "organization.costs.result",
                                "amount": { "value": 1.25, "currency": "usd" }
                            },
                            {
                                "object": "organization.costs.result",
                                "amount": { "value": "2.5", "currency": "usd" }
                            },
                            {
                                "object": "organization.costs.result",
                                "amount": { "value": 99, "currency": "eur" }
                            }
                        ]
                    },
                    {
                        "object": "bucket",
                        "results": [
                            {
                                "object": "organization.costs.result",
                                "amount": { "value": "0.25", "currency": "USD" }
                            }
                        ]
                    }
                ],
                "has_more": false
            }),
            100,
            Some(1_000),
        );

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Ok);
        assert_eq!(snapshot.exhausted, None);
        assert!(!snapshot.exhaustion_affects_routing);
        assert!(!snapshot.routing_exhausted());
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("4"));
        assert_eq!(snapshot.total_balance_usd, None);
    }
}
