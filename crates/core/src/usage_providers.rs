use std::collections::HashMap;
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
}

impl ProviderKind {
    fn source_name(&self) -> &'static str {
        match self {
            ProviderKind::BudgetHttpJson => "usage_provider:budget_http_json",
            ProviderKind::YescodeProfile => "usage_provider:yescode_profile",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct UsageProviderConfig {
    id: String,
    kind: ProviderKind,
    domains: Vec<String>,
    endpoint: String,
    #[serde(default)]
    token_env: Option<String>,
    #[serde(default)]
    poll_interval_secs: Option<u64>,
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

// Minimal poll interval per provider to avoid hammering usage APIs.
const MIN_POLL_INTERVAL_SECS: u64 = 20;

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn stale_after_ms(fetched_at_ms: u64, interval_secs: u64) -> Option<u64> {
    fetched_at_ms.checked_add(interval_secs.saturating_mul(3).saturating_mul(1_000))
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
            },
            UsageProviderConfig {
                id: "yescode".to_string(),
                kind: ProviderKind::YescodeProfile,
                // yes.vg 匹配 co.yes.vg / cotest.yes.vg 等子域名
                domains: vec!["yes.vg".to_string()],
                endpoint: "https://co.yes.vg/api/v1/auth/profile".to_string(),
                token_env: None,
                poll_interval_secs: Some(60),
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

async fn poll_budget_http_json(
    client: &Client,
    endpoint: &str,
    token: &str,
) -> Result<serde_json::Value> {
    let resp = client
        .get(endpoint)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("usage provider HTTP {}", resp.status());
    }
    Ok(resp.json().await?)
}

async fn poll_yescode_profile(
    client: &Client,
    endpoint: &str,
    token: &str,
) -> Result<serde_json::Value> {
    let resp = client
        .get(endpoint)
        .header("X-API-Key", token)
        .header("Accept", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("yescode profile HTTP {}", resp.status());
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

fn amount_from_key(value: &serde_json::Value, key: &str) -> Option<UsdAmount> {
    value.get(key).and_then(amount_from_json)
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
    let monthly_budget = amount_from_key(value, "monthly_budget_usd");
    let monthly_spent = amount_from_key(value, "monthly_spent_usd");
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
    let subscription_balance = amount_from_key(value, "subscription_balance");
    let paygo_balance = amount_from_key(value, "pay_as_you_go_balance");
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

        // Compute effective poll interval with a global minimum to avoid hammering.
        let mut interval_secs = provider
            .poll_interval_secs
            .unwrap_or(MIN_POLL_INTERVAL_SECS);
        if interval_secs < MIN_POLL_INTERVAL_SECS {
            interval_secs = MIN_POLL_INTERVAL_SECS;
        }

        if interval_secs > 0 {
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
            match provider.kind {
                ProviderKind::BudgetHttpJson => {
                    match poll_budget_http_json(c, &provider.endpoint, &token).await {
                        Ok(value) => {
                            let snapshot = budget_snapshot_from_json(
                                &provider,
                                &upstreams[0],
                                &value,
                                fetched_at_ms,
                                stale_after_ms,
                            );
                            let exhausted_for_lb = snapshot.exhausted.unwrap_or(false);
                            update_usage_exhausted(&lb_states, &cfg, &upstreams, exhausted_for_lb);
                            state
                                .record_provider_balance_snapshot(service_name, snapshot.clone())
                                .await;
                            info!(
                                "usage provider '{}' exhausted = {} (monthly: {}/{})",
                                provider.id,
                                exhausted_for_lb,
                                snapshot.monthly_spent_usd.as_deref().unwrap_or("unknown"),
                                snapshot.monthly_budget_usd.as_deref().unwrap_or("unknown")
                            );
                        }
                        Err(err) => {
                            state
                                .record_provider_balance_snapshot(
                                    service_name,
                                    base_snapshot(
                                        &provider,
                                        &upstreams[0],
                                        fetched_at_ms,
                                        stale_after_ms,
                                    )
                                    .with_error(err.to_string()),
                                )
                                .await;
                            warn!("usage provider '{}' poll failed: {}", provider.id, err);
                        }
                    }
                }
                ProviderKind::YescodeProfile => {
                    match poll_yescode_profile(c, &provider.endpoint, &token).await {
                        Ok(value) => {
                            let snapshot = yescode_snapshot_from_json(
                                &provider,
                                &upstreams[0],
                                &value,
                                fetched_at_ms,
                                stale_after_ms,
                            );
                            let exhausted_for_lb = snapshot.exhausted.unwrap_or(false);
                            update_usage_exhausted(&lb_states, &cfg, &upstreams, exhausted_for_lb);
                            state
                                .record_provider_balance_snapshot(service_name, snapshot.clone())
                                .await;
                            info!(
                                "usage provider '{}' exhausted = {} (yescode balance: total={}, subscription={}, paygo={})",
                                provider.id,
                                exhausted_for_lb,
                                snapshot.total_balance_usd.as_deref().unwrap_or("unknown"),
                                snapshot
                                    .subscription_balance_usd
                                    .as_deref()
                                    .unwrap_or("unknown"),
                                snapshot.paygo_balance_usd.as_deref().unwrap_or("unknown")
                            );
                        }
                        Err(err) => {
                            state
                                .record_provider_balance_snapshot(
                                    service_name,
                                    base_snapshot(
                                        &provider,
                                        &upstreams[0],
                                        fetched_at_ms,
                                        stale_after_ms,
                                    )
                                    .with_error(err.to_string()),
                                )
                                .await;
                            warn!("usage provider '{}' poll failed: {}", provider.id, err);
                        }
                    }
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
}
