use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::{
    ProxyConfig, ServiceConfigManager, ServiceStatusConfig, ServiceStatusProbeConfig, UpstreamAuth,
};

static SERVICE_STATUS_CACHE: OnceLock<Mutex<ServiceStatusCache>> = OnceLock::new();

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
    provider_id: String,
    endpoint_id: String,
    base_url: String,
    auth: UpstreamAuth,
    supported_models: HashMap<String, bool>,
    model_mapping: HashMap<String, String>,
}

pub async fn refresh_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_config: Option<&ProxyConfig>,
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
    runtime_config: Option<&ProxyConfig>,
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
    if let Some(runtime_config) = runtime_config {
        if let Some(mgr) = service_manager(runtime_config, service_name) {
            let mut providers = mgr
                .stations()
                .values()
                .flat_map(|station| station.upstreams.iter())
                .filter_map(|upstream| {
                    let provider_id = upstream.tags.get("provider_id")?;
                    let endpoint_id = upstream
                        .tags
                        .get("endpoint_id")
                        .map(String::as_str)
                        .unwrap_or("default");
                    Some((provider_id, endpoint_id, upstream))
                })
                .collect::<Vec<_>>();
            providers.sort_by(|left, right| {
                left.0
                    .cmp(right.0)
                    .then_with(|| left.1.cmp(right.1))
                    .then_with(|| left.2.base_url.cmp(&right.2.base_url))
            });
            for (provider_id, endpoint_id, upstream) in providers {
                provider_id.hash(&mut hasher);
                endpoint_id.hash(&mut hasher);
                upstream.base_url.hash(&mut hasher);
                let mut supported_models = upstream.supported_models.iter().collect::<Vec<_>>();
                supported_models.sort_by(|left, right| left.0.cmp(right.0));
                supported_models.hash(&mut hasher);
                let mut model_mapping = upstream.model_mapping.iter().collect::<Vec<_>>();
                model_mapping.sort_by(|left, right| left.0.cmp(right.0));
                model_mapping.hash(&mut hasher);
                upstream.auth.auth_token.hash(&mut hasher);
                upstream.auth.auth_token_env.hash(&mut hasher);
                upstream.auth.api_key.hash(&mut hasher);
                upstream.auth.api_key_env.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

async fn fetch_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_config: Option<&ProxyConfig>,
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
    runtime_config: Option<&ProxyConfig>,
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
    runtime_config: Option<&ProxyConfig>,
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
    runtime_config: Option<&ProxyConfig>,
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

    if let Some(token) = target.auth.resolve_auth_token() {
        request = request.bearer_auth(token);
    }
    if let Some(key) = target.auth.resolve_api_key() {
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
    runtime_config: Option<&ProxyConfig>,
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
    let cfg = runtime_config.context("provider probes require runtime config")?;
    let mgr = service_manager(cfg, service_name)
        .with_context(|| format!("unknown service for provider probe: {service_name}"))?;

    provider_probe_targets(mgr, provider, endpoint)
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
    mgr: &ServiceConfigManager,
    provider: &str,
    endpoint: Option<&str>,
) -> Vec<ProviderProbeTarget> {
    let mut targets = Vec::new();
    for station in mgr.stations().values() {
        for upstream in &station.upstreams {
            let upstream_provider = upstream
                .tags
                .get("provider_id")
                .map(String::as_str)
                .unwrap_or(station.name.as_str());
            if upstream_provider != provider {
                continue;
            }
            let upstream_endpoint = upstream
                .tags
                .get("endpoint_id")
                .map(String::as_str)
                .unwrap_or("default");
            if endpoint.is_some_and(|endpoint| endpoint != upstream_endpoint) {
                continue;
            }
            targets.push(ProviderProbeTarget {
                provider_id: upstream_provider.to_string(),
                endpoint_id: upstream_endpoint.to_string(),
                base_url: upstream.base_url.clone(),
                auth: upstream.auth.clone(),
                supported_models: upstream.supported_models.clone(),
                model_mapping: upstream.model_mapping.clone(),
            });
        }
    }
    targets.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    targets
}

fn provider_probe_models(
    probe: &ServiceStatusProbeConfig,
    target: &ProviderProbeTarget,
) -> Vec<String> {
    let mut models = if probe.models.is_empty() {
        target
            .supported_models
            .iter()
            .filter_map(|(model, supported)| {
                (*supported && !model.contains('*')).then(|| model.clone())
            })
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

fn service_manager<'a>(
    cfg: &'a ProxyConfig,
    service_name: &str,
) -> Option<&'a ServiceConfigManager> {
    match service_name {
        "claude" => Some(&cfg.claude),
        "codex" => Some(&cfg.codex),
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

    use crate::config::{ServiceConfig, UpstreamConfig};

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

        let mut runtime = ProxyConfig::default();
        runtime.codex.configs.insert(
            "relay".to_string(),
            ServiceConfig {
                name: "relay".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: format!("http://{addr}/v1"),
                    auth: UpstreamAuth {
                        auth_token: Some("secret-token".to_string()),
                        auth_token_env: None,
                        api_key: Some("api-key".to_string()),
                        api_key_env: None,
                    },
                    tags: HashMap::from([
                        ("provider_id".to_string(), "relay".to_string()),
                        ("endpoint_id".to_string(), "default".to_string()),
                    ]),
                    supported_models: HashMap::from([("gpt-5.5".to_string(), true)]),
                    model_mapping: HashMap::from([(
                        "gpt-5.5".to_string(),
                        "upstream-gpt-5.5".to_string(),
                    )]),
                }],
            },
        );
        let config = ServiceStatusConfig {
            enabled: true,
            refresh_interval_secs: 60,
            timeout_ms: 3_000,
            high_latency_ms: 3_000,
            history_cells: 4,
            probes: vec![ServiceStatusProbeConfig {
                id: Some("relay".to_string()),
                provider: Some("relay".to_string()),
                endpoint: Some("default".to_string()),
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
        assert_eq!(
            captured.authorization.as_deref(),
            Some("Bearer secret-token")
        );
        assert_eq!(captured.api_key.as_deref(), Some("api-key"));
        assert_eq!(captured.body["model"], "upstream-gpt-5.5");
        assert_eq!(captured.body["max_tokens"], 1);
        assert_eq!(captured.body["temperature"], 0);
        assert_eq!(captured.body["stream"], false);
        assert_eq!(captured.body["messages"][0]["role"], "user");
        assert_eq!(captured.body["messages"][0]["content"], "ping");
    }
}
