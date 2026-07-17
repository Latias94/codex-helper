use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::auth_resolution::resolve_upstream_auth_for_target;
use crate::config::{
    HelperConfig, ServiceRouteConfig, ServiceStatusConfig, ServiceStatusProbeConfig, UpstreamAuth,
};

static SERVICE_STATUS_CACHE: OnceLock<Mutex<ServiceStatusCache>> = OnceLock::new();
const UPSTREAM_AUTH_UNAVAILABLE_REASON: &str = "configured upstream credentials are unavailable";

#[derive(Debug, Default)]
struct ServiceStatusCache {
    config_key: Option<u64>,
    last_refresh: Option<Instant>,
    snapshot: Option<ServiceStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatusSnapshot {
    pub generated_at_ms: u64,
    pub configured: bool,
    pub enabled: bool,
    pub refresh_interval_secs: u64,
    pub history_cells: usize,
    #[serde(default)]
    pub probes: Vec<ServiceStatusProbeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ServiceStatusSnapshot {
    pub fn disabled(config: &ServiceStatusConfig) -> Self {
        Self {
            generated_at_ms: unix_now_ms(),
            configured: config.has_probes(),
            enabled: config.enabled,
            refresh_interval_secs: config.refresh_interval_secs,
            history_cells: config.history_cells,
            probes: Vec::new(),
            error: None,
        }
    }

    pub fn status_counts(&self) -> ServiceStatusCounts {
        let mut counts = ServiceStatusCounts::default();
        for probe in &self.probes {
            for service in &probe.services {
                counts.record(service.latest_kind);
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatusProbeSnapshot {
    pub id: String,
    pub url: String,
    pub fetched_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_ok: Option<bool>,
    #[serde(default)]
    pub services: Vec<ServiceStatusServiceSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatusServiceSnapshot {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_pct: Option<String>,
    pub latest_kind: ServiceStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<ServiceStatusProbeSample>,
    #[serde(default)]
    pub history: Vec<ServiceStatusCellSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatusCellSnapshot {
    pub kind: ServiceStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<ServiceStatusProbeSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatusProbeSample {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatusKind {
    Ok,
    Slow,
    Failed,
    #[default]
    Unknown,
}

impl ServiceStatusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceStatusKind::Ok => "ok",
            ServiceStatusKind::Slow => "slow",
            ServiceStatusKind::Failed => "failed",
            ServiceStatusKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStatusCounts {
    pub ok: usize,
    pub slow: usize,
    pub failed: usize,
    pub unknown: usize,
}

impl ServiceStatusCounts {
    pub fn record(&mut self, kind: ServiceStatusKind) {
        match kind {
            ServiceStatusKind::Ok => self.ok += 1,
            ServiceStatusKind::Slow => self.slow += 1,
            ServiceStatusKind::Failed => self.failed += 1,
            ServiceStatusKind::Unknown => self.unknown += 1,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawServiceStatusResponse {
    #[serde(default, alias = "allOK")]
    all_ok: Option<bool>,
    #[serde(default, alias = "generatedAt")]
    generated_at: Option<serde_json::Value>,
    #[serde(default)]
    services: Vec<RawServiceStatusService>,
}

#[derive(Debug, Deserialize)]
struct RawServiceStatusService {
    model: String,
    #[serde(default, alias = "uptimePct")]
    uptime_pct: Option<serde_json::Value>,
    #[serde(default)]
    last: Option<RawServiceStatusProbe>,
    #[serde(default)]
    history: Vec<RawServiceStatusProbe>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawServiceStatusProbe {
    #[serde(default)]
    ts: Option<serde_json::Value>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default, alias = "latencyMS")]
    latency_ms: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct ProviderProbeTarget {
    service_name: String,
    provider_id: String,
    endpoint_id: String,
    base_url: String,
    priority: u32,
    auth: UpstreamAuth,
    tags: HashMap<String, String>,
    supported_models: HashMap<String, bool>,
    model_mapping: HashMap<String, String>,
}

pub async fn refresh_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
) -> ServiceStatusSnapshot {
    if !config.is_active() {
        return ServiceStatusSnapshot::disabled(config);
    }

    let config_key = service_status_cache_key(config, runtime_config, service_name);
    let interval = Duration::from_secs(config.refresh_interval_secs.max(1));
    if let Some(cached) = cached_service_status_snapshot(config_key, interval) {
        return cached;
    }

    let snapshot = fetch_service_status_snapshot(config, runtime_config, service_name).await;
    store_service_status_snapshot(config_key, snapshot.clone());
    snapshot
}

fn cached_service_status_snapshot(
    config_key: u64,
    interval: Duration,
) -> Option<ServiceStatusSnapshot> {
    let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
    let guard = cache.lock().ok()?;
    if guard.config_key != Some(config_key) {
        return None;
    }
    let last_refresh = guard.last_refresh?;
    if last_refresh.elapsed() < interval {
        return guard.snapshot.clone();
    }
    None
}

fn store_service_status_snapshot(config_key: u64, snapshot: ServiceStatusSnapshot) {
    let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
    if let Ok(mut guard) = cache.lock() {
        guard.config_key = Some(config_key);
        guard.last_refresh = Some(Instant::now());
        guard.snapshot = Some(snapshot);
    }
}

fn service_status_cache_key(
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    service_name.hash(&mut hasher);
    config.enabled.hash(&mut hasher);
    config.refresh_interval_secs.hash(&mut hasher);
    config.timeout_ms.hash(&mut hasher);
    config.high_latency_ms.hash(&mut hasher);
    config.history_cells.hash(&mut hasher);
    for probe in &config.probes {
        probe.id.hash(&mut hasher);
        probe.provider.hash(&mut hasher);
        probe.endpoint.hash(&mut hasher);
        probe.url.hash(&mut hasher);
        probe.models.hash(&mut hasher);
        probe.timeout_ms.hash(&mut hasher);
        probe.high_latency_ms.hash(&mut hasher);
        probe.headers.hash(&mut hasher);
    }
    if let Some(runtime_config) = runtime_config
        && let Some(service) = service_route_config(runtime_config, service_name)
    {
        for target in provider_probe_targets(service_name, service, None, None) {
            hash_provider_probe_target(&target, &mut hasher);
        }
    }
    hasher.finish()
}

fn hash_provider_probe_target(target: &ProviderProbeTarget, hasher: &mut impl Hasher) {
    target.provider_id.hash(hasher);
    target.endpoint_id.hash(hasher);
    target.base_url.hash(hasher);
    target.priority.hash(hasher);

    let mut tags = target.tags.iter().collect::<Vec<_>>();
    tags.sort_by(|left, right| left.0.cmp(right.0));
    tags.hash(hasher);
    let mut supported_models = target.supported_models.iter().collect::<Vec<_>>();
    supported_models.sort_by(|left, right| left.0.cmp(right.0));
    supported_models.hash(hasher);
    let mut model_mapping = target.model_mapping.iter().collect::<Vec<_>>();
    model_mapping.sort_by(|left, right| left.0.cmp(right.0));
    model_mapping.hash(hasher);
    target.auth.auth_token.hash(hasher);
    target.auth.auth_token_env.hash(hasher);
    target.auth.api_key.hash(hasher);
    target.auth.api_key_env.hash(hasher);
    target.auth.allow_anonymous.hash(hasher);
}

async fn fetch_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
) -> ServiceStatusSnapshot {
    let client = match Client::builder()
        .timeout(Duration::from_millis(config.timeout_ms.max(1)))
        .connect_timeout(Duration::from_millis(config.timeout_ms.max(1)))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return ServiceStatusSnapshot {
                generated_at_ms: unix_now_ms(),
                configured: config.is_active(),
                enabled: config.enabled,
                refresh_interval_secs: config.refresh_interval_secs,
                history_cells: config.history_cells,
                probes: Vec::new(),
                error: Some(format!("service status client setup failed: {err}")),
            };
        }
    };

    let mut futures = config
        .probes
        .iter()
        .filter(|probe| probe_has_target(probe))
        .map(|probe| fetch_probe(&client, config, runtime_config, service_name, probe))
        .collect::<FuturesUnordered<_>>();

    let mut probes = Vec::new();
    while let Some(probe) = futures.next().await {
        probes.push(probe);
    }
    probes.sort_by(|a, b| a.id.cmp(&b.id));

    ServiceStatusSnapshot {
        generated_at_ms: unix_now_ms(),
        configured: true,
        enabled: config.enabled,
        refresh_interval_secs: config.refresh_interval_secs,
        history_cells: config.history_cells,
        probes,
        error: None,
    }
}

async fn fetch_probe(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> ServiceStatusProbeSnapshot {
    let fetched_at_ms = unix_now_ms();
    let id = probe_id(probe);
    match fetch_probe_inner(client, config, runtime_config, service_name, probe).await {
        Ok(mut snapshot) => {
            snapshot.id = id;
            snapshot.fetched_at_ms = fetched_at_ms;
            snapshot
        }
        Err(err) => ServiceStatusProbeSnapshot {
            id,
            url: probe_target_label(probe),
            fetched_at_ms,
            generated_at_ms: None,
            all_ok: None,
            services: probe
                .models
                .iter()
                .filter(|model| !model.trim().is_empty())
                .map(|model| missing_service_row(model, config.history_cells))
                .collect(),
            error: Some(err.to_string()),
        },
    }
}

async fn fetch_probe_inner(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> Result<ServiceStatusProbeSnapshot> {
    if has_provider_probe_target(probe) {
        return fetch_provider_probe(client, config, runtime_config, service_name, probe).await;
    }
    let Some(url) = probe
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        anyhow::bail!("service status probe needs provider or url");
    };
    let timeout_ms = probe.timeout_ms.unwrap_or(config.timeout_ms).max(1);
    let mut request = client.get(url).timeout(Duration::from_millis(timeout_ms));
    for (name, value) in &probe.headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("read response body from {url}"))?;
    if !status.is_success() {
        anyhow::bail!("status API returned HTTP {status}");
    }
    snapshot_from_status_json(
        probe,
        config.history_cells,
        probe.high_latency_ms.unwrap_or(config.high_latency_ms),
        &body,
    )
    .with_context(|| format!("decode service status response from {url}"))
}

async fn fetch_provider_probe(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> Result<ServiceStatusProbeSnapshot> {
    let target = provider_probe_target(runtime_config, service_name, probe)?;
    let timeout_ms = probe.timeout_ms.unwrap_or(config.timeout_ms).max(1);
    let high_latency_ms = probe.high_latency_ms.unwrap_or(config.high_latency_ms);
    let models = provider_probe_models(probe, &target);
    if models.is_empty() {
        anyhow::bail!(
            "provider probe has no model; configure models for {}",
            target.provider_id
        );
    }

    let mut services = Vec::with_capacity(models.len());
    for model in models {
        services.push(
            fetch_provider_model_probe(
                client,
                &target,
                &model,
                timeout_ms,
                high_latency_ms,
                config.history_cells,
            )
            .await,
        );
    }

    let all_ok = Some(
        !services.is_empty()
            && services.iter().all(|service| {
                matches!(
                    service.latest_kind,
                    ServiceStatusKind::Ok | ServiceStatusKind::Slow
                )
            }),
    );

    Ok(ServiceStatusProbeSnapshot {
        id: probe_id(probe),
        url: provider_target_label(&target),
        fetched_at_ms: 0,
        generated_at_ms: Some(unix_now_ms()),
        all_ok,
        services,
        error: None,
    })
}

async fn fetch_provider_model_probe(
    client: &Client,
    target: &ProviderProbeTarget,
    requested_model: &str,
    timeout_ms: u64,
    high_latency_ms: u64,
    history_cells: usize,
) -> ServiceStatusServiceSnapshot {
    let started = Instant::now();
    let model = crate::model_routing::effective_model(&target.model_mapping, requested_model);
    let url = chat_completions_url(&target.base_url);
    let result = send_provider_probe_request(client, target, &url, &model, timeout_ms).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let sample = match result {
        Ok(()) => ServiceStatusProbeSample {
            ts_ms: Some(unix_now_ms()),
            ok: Some(true),
            latency_ms: Some(latency_ms),
            error: None,
        },
        Err(err) => ServiceStatusProbeSample {
            ts_ms: Some(unix_now_ms()),
            ok: Some(false),
            latency_ms: Some(latency_ms),
            error: Some(err.to_string()),
        },
    };
    let kind = classify_probe(Some(&sample), high_latency_ms);
    let missing_count = history_cells.saturating_sub(1);
    let mut history = Vec::with_capacity(history_cells.max(1));
    history.extend((0..missing_count).map(|_| ServiceStatusCellSnapshot {
        kind: ServiceStatusKind::Unknown,
        probe: None,
    }));
    history.push(ServiceStatusCellSnapshot {
        kind,
        probe: Some(sample.clone()),
    });

    ServiceStatusServiceSnapshot {
        model: requested_model.to_string(),
        uptime_pct: None,
        latest_kind: kind,
        latest: Some(sample),
        history,
    }
}

async fn send_provider_probe_request(
    client: &Client,
    target: &ProviderProbeTarget,
    url: &str,
    model: &str,
    timeout_ms: u64,
) -> Result<()> {
    let resolved_auth =
        resolve_upstream_auth_for_target(target.service_name.as_str(), &target.auth, url)
            .map_err(|_| anyhow::anyhow!(UPSTREAM_AUTH_UNAVAILABLE_REASON))?;
    let mut request = client
        .post(url)
        .timeout(Duration::from_millis(timeout_ms))
        .json(&serde_json::json!({
            "model": model,
            "messages": [
                { "role": "user", "content": "ping" }
            ],
            "max_tokens": 1,
            "temperature": 0,
            "stream": false
        }));

    if let Some(token) = resolved_auth.auth_token.value() {
        request = request.bearer_auth(token);
    }
    if let Some(key) = resolved_auth.api_key.value() {
        request = request.header("x-api-key", key);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("probe request failed for {}", target.provider_id))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("provider probe HTTP {status}: {}", concise_body(&body));
    }
    Ok(())
}

fn provider_probe_target(
    runtime_config: Option<&HelperConfig>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> Result<ProviderProbeTarget> {
    let provider = probe
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("provider probe missing provider")?;
    let endpoint = probe
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let config = runtime_config.context("provider probes require runtime config")?;
    let service = service_route_config(config, service_name)
        .with_context(|| format!("unknown service for provider probe: {service_name}"))?;

    provider_probe_targets(service_name, service, Some(provider), endpoint)
        .into_iter()
        .next()
        .with_context(|| {
            endpoint
                .map(|endpoint| {
                    format!("provider probe target not found: {service_name}/{provider}/{endpoint}")
                })
                .unwrap_or_else(|| {
                    format!("provider probe target not found: {service_name}/{provider}")
                })
        })
}

fn provider_probe_targets(
    service_name: &str,
    service: &ServiceRouteConfig,
    provider_filter: Option<&str>,
    endpoint: Option<&str>,
) -> Vec<ProviderProbeTarget> {
    let mut targets = Vec::new();
    for (provider_id, provider) in &service.providers {
        if !provider.enabled || provider_filter.is_some_and(|filter| filter != provider_id.as_str())
        {
            continue;
        }

        let auth = merge_auth(&provider.auth, &provider.inline_auth);
        if endpoint.is_none_or(|endpoint| endpoint == "default")
            && let Some(base_url) = provider
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        {
            targets.push(ProviderProbeTarget {
                service_name: service_name.to_string(),
                provider_id: provider_id.clone(),
                endpoint_id: "default".to_string(),
                base_url: base_url.to_string(),
                priority: 0,
                auth: auth.clone(),
                tags: provider.tags.clone().into_iter().collect(),
                supported_models: provider.supported_models.clone().into_iter().collect(),
                model_mapping: provider.model_mapping.clone().into_iter().collect(),
            });
        }

        for (endpoint_id, endpoint_config) in &provider.endpoints {
            if !endpoint_config.enabled
                || endpoint.is_some_and(|filter| filter != endpoint_id.as_str())
            {
                continue;
            }
            let base_url = endpoint_config.base_url.trim();
            if base_url.is_empty() {
                continue;
            }
            targets.push(ProviderProbeTarget {
                service_name: service_name.to_string(),
                provider_id: provider_id.clone(),
                endpoint_id: endpoint_id.clone(),
                base_url: base_url.to_string(),
                priority: endpoint_config.priority,
                auth: auth.clone(),
                tags: merge_maps(&provider.tags, &endpoint_config.tags),
                supported_models: merge_maps(
                    &provider.supported_models,
                    &endpoint_config.supported_models,
                ),
                model_mapping: merge_maps(&provider.model_mapping, &endpoint_config.model_mapping),
            });
        }
    }
    targets.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    targets
}

fn merge_auth(block: &UpstreamAuth, inline: &UpstreamAuth) -> UpstreamAuth {
    UpstreamAuth {
        auth_token: inline
            .auth_token
            .clone()
            .or_else(|| block.auth_token.clone()),
        auth_token_env: inline
            .auth_token_env
            .clone()
            .or_else(|| block.auth_token_env.clone()),
        api_key: inline.api_key.clone().or_else(|| block.api_key.clone()),
        api_key_env: inline
            .api_key_env
            .clone()
            .or_else(|| block.api_key_env.clone()),
        allow_anonymous: inline.allow_anonymous.or(block.allow_anonymous),
    }
}

fn merge_maps<T: Clone>(
    provider_values: &std::collections::BTreeMap<String, T>,
    endpoint_values: &std::collections::BTreeMap<String, T>,
) -> HashMap<String, T> {
    let mut merged = provider_values.clone();
    merged.extend(endpoint_values.clone());
    merged.into_iter().collect()
}

fn provider_probe_models(
    probe: &ServiceStatusProbeConfig,
    target: &ProviderProbeTarget,
) -> Vec<String> {
    let mut models = if probe.models.is_empty() {
        target
            .supported_models
            .iter()
            .filter(|(model, supported)| **supported && !model.contains('*'))
            .map(|(model, _)| model.clone())
            .collect::<Vec<_>>()
    } else {
        probe.models.clone()
    };
    if models.is_empty() {
        models = target
            .model_mapping
            .keys()
            .filter(|model| !model.contains('*'))
            .cloned()
            .collect();
    }
    models.retain(|model| !model.trim().is_empty());
    models.sort();
    models.dedup();
    models
}

fn service_route_config<'a>(
    config: &'a HelperConfig,
    service_name: &str,
) -> Option<&'a ServiceRouteConfig> {
    match service_name {
        "claude" => Some(&config.claude),
        "codex" => Some(&config.codex),
        _ => None,
    }
}

fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn provider_target_label(target: &ProviderProbeTarget) -> String {
    format!(
        "{}/{} {}",
        target.provider_id, target.endpoint_id, target.base_url
    )
}

fn probe_has_target(probe: &ServiceStatusProbeConfig) -> bool {
    has_provider_probe_target(probe)
        || probe
            .url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn has_provider_probe_target(probe: &ServiceStatusProbeConfig) -> bool {
    probe
        .provider
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn probe_target_label(probe: &ServiceStatusProbeConfig) -> String {
    if let Some(provider) = probe
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return match probe
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(endpoint) => format!("{provider}/{endpoint}"),
            None => provider.to_string(),
        };
    }
    probe
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn concise_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "-".to_string();
    }
    if body.chars().count() <= 180 {
        body.to_string()
    } else {
        format!("{}...", body.chars().take(180).collect::<String>())
    }
}

fn snapshot_from_status_json(
    probe: &ServiceStatusProbeConfig,
    history_cells: usize,
    high_latency_ms: u64,
    body: &str,
) -> Result<ServiceStatusProbeSnapshot> {
    let raw: RawServiceStatusResponse = serde_json::from_str(body)?;
    let services_by_model = raw
        .services
        .into_iter()
        .map(|service| (service.model.clone(), service))
        .collect::<HashMap<_, _>>();
    let models = if probe.models.is_empty() {
        let mut models = services_by_model.keys().cloned().collect::<Vec<_>>();
        models.sort();
        models
    } else {
        probe.models.clone()
    };

    let services = models
        .into_iter()
        .filter(|model| !model.trim().is_empty())
        .map(|model| {
            services_by_model
                .get(model.as_str())
                .map(|service| service_snapshot(service, history_cells, high_latency_ms))
                .unwrap_or_else(|| missing_service_row(model.as_str(), history_cells))
        })
        .collect::<Vec<_>>();

    Ok(ServiceStatusProbeSnapshot {
        id: probe_id(probe),
        url: probe_target_label(probe),
        fetched_at_ms: 0,
        generated_at_ms: raw.generated_at.as_ref().and_then(ms_from_json),
        all_ok: raw.all_ok,
        services,
        error: None,
    })
}

fn service_snapshot(
    raw: &RawServiceStatusService,
    history_cells: usize,
    high_latency_ms: u64,
) -> ServiceStatusServiceSnapshot {
    let latest = raw.last.as_ref().map(sample_from_raw);
    let latest_kind = classify_probe(latest.as_ref(), high_latency_ms);
    let mut history = raw
        .history
        .iter()
        .rev()
        .take(history_cells)
        .map(sample_from_raw)
        .collect::<Vec<_>>();
    history.reverse();
    let missing_count = history_cells.saturating_sub(history.len());
    let mut cells = Vec::with_capacity(history_cells);
    cells.extend((0..missing_count).map(|_| ServiceStatusCellSnapshot {
        kind: ServiceStatusKind::Unknown,
        probe: None,
    }));
    cells.extend(history.into_iter().map(|sample| ServiceStatusCellSnapshot {
        kind: classify_probe(Some(&sample), high_latency_ms),
        probe: Some(sample),
    }));

    ServiceStatusServiceSnapshot {
        model: raw.model.clone(),
        uptime_pct: raw.uptime_pct.as_ref().and_then(decimal_string_from_json),
        latest_kind,
        latest,
        history: cells,
    }
}

fn missing_service_row(model: &str, history_cells: usize) -> ServiceStatusServiceSnapshot {
    ServiceStatusServiceSnapshot {
        model: model.to_string(),
        uptime_pct: None,
        latest_kind: ServiceStatusKind::Unknown,
        latest: None,
        history: (0..history_cells)
            .map(|_| ServiceStatusCellSnapshot {
                kind: ServiceStatusKind::Unknown,
                probe: None,
            })
            .collect(),
    }
}

fn classify_probe(
    sample: Option<&ServiceStatusProbeSample>,
    high_latency_ms: u64,
) -> ServiceStatusKind {
    let Some(sample) = sample else {
        return ServiceStatusKind::Unknown;
    };
    match sample.ok {
        Some(false) => ServiceStatusKind::Failed,
        Some(true) => match sample.latency_ms {
            Some(latency) if latency >= high_latency_ms => ServiceStatusKind::Slow,
            Some(_) => ServiceStatusKind::Ok,
            None => ServiceStatusKind::Unknown,
        },
        None => ServiceStatusKind::Unknown,
    }
}

fn sample_from_raw(raw: &RawServiceStatusProbe) -> ServiceStatusProbeSample {
    ServiceStatusProbeSample {
        ts_ms: raw.ts.as_ref().and_then(ms_from_json),
        ok: raw.ok,
        latency_ms: raw.latency_ms.as_ref().and_then(u64_from_json),
        error: raw
            .error
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }
}

fn probe_id(probe: &ServiceStatusProbeConfig) -> String {
    probe
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| probe_target_label(probe))
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ms_from_json(value: &serde_json::Value) -> Option<u64> {
    let raw = match value {
        serde_json::Value::Number(number) => number.as_f64()?,
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if raw <= 0.0 {
        return None;
    }
    if raw < 10_000_000_000.0 {
        Some((raw * 1_000.0) as u64)
    } else {
        Some(raw as u64)
    }
}

fn u64_from_json(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64().map(|value| value.max(0.0) as u64),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.max(0.0) as u64),
        _ => None,
    }
}

fn decimal_string_from_json(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    use crate::config::{
        ProviderConcurrencyLimits, ProviderConfig, ProviderEndpointConfig, ServiceRouteConfig,
    };

    #[derive(Debug, Clone, Default)]
    struct CapturedProviderProbeRequest {
        body: serde_json::Value,
        authorization: Option<String>,
        api_key: Option<String>,
    }

    fn probe(models: Vec<&str>) -> ServiceStatusProbeConfig {
        ServiceStatusProbeConfig {
            id: Some("openai".to_string()),
            provider: None,
            endpoint: None,
            url: Some("https://status.example.com/api/status".to_string()),
            models: models.into_iter().map(str::to_string).collect(),
            timeout_ms: None,
            high_latency_ms: None,
            headers: Default::default(),
        }
    }

    fn canonical_runtime_with_endpoint(base_url: String) -> HelperConfig {
        HelperConfig {
            codex: ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([(
                    "relay".to_string(),
                    ProviderConfig {
                        auth: UpstreamAuth {
                            auth_token: Some("test-token".to_string()),
                            api_key: Some("provider-api-key".to_string()),
                            ..UpstreamAuth::default()
                        },
                        inline_auth: UpstreamAuth {
                            api_key: Some("endpoint-api-key".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: std::collections::BTreeMap::from([
                            ("region".to_string(), "provider-region".to_string()),
                            ("shared".to_string(), "provider".to_string()),
                        ]),
                        supported_models: std::collections::BTreeMap::from([
                            ("gpt-5.5".to_string(), true),
                            ("legacy".to_string(), true),
                        ]),
                        model_mapping: std::collections::BTreeMap::from([
                            ("gpt-5.5".to_string(), "provider-gpt-5.5".to_string()),
                            ("legacy".to_string(), "provider-legacy".to_string()),
                        ]),
                        endpoints: std::collections::BTreeMap::from([(
                            "fast".to_string(),
                            ProviderEndpointConfig {
                                base_url,
                                continuity_domain: None,
                                enabled: true,
                                priority: 3,
                                tags: std::collections::BTreeMap::from([
                                    ("shared".to_string(), "endpoint".to_string()),
                                    ("zone".to_string(), "edge".to_string()),
                                ]),
                                supported_models: std::collections::BTreeMap::from([
                                    ("legacy".to_string(), false),
                                    ("endpoint-only".to_string(), true),
                                ]),
                                model_mapping: std::collections::BTreeMap::from([(
                                    "gpt-5.5".to_string(),
                                    "upstream-gpt-5.5".to_string(),
                                )]),
                                limits: ProviderConcurrencyLimits::default(),
                            },
                        )]),
                        ..ProviderConfig::default()
                    },
                )]),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    #[test]
    fn service_status_json_decodes_cells_and_missing_models() {
        let snapshot = snapshot_from_status_json(
            &probe(vec!["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"]),
            4,
            3_000,
            r#"{
              "all_ok": true,
              "generated_at": 1778762578,
              "services": [
                {
                  "model": "gpt-5.5",
                  "uptime_pct": "81.67",
                  "last": { "ts": 1778762557, "ok": true, "latency_ms": 1111, "error": null },
                  "history": [
                    { "ts": 1, "ok": true, "latency_ms": 1000, "error": null },
                    { "ts": 2, "ok": true, "latency_ms": 3000, "error": null },
                    { "ts": 3, "ok": false, "latency_ms": null, "error": "timeout" }
                  ]
                },
                {
                  "model": "gpt-5.4-mini",
                  "uptime_pct": 65,
                  "last": { "ts": 4, "ok": true, "latency_ms": 3001, "error": null },
                  "history": [
                    { "ts": 4, "ok": true, "latency_ms": 3001, "error": null }
                  ]
                }
              ]
            }"#,
        )
        .expect("snapshot");

        assert_eq!(snapshot.id, "openai");
        assert_eq!(snapshot.generated_at_ms, Some(1_778_762_578_000));
        assert_eq!(snapshot.services.len(), 3);
        assert_eq!(snapshot.services[0].latest_kind, ServiceStatusKind::Ok);
        assert_eq!(snapshot.services[0].history.len(), 4);
        assert_eq!(
            snapshot.services[0]
                .history
                .iter()
                .map(|cell| cell.kind)
                .collect::<Vec<_>>(),
            vec![
                ServiceStatusKind::Unknown,
                ServiceStatusKind::Ok,
                ServiceStatusKind::Slow,
                ServiceStatusKind::Failed,
            ]
        );
        assert_eq!(snapshot.services[1].latest_kind, ServiceStatusKind::Unknown);
        assert_eq!(snapshot.services[2].latest_kind, ServiceStatusKind::Slow);
    }

    #[test]
    fn canonical_provider_probe_target_merges_provider_and_endpoint_fields() {
        let runtime = canonical_runtime_with_endpoint("https://relay.example/v1".to_string());

        let targets = provider_probe_targets("codex", &runtime.codex, Some("relay"), Some("fast"));

        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(target.provider_id, "relay");
        assert_eq!(target.endpoint_id, "fast");
        assert_eq!(target.base_url, "https://relay.example/v1");
        assert_eq!(target.priority, 3);
        assert_eq!(target.auth.auth_token.as_deref(), Some("test-token"));
        assert_eq!(target.auth.api_key.as_deref(), Some("endpoint-api-key"));
        assert_eq!(
            target.tags.get("region").map(String::as_str),
            Some("provider-region")
        );
        assert_eq!(
            target.tags.get("shared").map(String::as_str),
            Some("endpoint")
        );
        assert_eq!(target.tags.get("zone").map(String::as_str), Some("edge"));
        assert_eq!(target.supported_models.get("gpt-5.5"), Some(&true));
        assert_eq!(target.supported_models.get("legacy"), Some(&false));
        assert_eq!(target.supported_models.get("endpoint-only"), Some(&true));
        assert_eq!(
            target.model_mapping.get("gpt-5.5").map(String::as_str),
            Some("upstream-gpt-5.5")
        );
        assert_eq!(
            target.model_mapping.get("legacy").map(String::as_str),
            Some("provider-legacy")
        );
    }

    #[test]
    fn service_status_cache_key_tracks_canonical_endpoint_fields() {
        let config = ServiceStatusConfig {
            enabled: true,
            probes: vec![ServiceStatusProbeConfig {
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };
        let mut runtime = canonical_runtime_with_endpoint("https://relay.example/v1".to_string());
        let initial_key = service_status_cache_key(&config, Some(&runtime), "codex");

        runtime
            .codex
            .providers
            .get_mut("relay")
            .and_then(|provider| provider.endpoints.get_mut("fast"))
            .expect("canonical endpoint")
            .tags
            .insert("region".to_string(), "endpoint-region".to_string());

        let endpoint_tag_key = service_status_cache_key(&config, Some(&runtime), "codex");
        assert_ne!(initial_key, endpoint_tag_key);

        runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider")
            .inline_auth
            .allow_anonymous = Some(true);

        assert_ne!(
            endpoint_tag_key,
            service_status_cache_key(&config, Some(&runtime), "codex")
        );
    }

    #[tokio::test]
    async fn provider_probe_sends_minimal_chat_completion_request() {
        async fn handler(
            State(captured): State<Arc<Mutex<Option<CapturedProviderProbeRequest>>>>,
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            let authorization = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let api_key = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            *captured.lock().expect("captured request lock") = Some(CapturedProviderProbeRequest {
                body,
                authorization,
                api_key,
            });
            Json(serde_json::json!({
                "id": "chatcmpl-probe",
                "object": "chat.completion",
                "choices": [
                    { "message": { "role": "assistant", "content": "ok" } }
                ]
            }))
        }

        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/v1/chat/completions", post(handler))
            .with_state(captured.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("set nonblocking");
        let addr = listener.local_addr().expect("local addr");
        let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve provider probe test");
        });

        let runtime = canonical_runtime_with_endpoint(format!("http://{addr}/v1"));
        let config = ServiceStatusConfig {
            enabled: true,
            refresh_interval_secs: 60,
            timeout_ms: 3_000,
            high_latency_ms: 3_000,
            history_cells: 4,
            probes: vec![ServiceStatusProbeConfig {
                id: Some("relay".to_string()),
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                url: None,
                models: vec!["gpt-5.5".to_string()],
                timeout_ms: None,
                high_latency_ms: None,
                headers: Default::default(),
            }],
        };

        let snapshot = fetch_service_status_snapshot(&config, Some(&runtime), "codex").await;
        server.abort();

        assert_eq!(snapshot.probes.len(), 1);
        let probe = &snapshot.probes[0];
        assert_eq!(probe.error, None);
        assert_eq!(probe.all_ok, Some(true));
        assert_eq!(probe.services.len(), 1);
        assert_eq!(probe.services[0].model, "gpt-5.5");
        assert_eq!(probe.services[0].latest_kind, ServiceStatusKind::Ok);
        assert_eq!(probe.services[0].history.len(), 4);

        let captured = captured
            .lock()
            .expect("captured request lock")
            .clone()
            .expect("captured provider probe request");
        assert_eq!(captured.authorization.as_deref(), Some("Bearer test-token"));
        assert_eq!(captured.api_key.as_deref(), Some("endpoint-api-key"));
        assert_eq!(captured.body["model"], "upstream-gpt-5.5");
        assert_eq!(captured.body["max_tokens"], 1);
        assert_eq!(captured.body["temperature"], 0);
        assert_eq!(captured.body["stream"], false);
        assert_eq!(captured.body["messages"][0]["role"], "user");
        assert_eq!(captured.body["messages"][0]["content"], "ping");
    }

    #[tokio::test]
    async fn provider_probe_remote_target_requires_auth_or_anonymous_opt_in() {
        let hits = Arc::new(Mutex::new(0usize));
        let hits_for_route = hits.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let hits = hits_for_route.clone();
                async move {
                    *hits.lock().expect("provider probe hits lock") += 1;
                    Json(serde_json::json!({
                        "id": "chatcmpl-probe",
                        "object": "chat.completion",
                        "choices": [
                            { "message": { "role": "assistant", "content": "ok" } }
                        ]
                    }))
                }
            }),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("set nonblocking");
        let addr = listener.local_addr().expect("local addr");
        let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve provider probe test");
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve("relay.example", addr)
            .build()
            .expect("build provider probe client");
        let mut runtime =
            canonical_runtime_with_endpoint(format!("http://relay.example:{}/v1", addr.port()));
        let provider = runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider");
        provider.auth = UpstreamAuth::default();
        provider.inline_auth = UpstreamAuth::default();
        let mut target =
            provider_probe_targets("codex", &runtime.codex, Some("relay"), Some("fast"))
                .into_iter()
                .next()
                .expect("provider probe target");
        let url = chat_completions_url(&target.base_url);

        let error = send_provider_probe_request(&client, &target, &url, "gpt-5.5", 3_000)
            .await
            .expect_err("remote target must fail closed without helper auth");

        assert_eq!(*hits.lock().expect("provider probe hits lock"), 0);
        assert_eq!(error.to_string(), UPSTREAM_AUTH_UNAVAILABLE_REASON);

        target.auth.allow_anonymous = Some(true);
        send_provider_probe_request(&client, &target, &url, "gpt-5.5", 3_000)
            .await
            .expect("explicit anonymous opt-in should allow the probe");

        assert_eq!(*hits.lock().expect("provider probe hits lock"), 1);

        server.abort();
    }
}
