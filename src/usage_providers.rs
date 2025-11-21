use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::{ProxyConfig, proxy_home_dir};
use crate::lb::LbState;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProviderKind {
    /// 简单预算接口，返回 total/used，判断是否用尽
    BudgetHttpJson,
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
    config_name: String,
    index: usize,
}

// 全局节流状态：按 provider.id 记录最近一次查询时间，避免高频请求。
static LAST_USAGE_POLL: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

// Minimal poll interval per provider to avoid hammering usage APIs.
const MIN_POLL_INTERVAL_SECS: u64 = 20;

fn usage_providers_path() -> std::path::PathBuf {
    proxy_home_dir().join("usage_providers.json")
}

fn default_providers() -> UsageProvidersFile {
    UsageProvidersFile {
        providers: vec![UsageProviderConfig {
            id: "packycode".to_string(),
            kind: ProviderKind::BudgetHttpJson,
            domains: vec!["packycode.com".to_string()],
            endpoint: "https://www.packycode.com/api/backend/users/info".to_string(),
            token_env: None,
            poll_interval_secs: Some(60),
        }],
    }
}

fn load_providers() -> UsageProvidersFile {
    let path = usage_providers_path();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(file) = serde_json::from_str::<UsageProvidersFile>(&text)
    {
        return file;
    }

    // 写入默认配置（当前仅包含 packycode），方便用户查看/修改
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
        if let Some(service) = cfg.codex.configs.get(&uref.config_name)
            && let Some(up) = service.upstreams.get(uref.index)
            && let Some(token) = &up.auth.auth_token
            && !token.trim().is_empty()
        {
            return Some(token.clone());
        }
    }
    None
}

async fn poll_budget_http_json(
    client: &Client,
    endpoint: &str,
    token: &str,
) -> Result<(bool, f64, f64)> {
    let resp = client
        .get(endpoint)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("usage provider HTTP {}", resp.status());
    }
    let value: serde_json::Value = resp.json().await?;

    let monthly_budget = value
        .get("monthly_budget_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let monthly_spent = value
        .get("monthly_spent_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let exhausted = monthly_budget > 0.0 && monthly_spent >= monthly_budget;
    Ok((exhausted, monthly_budget, monthly_spent))
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
        let service = match cfg.codex.configs.get(&uref.config_name) {
            Some(s) => s,
            None => continue,
        };

        let len = service.upstreams.len();
        let entry = map
            .entry(uref.config_name.clone())
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
    config_name: &str,
    upstream_index: usize,
) {
    let providers_file = load_providers();
    if providers_file.providers.is_empty() {
        return;
    }

    // Locate the current upstream once; if it no longer exists, bail out quietly.
    let current_service = match cfg.codex.configs.get(config_name) {
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
        // multiple hosts across configs, but only once per poll.
        let mut hosts: Vec<String> = Vec::new();
        for service in cfg.codex.configs.values() {
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
            config_name: config_name.to_string(),
            index: upstream_index,
        };
        let upstreams = vec![current_ref];

        let c = client.get_or_insert_with(Client::new);

        if let Some(token) = resolve_token(&provider, &upstreams, &cfg) {
            let exhausted = match provider.kind {
                ProviderKind::BudgetHttpJson => {
                    match poll_budget_http_json(c, &provider.endpoint, &token).await {
                        Ok((exhausted, monthly_budget, monthly_spent)) => {
                            update_usage_exhausted(&lb_states, &cfg, &upstreams, exhausted);
                            info!(
                                "usage provider '{}' exhausted = {} (monthly: {:.2}/{:.2} USD)",
                                provider.id, exhausted, monthly_spent, monthly_budget
                            );
                            exhausted
                        }
                        Err(err) => {
                            warn!("usage provider '{}' poll failed: {}", provider.id, err);
                            false
                        }
                    }
                }
            };

            // exhausted 状态已经在 update_usage_exhausted 中更新；这里不需要额外处理。
            let _ = exhausted;
        } else {
            warn!(
                "usage provider '{}' has no usable token (checked token_env and associated upstream auth_token); \
跳过本次用量查询，请检查 usage_providers.json 和 ~/.codex-helper/config.json",
                provider.id
            );
        }
    }
}
