use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::balance::ProviderBalanceSnapshot;
use crate::config::{ProxyConfig, proxy_home_dir};
use crate::lb::LbState;
use crate::pricing::UsdAmount;
use crate::state::ProxyState;

#[derive(Debug, Deserialize, Serialize)]
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
        alias = "sub2api_balance_http_json",
        alias = "relay_balance_http_json"
    )]
    OpenAiBalanceHttpJson,
    /// New API-style user quota endpoint, defaulting to /api/user/self.
    NewApiUserSelf,
}

impl ProviderKind {
    fn source_name(&self) -> &'static str {
        match self {
            ProviderKind::BudgetHttpJson => "usage_provider:budget_http_json",
            ProviderKind::YescodeProfile => "usage_provider:yescode_profile",
            ProviderKind::OpenAiBalanceHttpJson => "usage_provider:openai_balance_http_json",
            ProviderKind::NewApiUserSelf => "usage_provider:new_api_user_self",
        }
    }

    fn default_endpoint(&self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAiBalanceHttpJson => Some("{{base_url}}/user/balance"),
            ProviderKind::NewApiUserSelf => Some("{{base_url}}/api/user/self"),
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
    #[serde(default)]
    poll_interval_secs: Option<u64>,
    #[serde(
        default = "default_refresh_on_request",
        skip_serializing_if = "bool_is_true"
    )]
    refresh_on_request: bool,
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
}

// 全局节流状态：按 provider.id 记录最近一次查询时间，避免高频请求。
static LAST_USAGE_POLL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;
// Minimal poll interval per provider to avoid hammering usage APIs.
const MIN_POLL_INTERVAL_SECS: u64 = 20;

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn bool_is_true(value: &bool) -> bool {
    *value
}

fn default_refresh_on_request() -> bool {
    true
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn stale_after_ms(fetched_at_ms: u64, interval_secs: u64) -> Option<u64> {
    fetched_at_ms.checked_add(interval_secs.saturating_mul(3).saturating_mul(1_000))
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

fn default_providers() -> UsageProvidersFile {
    UsageProvidersFile {
        providers: vec![
            UsageProviderConfig {
                id: "packycode".to_string(),
                kind: ProviderKind::BudgetHttpJson,
                domains: vec!["packycode.com".to_string()],
                endpoint: "https://www.packycode.com/api/backend/users/info".to_string(),
                token_env: None,
                poll_interval_secs: Some(60),
                refresh_on_request: true,
                headers: BTreeMap::new(),
                variables: BTreeMap::new(),
                extract: UsageProviderExtractConfig::default(),
            },
            UsageProviderConfig {
                id: "yescode".to_string(),
                kind: ProviderKind::YescodeProfile,
                // yes.vg 匹配 co.yes.vg / cotest.yes.vg 等子域名
                domains: vec!["yes.vg".to_string()],
                endpoint: "https://co.yes.vg/api/v1/auth/profile".to_string(),
                token_env: None,
                poll_interval_secs: Some(60),
                refresh_on_request: true,
                headers: BTreeMap::new(),
                variables: BTreeMap::new(),
                extract: UsageProviderExtractConfig::default(),
            },
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
    for d in domains {
        if host == d || host.ends_with(&format!(".{}", d)) {
            return true;
        }
    }
    false
}

fn resolve_token(
    provider: &UsageProviderConfig,
    upstreams: &[UpstreamRef],
    cfg: &ProxyConfig,
) -> Option<String> {
    // 优先: token_env 环境变量
    if let Some(env_name) = &provider.token_env
        && let Ok(v) = std::env::var(env_name)
        && !v.trim().is_empty()
    {
        return Some(v);
    }

    // 否则: 使用绑定 upstream 的 auth_token（当前 Codex 正在使用的 token）
    for uref in upstreams {
        if let Some(service) = cfg.codex.station(&uref.station_name)
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

async fn poll_provider_http_json(
    client: &Client,
    provider: &UsageProviderConfig,
    upstream_base_url: &str,
    token: &str,
) -> Result<serde_json::Value> {
    let endpoint = resolve_endpoint(provider, upstream_base_url, token)?;
    let base_url = normalized_balance_base_url(upstream_base_url).unwrap_or_default();
    let mut req = client
        .get(endpoint)
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

    let resp = req.send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("usage provider HTTP {}", resp.status());
    }
    Ok(resp.json().await?)
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
        current = current.get(segment)?;
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

fn amount_to_string(amount: UsdAmount) -> String {
    amount.format_usd()
}

fn base_snapshot(
    provider: &UsageProviderConfig,
    upstream: &UpstreamRef,
    fetched_at_ms: u64,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    ProviderBalanceSnapshot::new(
        provider.id.clone(),
        upstream.station_name.clone(),
        upstream.index,
        provider.kind.source_name(),
        fetched_at_ms,
        stale_after_ms,
    )
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
            "data.balance",
            "data.remaining",
            "data.available",
            "data.available_balance",
            "data.credit",
            "data.credits",
            "data.total_balance",
        ],
        provider.extract.remaining_divisor,
    );
    let subscription_balance = first_amount_from_paths(
        value,
        &provider.extract.subscription_balance_paths,
        &[
            "subscription_balance",
            "subscription_balance_usd",
            "data.subscription_balance",
            "data.subscription_balance_usd",
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
            "data.pay_as_you_go_balance",
            "data.paygo_balance",
            "data.paygo",
        ],
        provider.extract.remaining_divisor,
    );
    let derived_remaining = match (subscription_balance, paygo_balance) {
        (Some(subscription), Some(paygo)) => Some(subscription.saturating_add(paygo)),
        (Some(subscription), None) => Some(subscription),
        (None, Some(paygo)) => Some(paygo),
        (None, None) => None,
    };
    let total_balance = remaining_balance.or(derived_remaining);
    let monthly_spent = first_amount_from_paths(
        value,
        &provider.extract.monthly_spent_paths,
        &[
            "monthly_spent_usd",
            "spent",
            "used",
            "used_balance",
            "data.monthly_spent_usd",
            "data.spent",
            "data.used",
            "data.used_balance",
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
            "data.monthly_budget_usd",
            "data.budget",
            "data.limit",
            "data.quota_total",
        ],
        provider.extract.monthly_budget_divisor,
    )
    .or_else(|| {
        if provider.extract.derive_budget_from_remaining_and_spent {
            match (total_balance, monthly_spent) {
                (Some(remaining), Some(spent)) => Some(remaining.saturating_add(spent)),
                _ => None,
            }
        } else {
            None
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
    let exhausted = first_bool_from_paths(
        value,
        &effective.exhausted_paths,
        &["data.exhausted", "exhausted"],
    )
    .or_else(|| remaining_balance.map(UsdAmount::is_zero));

    let mut snapshot = base_snapshot(provider, upstream, fetched_at_ms, stale_after_ms);
    snapshot.total_balance_usd = remaining_balance.map(amount_to_string);
    snapshot.monthly_budget_usd = monthly_budget.map(amount_to_string);
    snapshot.monthly_spent_usd = monthly_spent.map(amount_to_string);
    snapshot.exhausted = exhausted;
    snapshot.refresh_status(fetched_at_ms);
    snapshot
}

fn update_usage_exhausted(
    lb_states: &Arc<Mutex<HashMap<String, LbState>>>,
    cfg: &ProxyConfig,
    upstreams: &[UpstreamRef],
    exhausted: bool,
) {
    let mut map = match lb_states.lock() {
        Ok(m) => m,
        Err(_) => return,
    };

    for uref in upstreams {
        let service = match cfg.codex.station(&uref.station_name) {
            Some(s) => s,
            None => continue,
        };

        let len = service.upstreams.len();
        let entry = map
            .entry(uref.station_name.clone())
            .or_insert_with(LbState::default);
        if entry.failure_counts.len() != len {
            entry.failure_counts.resize(len, 0);
            entry.cooldown_until.resize(len, None);
            entry.usage_exhausted.resize(len, false);
        }
        if uref.index < entry.usage_exhausted.len() {
            entry.usage_exhausted[uref.index] = exhausted;
        }
    }
}

/// 在特定 Codex upstream 请求结束后，按需查询一次用量并更新 LB 状态。
/// 设计为轻量的“按需刷新”，而非后台定时轮询。
pub async fn poll_for_codex_upstream(
    cfg: Arc<ProxyConfig>,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    state: Arc<ProxyState>,
    service_name: &str,
    station_name: &str,
    upstream_index: usize,
) {
    // Tests should be hermetic and should not depend on any real user `usage_providers.json` on
    // the machine running the suite. Disable provider polling during tests to avoid flakiness.
    if cfg!(test) {
        return;
    }

    let providers_file = load_providers();
    if providers_file.providers.is_empty() {
        return;
    }

    // Locate the current upstream once; if it no longer exists, bail out quietly.
    let current_service = match cfg.codex.station(station_name) {
        Some(s) => s,
        None => return,
    };
    let current_upstream = match current_service.upstreams.get(upstream_index) {
        Some(u) => u,
        None => return,
    };
    let current_base_url = current_upstream.base_url.clone();

    let now = Instant::now();
    let poll_map = LAST_USAGE_POLL.get_or_init(|| Mutex::new(HashMap::new()));

    let mut client: Option<Client> = None;

    for provider in providers_file.providers {
        // Only providers whose domains match the current upstream are considered.
        if !domain_matches(&current_base_url, &provider.domains) {
            continue;
        }

        let Some(interval_secs) = effective_poll_interval_secs(&provider) else {
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

        // For diagnostics, still check whether this provider is associated with
        // multiple hosts across stations, but only once per poll.
        let mut hosts: Vec<String> = Vec::new();
        for service in cfg.codex.stations().values() {
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
        if hosts.len() > 1 {
            warn!(
                "usage provider '{}' is associated with multiple hosts: {:?}; \
将按统一额度处理这些 upstream，如需区分配额请拆分为多个 provider 配置",
                provider.id, hosts
            );
        }

        // Only the current upstream participates in token resolution and usage update.
        let current_ref = UpstreamRef {
            station_name: station_name.to_string(),
            index: upstream_index,
        };
        let upstreams = vec![current_ref];

        let c = client.get_or_insert_with(Client::new);
        let fetched_at_ms = unix_now_ms();
        let stale_after_ms = stale_after_ms(fetched_at_ms, interval_secs);

        if let Some(token) = resolve_token(&provider, &upstreams, &cfg) {
            let snapshot_result = match provider.kind {
                ProviderKind::BudgetHttpJson
                | ProviderKind::OpenAiBalanceHttpJson
                | ProviderKind::NewApiUserSelf
                | ProviderKind::YescodeProfile => {
                    match poll_provider_http_json(c, &provider, &current_base_url, &token).await {
                        Ok(value) => {
                            let snapshot = match provider.kind {
                                ProviderKind::BudgetHttpJson => budget_snapshot_from_json(
                                    &provider,
                                    &upstreams[0],
                                    &value,
                                    fetched_at_ms,
                                    stale_after_ms,
                                ),
                                ProviderKind::YescodeProfile => yescode_snapshot_from_json(
                                    &provider,
                                    &upstreams[0],
                                    &value,
                                    fetched_at_ms,
                                    stale_after_ms,
                                ),
                                ProviderKind::OpenAiBalanceHttpJson => {
                                    balance_http_snapshot_from_json(
                                        &provider,
                                        &upstreams[0],
                                        &value,
                                        fetched_at_ms,
                                        stale_after_ms,
                                    )
                                }
                                ProviderKind::NewApiUserSelf => new_api_snapshot_from_json(
                                    &provider,
                                    &upstreams[0],
                                    &value,
                                    fetched_at_ms,
                                    stale_after_ms,
                                ),
                            };
                            Ok(snapshot)
                        }
                        Err(err) => Err(err),
                    }
                }
            };

            match snapshot_result {
                Ok(snapshot) => {
                    let exhausted_for_lb = snapshot.exhausted.unwrap_or(false);
                    update_usage_exhausted(&lb_states, &cfg, &upstreams, exhausted_for_lb);
                    state
                        .record_provider_balance_snapshot(service_name, snapshot.clone())
                        .await;
                    info!(
                        "usage provider '{}' exhausted = {}",
                        provider.id, exhausted_for_lb
                    );
                }
                Err(err) => {
                    state
                        .record_provider_balance_snapshot(
                            service_name,
                            base_snapshot(&provider, &upstreams[0], fetched_at_ms, stale_after_ms)
                                .with_error(err.to_string()),
                        )
                        .await;
                    warn!("usage provider '{}' poll failed: {}", provider.id, err);
                }
            }
        } else {
            state
                .record_provider_balance_snapshot(
                    service_name,
                    base_snapshot(&provider, &upstreams[0], fetched_at_ms, stale_after_ms)
                        .with_error(
                            "no usable token; checked provider token_env and upstream auth",
                        ),
                )
                .await;
            warn!(
                "usage provider '{}' has no usable token (checked token_env and associated upstream auth_token); \
跳过本次用量查询，请检查 usage_providers.json 和 ~/.codex-helper/config.json",
                provider.id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::balance::BalanceSnapshotStatus;

    fn provider(id: &str, kind: ProviderKind) -> UsageProviderConfig {
        UsageProviderConfig {
            id: id.to_string(),
            kind,
            domains: vec!["example.com".to_string()],
            endpoint: "https://example.com/usage".to_string(),
            token_env: None,
            poll_interval_secs: Some(60),
            refresh_on_request: true,
            headers: BTreeMap::new(),
            variables: BTreeMap::new(),
            extract: UsageProviderExtractConfig::default(),
        }
    }

    fn upstream() -> UpstreamRef {
        UpstreamRef {
            station_name: "right".to_string(),
            index: 1,
        }
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
        assert_eq!(snapshot.total_balance_usd.as_deref(), Some("1"));
        assert_eq!(snapshot.monthly_spent_usd.as_deref(), Some("0.5"));
        assert_eq!(snapshot.monthly_budget_usd.as_deref(), Some("1.5"));
    }
}
