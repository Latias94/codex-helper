use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::balance::{BalanceSnapshotStatus, ProviderBalanceSnapshot};
use crate::config::{ProxyConfig, ServiceConfigManager, proxy_home_dir};
use crate::lb::LbState;
use crate::pricing::UsdAmount;
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::ProxyState;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
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
            ProviderKind::OpenAiBalanceHttpJson => Some("{{base_url}}/user/balance"),
            ProviderKind::Sub2ApiUsage => Some("{{base_url}}/v1/usage"),
            ProviderKind::Sub2ApiAuthMe => Some("{{base_url}}/api/v1/auth/me"),
            ProviderKind::NewApiTokenUsage => Some("{{base_url}}/api/usage/token/"),
            ProviderKind::NewApiUserSelf => Some("{{base_url}}/api/user/self"),
            ProviderKind::RightCodeAccountSummary => {
                Some("https://www.right.codes/account/summary")
            }
            ProviderKind::OpenAiOrganizationCosts => {
                Some("{{base_url}}/v1/organization/costs?start_time={{unix_days_ago:30}}&limit=30")
            }
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    variables: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "UsageProviderExtractConfig::is_empty")]
    extract: UsageProviderExtractConfig,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct UsageProvidersFile {
    #[serde(default)]
    providers: Vec<UsageProviderConfig>,
}

#[derive(Debug, Clone)]
struct UpstreamRef {
    station_name: String,
    index: usize,
    provider_endpoint: Option<ProviderEndpointKey>,
}

#[derive(Debug, Clone)]
struct UsageProviderTarget {
    upstream: UpstreamRef,
    base_url: String,
    provider_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct UsageProviderTargetKey {
    station_name: String,
    upstream_index: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    cfg: &'a ProxyConfig,
    lb_states: &'a Arc<Mutex<HashMap<String, LbState>>>,
    state: &'a Arc<ProxyState>,
    service_name: &'a str,
    interval_secs: u64,
}

// 全局节流状态：按 provider.id 记录最近一次查询时间，避免高频请求。
static LAST_USAGE_POLL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;
// Minimal poll interval per provider to avoid hammering usage APIs.
const MIN_POLL_INTERVAL_SECS: u64 = 20;
const BALANCE_REFRESH_CONCURRENCY: usize = 6;
const BALANCE_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const AUTO_PROVIDER_ID_PREFIX: &str = "auto:balance:";
const AUTO_PROBE_KINDS: [ProviderKind; 5] = [
    ProviderKind::RightCodeAccountSummary,
    ProviderKind::Sub2ApiUsage,
    ProviderKind::NewApiTokenUsage,
    ProviderKind::NewApiUserSelf,
    ProviderKind::OpenAiBalanceHttpJson,
];

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

fn usage_providers_path() -> std::path::PathBuf {
    proxy_home_dir().join("usage_providers.json")
}

fn service_manager<'a>(cfg: &'a ProxyConfig, service_name: &str) -> &'a ServiceConfigManager {
    match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
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
        poll_interval_secs: Some(60),
        refresh_on_request: true,
        trust_exhaustion_for_routing: true,
        headers: BTreeMap::new(),
        variables: BTreeMap::new(),
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

fn provider_id_component(value: &str) -> String {
    let component = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if component.is_empty() {
        "station".to_string()
    } else {
        component
    }
}

fn auto_provider_id(target: &UsageProviderTarget) -> String {
    if let Some(provider_id) = target
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return provider_id.to_string();
    }
    format!(
        "{}{}:{}",
        AUTO_PROVIDER_ID_PREFIX,
        provider_id_component(&target.upstream.station_name),
        target.upstream.index
    )
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
        poll_interval_secs: Some(DEFAULT_POLL_INTERVAL_SECS),
        refresh_on_request: true,
        trust_exhaustion_for_routing: true,
        headers: BTreeMap::new(),
        variables: BTreeMap::new(),
        extract: UsageProviderExtractConfig::default(),
    };
    if matches!(kind, ProviderKind::RightCodeAccountSummary) {
        provider.trust_exhaustion_for_routing = false;
    }
    provider
}

fn first_auto_probe_kind(target: &UsageProviderTarget) -> ProviderKind {
    if is_rightcode_base_url(&target.base_url) {
        ProviderKind::RightCodeAccountSummary
    } else {
        ProviderKind::Sub2ApiUsage
    }
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
        "https://api.openai.com/v1/organization/costs?start_time={{unix_days_ago:30}}&limit=30",
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
                "{{base_url}}/v1/user/info",
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

fn load_providers() -> UsageProvidersFile {
    let path = usage_providers_path();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(file) = serde_json::from_str::<UsageProvidersFile>(&text)
    {
        return file;
    }

    // 写入默认配置，方便用户查看/修改。
    let default = default_providers();
    if let Ok(text) = serde_json::to_string_pretty(&default) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, text);
    }
    default
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
    cfg: &ProxyConfig,
    service_name: &str,
    provider: &UsageProviderConfig,
    station_name_filter: Option<&str>,
) -> Vec<UsageProviderTarget> {
    let mut stations: Vec<_> = service_manager(cfg, service_name)
        .stations()
        .iter()
        .collect();
    stations.sort_by_key(|(name, _)| name.as_str());

    let mut targets = Vec::new();
    for (station_name, service) in stations {
        if station_name_filter.is_some_and(|filter| filter != station_name.as_str()) {
            continue;
        }
        for (index, upstream) in service.upstreams.iter().enumerate() {
            if domain_matches(&upstream.base_url, &provider.domains) {
                targets.push(UsageProviderTarget {
                    upstream: UpstreamRef {
                        station_name: station_name.clone(),
                        index,
                        provider_endpoint: upstream.provider_endpoint_key(service_name),
                    },
                    base_url: upstream.base_url.clone(),
                    provider_id: upstream.tags.get("provider_id").cloned(),
                });
            }
        }
    }

    targets
}

fn usage_provider_targets(
    cfg: &ProxyConfig,
    service_name: &str,
    station_name_filter: Option<&str>,
) -> Vec<UsageProviderTarget> {
    let mut stations: Vec<_> = service_manager(cfg, service_name)
        .stations()
        .iter()
        .collect();
    stations.sort_by_key(|(name, _)| name.as_str());

    let mut targets = Vec::new();
    for (station_name, service) in stations {
        if station_name_filter.is_some_and(|filter| filter != station_name.as_str()) {
            continue;
        }
        for (index, upstream) in service.upstreams.iter().enumerate() {
            targets.push(UsageProviderTarget {
                upstream: UpstreamRef {
                    station_name: station_name.clone(),
                    index,
                    provider_endpoint: upstream.provider_endpoint_key(service_name),
                },
                base_url: upstream.base_url.clone(),
                provider_id: upstream.tags.get("provider_id").cloned(),
            });
        }
    }

    targets
}

fn target_key(target: &UsageProviderTarget) -> UsageProviderTargetKey {
    UsageProviderTargetKey {
        station_name: target.upstream.station_name.clone(),
        upstream_index: target.upstream.index,
    }
}

fn usage_provider_target_for_legacy_upstream(
    cfg: &ProxyConfig,
    service_name: &str,
    station_name: &str,
    upstream_index: usize,
) -> Option<UsageProviderTarget> {
    let current_service = service_manager(cfg, service_name).station(station_name)?;
    let current_upstream = current_service.upstreams.get(upstream_index)?;
    Some(UsageProviderTarget {
        upstream: UpstreamRef {
            station_name: station_name.to_string(),
            index: upstream_index,
            provider_endpoint: current_upstream.provider_endpoint_key(service_name),
        },
        base_url: current_upstream.base_url.clone(),
        provider_id: current_upstream.tags.get("provider_id").cloned(),
    })
}

fn usage_provider_target_for_provider_endpoint(
    cfg: &ProxyConfig,
    service_name: &str,
    provider_endpoint: &ProviderEndpointKey,
) -> Option<UsageProviderTarget> {
    service_manager(cfg, service_name)
        .stations()
        .iter()
        .filter_map(|(station_name, service)| {
            service
                .upstreams
                .iter()
                .enumerate()
                .find_map(|(index, upstream)| {
                    let upstream_endpoint = upstream.provider_endpoint_key(service_name)?;
                    if upstream_endpoint != *provider_endpoint {
                        return None;
                    }
                    Some(UsageProviderTarget {
                        upstream: UpstreamRef {
                            station_name: station_name.clone(),
                            index,
                            provider_endpoint: Some(upstream_endpoint),
                        },
                        base_url: upstream.base_url.clone(),
                        provider_id: upstream.tags.get("provider_id").cloned(),
                    })
                })
        })
        .next()
}

trait UsageProviderUpstreamIdentityExt {
    fn provider_endpoint_key(&self, service_name: &str) -> Option<ProviderEndpointKey>;
}

impl UsageProviderUpstreamIdentityExt for crate::config::UpstreamConfig {
    fn provider_endpoint_key(&self, service_name: &str) -> Option<ProviderEndpointKey> {
        let provider_id = self.tags.get("provider_id")?.trim();
        let endpoint_id = self.tags.get("endpoint_id")?.trim();
        if provider_id.is_empty() || endpoint_id.is_empty() {
            return None;
        }
        Some(ProviderEndpointKey::new(
            service_name.to_string(),
            provider_id.to_string(),
            endpoint_id.to_string(),
        ))
    }
}

fn configured_target_keys(
    cfg: &ProxyConfig,
    service_name: &str,
    providers: &[UsageProviderConfig],
    station_name_filter: Option<&str>,
) -> HashSet<UsageProviderTargetKey> {
    providers
        .iter()
        .flat_map(|provider| {
            matching_provider_targets(cfg, service_name, provider, station_name_filter)
        })
        .map(|target| target_key(&target))
        .collect()
}

fn resolve_token(
    provider: &UsageProviderConfig,
    upstreams: &[UpstreamRef],
    cfg: &ProxyConfig,
    service_name: &str,
) -> Option<String> {
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

    // 否则: 使用绑定 upstream 的 auth_token（当前 Codex 正在使用的 token）
    for uref in upstreams {
        if let Some(service) = service_manager(cfg, service_name).station(&uref.station_name)
            && let Some(up) = service.upstreams.get(uref.index)
        {
            if let Some(token) = up.auth.resolve_auth_token() {
                return Some(token);
            }
            if let Some(token) = up.auth.resolve_api_key() {
                return Some(token);
            }
        }
    }
    None
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

fn render_provider_template(
    template: &str,
    base_url: &str,
    upstream_base_url: &str,
    token: &str,
    variables: &BTreeMap<String, String>,
) -> String {
    let mut out = template
        .replace("{{baseUrl}}", base_url)
        .replace("{{base_url}}", base_url)
        .replace("{{upstreamBaseUrl}}", upstream_base_url)
        .replace("{{upstream_base_url}}", upstream_base_url)
        .replace("{{apiKey}}", token)
        .replace("{{accessToken}}", token)
        .replace("{{token}}", token);

    out = out
        .replace("{{unix_now}}", &unix_now_secs().to_string())
        .replace("{{unix_now_ms}}", &unix_now_ms().to_string());

    while let Some(start) = out.find("{{unix_days_ago:") {
        let Some(end_offset) = out[start..].find("}}") else {
            break;
        };
        let end = start + end_offset + 2;
        let days_str = out[start + "{{unix_days_ago:".len()..end - 2].trim();
        let replacement = days_str
            .parse::<u64>()
            .ok()
            .map(|days| unix_now_secs().saturating_sub(days.saturating_mul(24 * 60 * 60)))
            .map(|secs| secs.to_string())
            .unwrap_or_default();
        out.replace_range(start..end, &replacement);
    }

    while let Some(start) = out.find("{{env:") {
        let Some(end_offset) = out[start..].find("}}") else {
            break;
        };
        let end = start + end_offset + 2;
        let env_name = out[start + 6..end - 2].trim();
        let value = std::env::var(env_name).unwrap_or_default();
        out.replace_range(start..end, &value);
    }

    for (name, value_template) in variables {
        let value = render_provider_template(
            value_template,
            base_url,
            upstream_base_url,
            token,
            &BTreeMap::new(),
        );
        out = out.replace(&format!("{{{{{name}}}}}"), &value);
    }

    out
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

    let rendered = render_provider_template(
        &endpoint,
        &base_url,
        upstream_base_url,
        token,
        &provider.variables,
    );
    if rendered.starts_with("http://") || rendered.starts_with("https://") {
        return Ok(rendered);
    }

    let path = if rendered.starts_with('/') {
        rendered
    } else {
        format!("/{rendered}")
    };
    Ok(format!("{base_url}{path}"))
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

async fn poll_provider_http_json(
    client: &Client,
    provider: &UsageProviderConfig,
    upstream_base_url: &str,
    token: &str,
) -> Result<serde_json::Value> {
    let endpoint = resolve_endpoint(provider, upstream_base_url, token)?;
    let origin = endpoint_origin(&endpoint);
    let base_url = normalized_balance_base_url(upstream_base_url).unwrap_or_default();
    let mut req = client
        .get(endpoint)
        .timeout(BALANCE_HTTP_REQUEST_TIMEOUT)
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            concat!("codex-helper/", env!("CARGO_PKG_VERSION")),
        );

    match provider.kind {
        ProviderKind::YescodeProfile => {
            req = req.header("X-API-Key", token);
        }
        _ => {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
    }

    for (name, template) in &provider.headers {
        let value = render_provider_template(
            template,
            &base_url,
            upstream_base_url,
            token,
            &provider.variables,
        );
        if !value.trim().is_empty() {
            req = req.header(name.as_str(), value);
        }
    }

    let resp = req.send().await.with_context(|| {
        format!(
            "usage provider request failed for {} via {:?}",
            origin, provider.kind
        )
    })?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "usage provider HTTP {} from {} via {:?}",
            resp.status(),
            origin,
            provider.kind
        );
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
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

fn amount_from_json(value: &serde_json::Value) -> Option<UsdAmount> {
    let raw = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_string(),
        _ => return None,
    };
    UsdAmount::from_decimal_str(raw.as_str())
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

fn base_snapshot(
    provider: &UsageProviderConfig,
    upstream: &UpstreamRef,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    let mut snapshot = ProviderBalanceSnapshot::new(
        provider.id.clone(),
        upstream.station_name.clone(),
        upstream.index,
        provider.kind.source_name(),
        fetched_at_ms,
        stale_after_ms,
    );
    snapshot.exhaustion_affects_routing = provider.trust_exhaustion_for_routing;
    snapshot
}

fn snapshot_error(
    provider: &UsageProviderConfig,
    upstream: &UpstreamRef,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
    message: impl Into<String>,
) -> ProviderBalanceSnapshot {
    base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms).with_error(message)
}

fn budget_snapshot_from_json(
    provider: &UsageProviderConfig,
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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
    snapshot.plan_name = first_string_from_paths(value, &["planName", "data.planName"]);
    snapshot.total_used_usd = first_amount_from_paths(
        value,
        &[],
        &["usage.total.cost", "data.usage.total.cost"],
        None,
    )
    .map(amount_to_string);
    snapshot.today_used_usd = first_amount_from_paths(
        value,
        &[],
        &["usage.today.cost", "data.usage.today.cost"],
        None,
    )
    .map(amount_to_string);
    snapshot.total_requests = first_u64_from_paths(
        value,
        &["usage.total.requests", "data.usage.total.requests"],
    );
    snapshot.today_requests = first_u64_from_paths(
        value,
        &["usage.today.requests", "data.usage.today.requests"],
    );
    snapshot.total_tokens = first_u64_from_paths(
        value,
        &[
            "usage.total.total_tokens",
            "usage.total.tokens",
            "data.usage.total.total_tokens",
            "data.usage.total.tokens",
        ],
    );
    snapshot.today_tokens = first_u64_from_paths(
        value,
        &[
            "usage.today.total_tokens",
            "usage.today.tokens",
            "data.usage.today.total_tokens",
            "data.usage.today.tokens",
        ],
    );
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
    let spent = first_amount_from_paths(value, &[], usage_paths, None).unwrap_or(UsdAmount::ZERO);
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
    upstream: &UpstreamRef,
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
        .or_else(|| quota_remaining.map(UsdAmount::is_zero));

        let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
        snapshot.quota_period = Some("quota".to_string());
        snapshot.quota_remaining_usd = quota_remaining.map(amount_to_string);
        snapshot.quota_limit_usd = quota_limit.map(amount_to_string);
        snapshot.quota_used_usd = quota_used.map(amount_to_string);
        snapshot.monthly_budget_usd = quota_limit.map(amount_to_string);
        snapshot.monthly_spent_usd = quota_used.map(amount_to_string);
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
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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
    upstream: &UpstreamRef,
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

async fn update_usage_exhausted(
    lb_states: &Arc<Mutex<HashMap<String, LbState>>>,
    state: &Arc<ProxyState>,
    cfg: &ProxyConfig,
    service_name: &str,
    upstreams: &[UpstreamRef],
    exhausted: bool,
) {
    if let Ok(mut map) = lb_states.lock() {
        for uref in upstreams {
            let service = match service_manager(cfg, service_name).station(&uref.station_name) {
                Some(s) => s,
                None => continue,
            };

            let entry = map
                .entry(uref.station_name.clone())
                .or_insert_with(LbState::default);
            entry.ensure_layout(service.name.as_str(), &service.upstreams);
            if uref.index < entry.usage_exhausted.len() {
                entry.usage_exhausted[uref.index] = exhausted;
            }
        }
    }

    for uref in upstreams {
        if let Some(endpoint_key) = uref.provider_endpoint.clone() {
            state
                .set_provider_endpoint_usage_exhausted(service_name, endpoint_key, exhausted)
                .await;
        }
    }
}

fn provider_hosts_for_diagnostics(
    cfg: &ProxyConfig,
    service_name: &str,
    provider: &UsageProviderConfig,
) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for service in service_manager(cfg, service_name).stations().values() {
        for upstream in &service.upstreams {
            if domain_matches(&upstream.base_url, &provider.domains)
                && let Ok(url) = reqwest::Url::parse(&upstream.base_url)
                && let Some(host) = url.host_str()
            {
                hosts.push(host.to_string());
            }
        }
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

fn warn_if_provider_spans_hosts(
    cfg: &ProxyConfig,
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
    upstream: &UpstreamRef,
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
        cfg,
        lb_states,
        state,
        service_name,
        interval_secs,
    } = params;

    let upstreams = vec![target.upstream.clone()];
    let fetched_at_ms = unix_now_ms();
    let stale_after_ms = stale_after_ms(fetched_at_ms, interval_secs);

    let Some(token) = resolve_token(provider, &upstreams, cfg, service_name) else {
        let snapshot = if provider.kind == ProviderKind::OpenAiOrganizationCosts {
            base_snapshot(provider, &upstreams[0], fetched_at_ms, stale_after_ms)
        } else {
            base_snapshot(provider, &upstreams[0], fetched_at_ms, stale_after_ms)
                .with_error("no usable token; checked provider token_env and upstream auth")
        };
        state
            .record_provider_balance_snapshot(service_name, snapshot)
            .await;
        update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false).await;
        if provider.kind == ProviderKind::OpenAiOrganizationCosts {
            warn!(
                "usage provider '{}' is missing OPENAI_ADMIN_KEY; OpenAI official costs stay unknown",
                provider.id
            );
        } else {
            warn!(
                "usage provider '{}' has no usable token (checked token_env and associated upstream auth_token); \
跳过本次用量查询，请检查 usage_providers.json 和 ~/.codex-helper/config.json",
                provider.id
            );
        }
        return UsageProviderRefreshOutcome::MissingToken;
    };

    match poll_provider_http_json(client, provider, &target.base_url, &token).await {
        Ok(value) => {
            let snapshot = snapshot_from_provider_json(
                provider,
                &upstreams[0],
                &value,
                &target.base_url,
                fetched_at_ms,
                stale_after_ms,
            );
            let exhausted_for_lb = snapshot.routing_exhausted();
            update_usage_exhausted(
                lb_states,
                state,
                cfg,
                service_name,
                &upstreams,
                exhausted_for_lb,
            )
            .await;
            state
                .record_provider_balance_snapshot(service_name, snapshot)
                .await;
            info!(
                "usage provider '{}' refreshed {}[{}], exhausted = {}, routing_trusted = {}",
                provider.id,
                target.upstream.station_name,
                target.upstream.index,
                exhausted_for_lb,
                provider.trust_exhaustion_for_routing
            );
            UsageProviderRefreshOutcome::Refreshed
        }
        Err(err) => {
            state
                .record_provider_balance_snapshot(
                    service_name,
                    base_snapshot(provider, &upstreams[0], fetched_at_ms, stale_after_ms)
                        .with_error(err.to_string()),
                )
                .await;
            update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false).await;
            warn!(
                "usage provider '{}' poll failed for {}[{}]: {}",
                provider.id, target.upstream.station_name, target.upstream.index, err
            );
            UsageProviderRefreshOutcome::Failed
        }
    }
}

struct ConfiguredRefreshJob<'a> {
    provider: &'a UsageProviderConfig,
    target: UsageProviderTarget,
    interval_secs: u64,
}

struct AutoRefreshJob {
    target: UsageProviderTarget,
}

async fn run_configured_refresh_job<'a>(
    client: &'a Client,
    job: ConfiguredRefreshJob<'a>,
    cfg: &'a ProxyConfig,
    lb_states: &'a Arc<Mutex<HashMap<String, LbState>>>,
    state: &'a Arc<ProxyState>,
    service_name: &'a str,
) -> (String, UsageProviderRefreshOutcome) {
    let provider_id = job.provider.id.clone();
    let outcome = refresh_provider_target(RefreshProviderTargetParams {
        client,
        provider: job.provider,
        target: &job.target,
        cfg,
        lb_states,
        state,
        service_name,
        interval_secs: job.interval_secs,
    })
    .await;
    (provider_id, outcome)
}

async fn run_auto_refresh_job(
    client: &Client,
    job: AutoRefreshJob,
    cfg: &ProxyConfig,
    lb_states: &Arc<Mutex<HashMap<String, LbState>>>,
    state: &Arc<ProxyState>,
    service_name: &str,
) -> UsageProviderRefreshOutcome {
    auto_probe_provider_target(client, &job.target, cfg, lb_states, state, service_name).await
}

async fn run_configured_refresh_jobs<'a>(
    client: &'a Client,
    jobs: Vec<ConfiguredRefreshJob<'a>>,
    cfg: &'a ProxyConfig,
    lb_states: &'a Arc<Mutex<HashMap<String, LbState>>>,
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
        running.push(run_configured_refresh_job(
            client,
            job,
            cfg,
            lb_states,
            state,
            service_name,
        ));
    }

    while let Some(result) = running.next().await {
        results.push(result);
        if let Some(job) = pending.next() {
            running.push(run_configured_refresh_job(
                client,
                job,
                cfg,
                lb_states,
                state,
                service_name,
            ));
        }
    }

    results
}

async fn run_auto_refresh_jobs(
    client: &Client,
    jobs: Vec<AutoRefreshJob>,
    cfg: &ProxyConfig,
    lb_states: &Arc<Mutex<HashMap<String, LbState>>>,
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
        running.push(run_auto_refresh_job(
            client,
            job,
            cfg,
            lb_states,
            state,
            service_name,
        ));
    }

    while let Some(result) = running.next().await {
        results.push(result);
        if let Some(job) = pending.next() {
            running.push(run_auto_refresh_job(
                client,
                job,
                cfg,
                lb_states,
                state,
                service_name,
            ));
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

async fn auto_probe_provider_target(
    client: &Client,
    target: &UsageProviderTarget,
    cfg: &ProxyConfig,
    lb_states: &Arc<Mutex<HashMap<String, LbState>>>,
    state: &Arc<ProxyState>,
    service_name: &str,
) -> UsageProviderRefreshOutcome {
    let upstreams = vec![target.upstream.clone()];
    let fetched_at_ms = unix_now_ms();
    let interval_secs = DEFAULT_POLL_INTERVAL_SECS;
    let stale_after_ms = stale_after_ms(fetched_at_ms, interval_secs);

    if is_official_openai_base_url(&target.base_url) {
        let provider = auto_openai_official_provider(target);
        let Some(token) = resolve_token(&provider, &upstreams, cfg, service_name) else {
            state
                .record_provider_balance_snapshot(
                    service_name,
                    base_snapshot(&provider, &target.upstream, fetched_at_ms, stale_after_ms),
                )
                .await;
            update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false).await;
            warn!(
                "OpenAI organization costs require OPENAI_ADMIN_KEY; balance stays unknown for {}[{}]",
                target.upstream.station_name, target.upstream.index
            );
            return UsageProviderRefreshOutcome::MissingToken;
        };

        return match poll_provider_http_json(client, &provider, &target.base_url, &token).await {
            Ok(value) => {
                let snapshot = snapshot_from_provider_json(
                    &provider,
                    &upstreams[0],
                    &value,
                    &target.base_url,
                    fetched_at_ms,
                    stale_after_ms,
                );
                update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false)
                    .await;
                state
                    .record_provider_balance_snapshot(service_name, snapshot)
                    .await;
                UsageProviderRefreshOutcome::Refreshed
            }
            Err(err) => {
                state
                    .record_provider_balance_snapshot(
                        service_name,
                        base_snapshot(&provider, &upstreams[0], fetched_at_ms, stale_after_ms)
                            .with_error(err.to_string()),
                    )
                    .await;
                update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false)
                    .await;
                warn!(
                    "OpenAI organization costs poll failed for {}[{}]: {}",
                    target.upstream.station_name, target.upstream.index, err
                );
                UsageProviderRefreshOutcome::Failed
            }
        };
    }

    let first_provider = auto_usage_provider(target, first_auto_probe_kind(target));

    let Some(token) = resolve_token(&first_provider, &upstreams, cfg, service_name) else {
        state
            .record_provider_balance_snapshot(
                service_name,
                base_snapshot(
                    &first_provider,
                    &target.upstream,
                    fetched_at_ms,
                    stale_after_ms,
                )
                .with_error("no usable token; checked upstream auth"),
            )
            .await;
        update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false).await;
        return UsageProviderRefreshOutcome::MissingToken;
    };

    let mut last_error: Option<String> = None;
    for kind in AUTO_PROBE_KINDS {
        if kind == ProviderKind::RightCodeAccountSummary && !is_rightcode_base_url(&target.base_url)
        {
            continue;
        }
        let provider = auto_usage_provider(target, kind);
        match poll_provider_http_json(client, &provider, &target.base_url, &token).await {
            Ok(value) => {
                let snapshot = snapshot_from_provider_json(
                    &provider,
                    &upstreams[0],
                    &value,
                    &target.base_url,
                    fetched_at_ms,
                    stale_after_ms,
                );
                if auto_snapshot_is_usable(&snapshot) {
                    let exhausted_for_lb = snapshot.routing_exhausted();
                    update_usage_exhausted(
                        lb_states,
                        state,
                        cfg,
                        service_name,
                        &upstreams,
                        exhausted_for_lb,
                    )
                    .await;
                    state
                        .record_provider_balance_snapshot(service_name, snapshot)
                        .await;
                    info!(
                        "auto usage provider '{}' refreshed {}[{}] via {:?}, exhausted = {}",
                        provider.id,
                        target.upstream.station_name,
                        target.upstream.index,
                        kind,
                        exhausted_for_lb
                    );
                    return UsageProviderRefreshOutcome::Refreshed;
                }
                last_error = snapshot.error.or_else(|| {
                    Some(format!(
                        "auto probe {:?} returned no usable balance fields",
                        kind
                    ))
                });
            }
            Err(err) => {
                last_error = Some(err.to_string());
            }
        }
    }

    if let Some(error) = last_error {
        warn!(
            "auto usage provider '{}' found no usable balance endpoint for {}[{}]: {}",
            first_provider.id, target.upstream.station_name, target.upstream.index, error
        );
        state
            .record_provider_balance_snapshot(
                service_name,
                base_snapshot(
                    &first_provider,
                    &target.upstream,
                    fetched_at_ms,
                    stale_after_ms,
                )
                .with_error(error),
            )
            .await;
        update_usage_exhausted(lb_states, state, cfg, service_name, &upstreams, false).await;
    }
    UsageProviderRefreshOutcome::Failed
}

pub async fn refresh_balances_for_service(
    client: &Client,
    cfg: Arc<ProxyConfig>,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    state: Arc<ProxyState>,
    service_name: &str,
    station_name_filter: Option<&str>,
    provider_id_filter: Option<&str>,
) -> UsageProviderRefreshSummary {
    // Tests should be hermetic and must not depend on real user `usage_providers.json`.
    if cfg!(test) {
        return UsageProviderRefreshSummary::default();
    }

    let station_name_filter = station_name_filter
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let provider_id_filter = provider_id_filter
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let providers_file = load_providers();
    let mut summary = UsageProviderRefreshSummary {
        providers_configured: providers_file.providers.len(),
        ..UsageProviderRefreshSummary::default()
    };
    let configured_keys = if provider_id_filter.is_none() {
        configured_target_keys(
            &cfg,
            service_name,
            &providers_file.providers,
            station_name_filter,
        )
    } else {
        HashSet::new()
    };

    let poll_map = LAST_USAGE_POLL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut configured_jobs = Vec::new();
    for provider in &providers_file.providers {
        if provider_id_filter.is_some_and(|filter| filter != provider.id.as_str()) {
            continue;
        }

        let targets = matching_provider_targets(&cfg, service_name, provider, station_name_filter);
        if targets.is_empty() {
            continue;
        }

        summary.providers_matched += 1;
        summary.upstreams_matched += targets.len();
        warn_if_provider_spans_hosts(&cfg, service_name, provider);

        let interval_secs = snapshot_refresh_interval_secs(provider);
        for target in targets {
            summary.attempted += 1;
            configured_jobs.push(ConfiguredRefreshJob {
                provider,
                target,
                interval_secs,
            });
        }
    }

    let mut refreshed_provider_ids = HashSet::new();
    if !configured_jobs.is_empty() {
        for (provider_id, outcome) in run_configured_refresh_jobs(
            client,
            configured_jobs,
            &cfg,
            &lb_states,
            &state,
            service_name,
        )
        .await
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

    if provider_id_filter.is_none() {
        let mut auto_jobs = Vec::new();
        for target in usage_provider_targets(&cfg, service_name, station_name_filter) {
            if configured_keys.contains(&target_key(&target)) {
                continue;
            }

            summary.attempted += 1;
            summary.auto_attempted += 1;
            auto_jobs.push(AutoRefreshJob { target });
        }

        if !auto_jobs.is_empty() {
            for outcome in
                run_auto_refresh_jobs(client, auto_jobs, &cfg, &lb_states, &state, service_name)
                    .await
            {
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
    }

    if !refreshed_provider_ids.is_empty()
        && let Ok(mut map) = poll_map.lock()
    {
        let now = Instant::now();
        for provider_id in refreshed_provider_ids {
            map.insert(provider_id, now);
        }
    }

    summary
}

/// 在特定 Codex upstream 请求结束后，按需查询一次用量并更新 LB 状态。
/// 设计为轻量的“按需刷新”，而非后台定时轮询。
pub async fn poll_for_codex_upstream(
    client: Client,
    cfg: Arc<ProxyConfig>,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    state: Arc<ProxyState>,
    service_name: &str,
    station_name: &str,
    upstream_index: usize,
) {
    let current_target =
        usage_provider_target_for_legacy_upstream(&cfg, service_name, station_name, upstream_index);
    poll_for_codex_target(client, cfg, lb_states, state, service_name, current_target).await;
}

/// Provider-endpoint keyed variant used by the route graph executor.
/// Station/upstream are still updated inside the usage provider as a compatibility projection
/// when the current runtime config can map the endpoint back to one legacy upstream.
pub async fn poll_for_codex_provider_endpoint(
    client: Client,
    cfg: Arc<ProxyConfig>,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    state: Arc<ProxyState>,
    service_name: &str,
    provider_endpoint: ProviderEndpointKey,
) {
    let current_target =
        usage_provider_target_for_provider_endpoint(&cfg, service_name, &provider_endpoint);
    poll_for_codex_target(client, cfg, lb_states, state, service_name, current_target).await;
}

async fn poll_for_codex_target(
    client: Client,
    cfg: Arc<ProxyConfig>,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    state: Arc<ProxyState>,
    service_name: &str,
    current_target: Option<UsageProviderTarget>,
) {
    // Tests should be hermetic and should not depend on any real user `usage_providers.json` on
    // the machine running the suite. Disable provider polling during tests to avoid flakiness.
    if cfg!(test) {
        return;
    }

    let providers_file = load_providers();
    let Some(current_target) = current_target else {
        return;
    };

    let now = Instant::now();
    let poll_map = LAST_USAGE_POLL.get_or_init(|| Mutex::new(HashMap::new()));
    let mut matched_configured_provider = false;
    let mut configured_jobs = Vec::new();

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
                && now.duration_since(*last) < Duration::from_secs(interval_secs)
            {
                continue;
            }
            map.insert(provider.id.clone(), now);
        }

        warn_if_provider_spans_hosts(&cfg, service_name, provider);
        configured_jobs.push(ConfiguredRefreshJob {
            provider,
            target: current_target.clone(),
            interval_secs,
        });
    }

    if !configured_jobs.is_empty() {
        let _ = run_configured_refresh_jobs(
            &client,
            configured_jobs,
            &cfg,
            &lb_states,
            &state,
            service_name,
        )
        .await;
    }

    if matched_configured_provider {
        return;
    }

    let auto_provider = if is_official_openai_base_url(&current_target.base_url) {
        auto_openai_official_provider(&current_target)
    } else {
        auto_usage_provider(&current_target, first_auto_probe_kind(&current_target))
    };
    let Some(interval_secs) = effective_poll_interval_secs(&auto_provider) else {
        return;
    };

    {
        let mut map = match poll_map.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        if let Some(last) = map.get(&auto_provider.id)
            && now.duration_since(*last) < Duration::from_secs(interval_secs)
        {
            return;
        }
        map.insert(auto_provider.id.clone(), now);
    }

    let _ = auto_probe_provider_target(
        &client,
        &current_target,
        &cfg,
        &lb_states,
        &state,
        service_name,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::balance::BalanceSnapshotStatus;
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};

    fn provider(id: &str, kind: ProviderKind) -> UsageProviderConfig {
        UsageProviderConfig {
            id: id.to_string(),
            kind,
            domains: vec!["example.com".to_string()],
            endpoint: "https://example.com/usage".to_string(),
            token_env: None,
            require_token_env: false,
            poll_interval_secs: Some(60),
            refresh_on_request: true,
            trust_exhaustion_for_routing: true,
            headers: BTreeMap::new(),
            variables: BTreeMap::new(),
            extract: UsageProviderExtractConfig::default(),
        }
    }

    fn upstream() -> UpstreamRef {
        UpstreamRef {
            station_name: "right".to_string(),
            index: 1,
            provider_endpoint: None,
        }
    }

    fn upstream_config(base_url: &str) -> UpstreamConfig {
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn endpoint_upstream_config(
        base_url: &str,
        provider_id: &str,
        endpoint_id: &str,
    ) -> UpstreamConfig {
        let mut upstream = upstream_config(base_url);
        upstream
            .tags
            .insert("provider_id".to_string(), provider_id.to_string());
        upstream
            .tags
            .insert("endpoint_id".to_string(), endpoint_id.to_string());
        upstream
    }

    fn service_config(name: &str, upstreams: Vec<UpstreamConfig>) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams,
        }
    }

    fn proxy_config(stations: Vec<ServiceConfig>) -> ProxyConfig {
        let mut cfg = ProxyConfig::default();
        cfg.codex.configs = stations
            .into_iter()
            .map(|station| (station.name.clone(), station))
            .collect();
        cfg
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
    fn provider_templates_support_variables_for_custom_headers_or_queries() {
        let mut provider = provider("newapi", ProviderKind::NewApiUserSelf);
        provider.endpoint = "{{base_url}}/api/user/self?user={{userId}}".to_string();
        provider
            .variables
            .insert("userId".to_string(), "42".to_string());

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
    fn require_token_env_prevents_upstream_model_key_fallback() {
        let mut cfg = proxy_config(vec![service_config(
            "right",
            vec![upstream_config("https://api.openai.com/v1")],
        )]);
        cfg.codex
            .configs
            .get_mut("right")
            .expect("station")
            .upstreams[0]
            .auth
            .auth_token = Some("model-key".to_string());

        let mut provider = provider("openai", ProviderKind::OpenAiOrganizationCosts);
        provider.token_env = Some("__CODEX_HELPER_TEST_MISSING_TOKEN_ENV__".to_string());
        provider.require_token_env = true;
        let upstreams = [UpstreamRef {
            station_name: "right".to_string(),
            index: 0,
            provider_endpoint: None,
        }];

        assert_eq!(resolve_token(&provider, &upstreams, &cfg, "codex"), None);

        provider.require_token_env = false;
        assert_eq!(
            resolve_token(&provider, &upstreams, &cfg, "codex").as_deref(),
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
    fn auto_provider_uses_stable_target_id_across_probe_kinds() {
        let target = UsageProviderTarget {
            upstream: UpstreamRef {
                station_name: "input/sub".to_string(),
                index: 2,
                provider_endpoint: None,
            },
            base_url: "https://ai.input.im/v1".to_string(),
            provider_id: None,
        };

        let sub2api = auto_usage_provider(&target, ProviderKind::Sub2ApiUsage);
        let newapi_token = auto_usage_provider(&target, ProviderKind::NewApiTokenUsage);
        let newapi = auto_usage_provider(&target, ProviderKind::NewApiUserSelf);

        assert_eq!(sub2api.id, "auto:balance:input-sub:2");
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
        let target = UsageProviderTarget {
            upstream: UpstreamRef {
                station_name: "right".to_string(),
                index: 0,
                provider_endpoint: None,
            },
            base_url: "https://www.right.codes/codex/v1".to_string(),
            provider_id: Some("right".to_string()),
        };

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
    fn auto_provider_id_prefers_runtime_provider_tag() {
        let target = UsageProviderTarget {
            upstream: UpstreamRef {
                station_name: "routing".to_string(),
                index: 0,
                provider_endpoint: Some(ProviderEndpointKey::new("codex", "input", "default")),
            },
            base_url: "https://ai.input.im/v1".to_string(),
            provider_id: Some("input".to_string()),
        };

        let provider = auto_usage_provider(&target, ProviderKind::Sub2ApiUsage);

        assert_eq!(provider.id, "input");
    }

    #[test]
    fn provider_endpoint_target_lookup_uses_endpoint_identity() {
        let cfg = proxy_config(vec![service_config(
            "routing",
            vec![
                endpoint_upstream_config("https://input.example/v1", "input", "default"),
                endpoint_upstream_config("https://right.example/v1", "right", "default"),
            ],
        )]);

        let target = usage_provider_target_for_provider_endpoint(
            &cfg,
            "codex",
            &ProviderEndpointKey::new("codex", "right", "default"),
        )
        .expect("provider endpoint target");

        assert_eq!(target.upstream.station_name, "routing");
        assert_eq!(target.upstream.index, 1);
        assert_eq!(
            target
                .upstream
                .provider_endpoint
                .as_ref()
                .map(ProviderEndpointKey::stable_key)
                .as_deref(),
            Some("codex/right/default")
        );
        assert_eq!(target.base_url, "https://right.example/v1");
        assert_eq!(target.provider_id.as_deref(), Some("right"));
    }

    #[test]
    fn configured_target_keys_prevent_auto_probe_for_explicit_balance_domains() {
        let cfg = proxy_config(vec![
            service_config("explicit", vec![upstream_config("https://example.com/v1")]),
            service_config("auto", vec![upstream_config("https://ai.input.im/v1")]),
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
            .map(|target| target.upstream.station_name)
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
    fn sub2api_subscription_zero_remaining_is_display_only_period_capacity_exhaustion() {
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

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert_eq!(snapshot.exhausted, Some(true));
        assert_eq!(snapshot.plan_name.as_deref(), Some("CodeX Lite 年度"));
        assert_eq!(snapshot.total_balance_usd, None);
        assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
        assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("0"));
        assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("100"));
        assert_eq!(snapshot.quota_used_usd.as_deref(), Some("100.468025"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("100"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("100.468025"));
        assert_eq!(snapshot.total_used_usd.as_deref(), Some("702.492098"));
        assert_eq!(snapshot.today_used_usd.as_deref(), Some("0"));
        assert!(
            !snapshot.routing_exhausted(),
            "sub2api /v1/usage skips billing checks; subscription windows are reset lazily on real requests"
        );
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
    async fn provider_missing_token_clears_stale_lb_exhaustion_marker() {
        let cfg = proxy_config(vec![service_config(
            "right",
            vec![
                upstream_config("https://primary.example/v1"),
                upstream_config("https://backup.example/v1"),
            ],
        )]);
        let lb_states = Arc::new(Mutex::new(HashMap::new()));
        let target = UsageProviderTarget {
            upstream: upstream(),
            base_url: "https://backup.example/v1".to_string(),
            provider_id: Some("right".to_string()),
        };
        let upstreams = vec![target.upstream.clone()];
        let state = ProxyState::new();
        update_usage_exhausted(&lb_states, &state, &cfg, "codex", &upstreams, true).await;
        {
            let guard = lb_states.lock().expect("lb states");
            assert!(
                guard
                    .get("right")
                    .and_then(|entry| entry.usage_exhausted.get(1))
                    .copied()
                    .unwrap_or(false)
            );
        }

        let outcome = refresh_provider_target(RefreshProviderTargetParams {
            client: &Client::new(),
            provider: &provider("sub2api", ProviderKind::OpenAiBalanceHttpJson),
            target: &target,
            cfg: &cfg,
            lb_states: &lb_states,
            state: &state,
            service_name: "codex",
            interval_secs: 60,
        })
        .await;

        assert_eq!(outcome, UsageProviderRefreshOutcome::MissingToken);
        let guard = lb_states.lock().expect("lb states");
        assert!(
            !guard
                .get("right")
                .and_then(|entry| entry.usage_exhausted.get(1))
                .copied()
                .unwrap_or(true)
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
