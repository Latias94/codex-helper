use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::StreamExt;
#[cfg(test)]
use futures_util::stream::FuturesUnordered;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::auth_resolution::target_credential_readiness;
use crate::config::{ServiceStatusConfig, ServiceStatusProbeConfig};
use crate::credentials::{
    CapturedUpstreamCredential, CredentialReadinessCode, CredentialReadinessDetail,
};
use crate::routing_ir::CompiledRouteGraph;

static SERVICE_STATUS_CACHE: OnceLock<Mutex<ServiceStatusCache>> = OnceLock::new();
const UPSTREAM_AUTH_UNAVAILABLE_REASON: &str = "configured upstream credentials are unavailable";
const SERVICE_STATUS_CLIENT_SETUP_FAILURE_REASON: &str = "service status client setup failed";
const SERVICE_STATUS_PROBE_FAILURE_REASON: &str = "service status probe failed";
const UPSTREAM_STATUS_FAILURE_REASON: &str = "upstream status reported failure";
const MAX_SERVICE_STATUS_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SERVICE_STATUS_PROBES: usize = 256;
const MAX_SERVICE_STATUS_MODELS_PER_PROBE: usize = 256;
const MAX_SERVICE_STATUS_HISTORY_CELLS: usize = 240;
const MAX_SERVICE_STATUS_LABEL_CHARS: usize = 256;
const MAX_CACHED_SERVICE_STATUS_PROBES: usize = MAX_SERVICE_STATUS_PROBES * 4;

#[derive(Debug, Default)]
struct ServiceStatusCache {
    probes: HashMap<(u64, u64), CachedServiceStatusProbe>,
    in_flight: HashSet<(u64, u64)>,
}

impl ServiceStatusCache {
    fn store(&mut self, key: (u64, u64), facts: ServiceStatusProbeSnapshot) {
        while self.probes.len() >= MAX_CACHED_SERVICE_STATUS_PROBES
            && !self.probes.contains_key(&key)
        {
            let Some(oldest_key) = self
                .probes
                .iter()
                .min_by_key(|(_, cached)| cached.last_refresh)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.probes.remove(&oldest_key);
        }
        self.probes.insert(
            key,
            CachedServiceStatusProbe {
                last_refresh: Instant::now(),
                facts,
            },
        );
    }
}

#[derive(Debug)]
struct CachedServiceStatusProbe {
    last_refresh: Instant,
    facts: ServiceStatusProbeSnapshot,
}

struct ServiceStatusRefreshClaim {
    cache_key: u64,
    execution_key: u64,
}

impl Drop for ServiceStatusRefreshClaim {
    fn drop(&mut self) {
        let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
        if let Ok(mut guard) = cache.lock() {
            guard
                .in_flight
                .remove(&(self.cache_key, self.execution_key));
        }
    }
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
            history_cells: effective_service_status_history_cells(config.history_cells),
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
    pub credential_readiness: Option<CredentialReadinessCode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_details: Vec<CredentialReadinessDetail>,
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

#[derive(Debug)]
enum ProviderProbeFailure {
    CredentialUnavailable,
    Transport,
    Http(reqwest::StatusCode),
}

impl std::fmt::Display for ProviderProbeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CredentialUnavailable => formatter.write_str(UPSTREAM_AUTH_UNAVAILABLE_REASON),
            Self::Transport => formatter.write_str("provider probe request failed"),
            Self::Http(status) => write!(formatter, "provider probe HTTP {status}"),
        }
    }
}

impl std::error::Error for ProviderProbeFailure {}

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
    stable_index: usize,
    route_scope: String,
    credential: CapturedUpstreamCredential,
    tags: HashMap<String, String>,
    supported_models: HashMap<String, bool>,
    model_mapping: HashMap<String, String>,
}

pub(crate) fn project_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
) -> ServiceStatusSnapshot {
    if !config.is_active() {
        return ServiceStatusSnapshot::disabled(config);
    }

    let interval = Duration::from_secs(config.refresh_interval_secs.max(1));
    let configured_probe_count = active_service_status_probe_count(config);
    let mut probes = Vec::new();
    for (index, probe) in config
        .probes
        .iter()
        .enumerate()
        .filter(|(_, probe)| probe_has_target(probe))
        .take(MAX_SERVICE_STATUS_PROBES)
    {
        let cache_key = service_status_probe_cache_key(service_name, index, probe);
        let target = if has_provider_probe_target(probe) {
            provider_probe_target(runtime_route, service_name, probe).ok()
        } else {
            None
        };
        let execution_key = service_status_probe_execution_key(
            config,
            runtime_route,
            service_name,
            probe,
            target.as_ref(),
        );
        let fresh = cached_service_status_probe(cache_key, execution_key, Some(interval));
        let cached = fresh
            .clone()
            .or_else(|| cached_service_status_probe(cache_key, execution_key, None));
        let mut snapshot =
            cached.unwrap_or_else(|| pending_probe_snapshot(config, probe, target.as_ref()));

        let blocked = target.as_ref().is_some_and(|target| {
            let (readiness, details) = provider_probe_credential_readiness(target);
            apply_provider_readiness_overlay(&mut snapshot, readiness, details);
            !readiness.is_routable()
        });
        if fresh.is_none() && !blocked {
            schedule_service_status_probe_refresh(
                config,
                runtime_route,
                service_name,
                index,
                probe,
                cache_key,
                execution_key,
                interval,
            );
        }
        probes.push(snapshot);
    }
    probes.sort_by(|left, right| left.id.cmp(&right.id));

    ServiceStatusSnapshot {
        generated_at_ms: unix_now_ms(),
        configured: true,
        enabled: config.enabled,
        refresh_interval_secs: config.refresh_interval_secs,
        history_cells: effective_service_status_history_cells(config.history_cells),
        probes,
        error: probe_limit_error(configured_probe_count),
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_service_status_probe_refresh(
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    index: usize,
    probe: &ServiceStatusProbeConfig,
    cache_key: u64,
    execution_key: u64,
    interval: Duration,
) {
    let Some(claim) = claim_service_status_probe_refresh(cache_key, execution_key) else {
        return;
    };
    let config = config.clone();
    let runtime_route = runtime_route.cloned();
    let service_name = service_name.to_string();
    let probe = probe.clone();
    tokio::spawn(async move {
        let _claim = claim;
        let client = match service_status_client(&config) {
            Ok(client) => client,
            Err(_) => {
                let target = if has_provider_probe_target(&probe) {
                    provider_probe_target(runtime_route.as_ref(), &service_name, &probe).ok()
                } else {
                    None
                };
                let facts = client_setup_failure_probe_snapshot(&config, &probe, target.as_ref());
                store_service_status_probe(cache_key, execution_key, facts);
                return;
            }
        };
        refresh_probe_snapshot(
            &client,
            &config,
            runtime_route.as_ref(),
            &service_name,
            index,
            &probe,
            interval,
        )
        .await;
    });
}

fn claim_service_status_probe_refresh(
    cache_key: u64,
    execution_key: u64,
) -> Option<ServiceStatusRefreshClaim> {
    let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
    let mut guard = cache.lock().ok()?;
    if !guard.in_flight.insert((cache_key, execution_key)) {
        return None;
    }
    Some(ServiceStatusRefreshClaim {
        cache_key,
        execution_key,
    })
}

#[cfg(test)]
async fn refresh_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
) -> ServiceStatusSnapshot {
    if !config.is_active() {
        return ServiceStatusSnapshot::disabled(config);
    }

    let interval = Duration::from_secs(config.refresh_interval_secs.max(1));
    let client = match service_status_client(config) {
        Ok(client) => client,
        Err(err) => return service_status_client_error(config, err),
    };
    let configured_probe_count = active_service_status_probe_count(config);
    let mut futures = config
        .probes
        .iter()
        .enumerate()
        .filter(|(_, probe)| probe_has_target(probe))
        .take(MAX_SERVICE_STATUS_PROBES)
        .map(|(index, probe)| {
            refresh_probe_snapshot(
                &client,
                config,
                runtime_route,
                service_name,
                index,
                probe,
                interval,
            )
        })
        .collect::<FuturesUnordered<_>>();
    let mut probes = Vec::new();
    while let Some(probe) = futures.next().await {
        probes.push(probe);
    }
    probes.sort_by(|left, right| left.id.cmp(&right.id));
    ServiceStatusSnapshot {
        generated_at_ms: unix_now_ms(),
        configured: true,
        enabled: config.enabled,
        refresh_interval_secs: config.refresh_interval_secs,
        history_cells: effective_service_status_history_cells(config.history_cells),
        probes,
        error: probe_limit_error(configured_probe_count),
    }
}

async fn refresh_probe_snapshot(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    index: usize,
    probe: &ServiceStatusProbeConfig,
    interval: Duration,
) -> ServiceStatusProbeSnapshot {
    let cache_key = service_status_probe_cache_key(service_name, index, probe);
    let target = if has_provider_probe_target(probe) {
        provider_probe_target(runtime_route, service_name, probe).ok()
    } else {
        None
    };
    let execution_key = service_status_probe_execution_key(
        config,
        runtime_route,
        service_name,
        probe,
        target.as_ref(),
    );

    if let Some(target) = target.as_ref() {
        let (readiness, details) = provider_probe_credential_readiness(target);
        if !readiness.is_routable() {
            let mut snapshot = cached_service_status_probe(cache_key, execution_key, None)
                .unwrap_or_else(|| blocked_provider_probe_snapshot(config, probe, target));
            apply_provider_readiness_overlay(&mut snapshot, readiness, details);
            return snapshot;
        }
    }

    if let Some(mut snapshot) =
        cached_service_status_probe(cache_key, execution_key, Some(interval))
    {
        if let Some(target) = target.as_ref() {
            let (readiness, details) = provider_probe_credential_readiness(target);
            apply_provider_readiness_overlay(&mut snapshot, readiness, details);
        }
        return snapshot;
    }

    let mut snapshot = fetch_probe(client, config, runtime_route, service_name, probe).await;
    if let Some(target) = target.as_ref() {
        let (readiness, details) = provider_probe_credential_readiness(target);
        apply_provider_readiness_overlay(&mut snapshot, readiness, details);
    }
    let mut facts = snapshot.clone();
    facts.credential_readiness = None;
    facts.credential_details.clear();
    store_service_status_probe(cache_key, execution_key, facts);
    snapshot
}

fn cached_service_status_probe(
    cache_key: u64,
    execution_key: u64,
    max_age: Option<Duration>,
) -> Option<ServiceStatusProbeSnapshot> {
    let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
    let guard = cache.lock().ok()?;
    let cached = guard.probes.get(&(cache_key, execution_key))?;
    if max_age.is_some_and(|max_age| cached.last_refresh.elapsed() >= max_age) {
        return None;
    }
    Some(cached.facts.clone())
}

fn store_service_status_probe(
    cache_key: u64,
    execution_key: u64,
    facts: ServiceStatusProbeSnapshot,
) {
    let cache = SERVICE_STATUS_CACHE.get_or_init(|| Mutex::new(ServiceStatusCache::default()));
    if let Ok(mut guard) = cache.lock() {
        guard.store((cache_key, execution_key), facts);
    }
}

fn active_service_status_probe_count(config: &ServiceStatusConfig) -> usize {
    config
        .probes
        .iter()
        .filter(|probe| probe_has_target(probe))
        .count()
}

fn probe_limit_error(configured_probe_count: usize) -> Option<String> {
    (configured_probe_count > MAX_SERVICE_STATUS_PROBES).then(|| {
        format!(
            "service status probe limit exceeded: configured={configured_probe_count} limit={MAX_SERVICE_STATUS_PROBES}"
        )
    })
}

fn effective_service_status_history_cells(configured: usize) -> usize {
    configured.clamp(1, MAX_SERVICE_STATUS_HISTORY_CELLS)
}

fn service_status_probe_failure_reason(_error: &anyhow::Error) -> &'static str {
    SERVICE_STATUS_PROBE_FAILURE_REASON
}

fn service_status_probe_cache_key(
    service_name: &str,
    index: usize,
    probe: &ServiceStatusProbeConfig,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    service_name.hash(&mut hasher);
    index.hash(&mut hasher);
    probe.id.hash(&mut hasher);
    probe.provider.hash(&mut hasher);
    probe.endpoint.hash(&mut hasher);
    probe.url.hash(&mut hasher);
    hasher.finish()
}

fn service_status_probe_execution_key(
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
    target: Option<&ProviderProbeTarget>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    service_name.hash(&mut hasher);
    config.timeout_ms.hash(&mut hasher);
    config.high_latency_ms.hash(&mut hasher);
    effective_service_status_history_cells(config.history_cells).hash(&mut hasher);
    probe.id.hash(&mut hasher);
    probe.provider.hash(&mut hasher);
    probe.endpoint.hash(&mut hasher);
    probe.url.hash(&mut hasher);
    probe.models.hash(&mut hasher);
    probe.timeout_ms.hash(&mut hasher);
    probe.high_latency_ms.hash(&mut hasher);
    probe.headers.hash(&mut hasher);
    if let Some(target) = target {
        hash_provider_probe_target(target, &mut hasher);
    } else if has_provider_probe_target(probe)
        && let Some(runtime_route) = runtime_route
    {
        runtime_route.digest().hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_provider_probe_target(target: &ProviderProbeTarget, hasher: &mut impl Hasher) {
    target.service_name.hash(hasher);
    target.provider_id.hash(hasher);
    target.endpoint_id.hash(hasher);
    target.base_url.hash(hasher);
    target.stable_index.hash(hasher);
    target.route_scope.hash(hasher);
    target.credential.configured_contract().hash(hasher);
    target.credential.allow_anonymous().hash(hasher);

    let mut tags = target.tags.iter().collect::<Vec<_>>();
    tags.sort_by(|left, right| left.0.cmp(right.0));
    tags.hash(hasher);
    let mut supported_models = target.supported_models.iter().collect::<Vec<_>>();
    supported_models.sort_by(|left, right| left.0.cmp(right.0));
    supported_models.hash(hasher);
    let mut model_mapping = target.model_mapping.iter().collect::<Vec<_>>();
    model_mapping.sort_by(|left, right| left.0.cmp(right.0));
    model_mapping.hash(hasher);
}

fn service_status_client(config: &ServiceStatusConfig) -> Result<Client> {
    let builder = Client::builder()
        .timeout(Duration::from_millis(config.timeout_ms.max(1)))
        .connect_timeout(Duration::from_millis(config.timeout_ms.max(1)))
        .redirect(reqwest::redirect::Policy::none());
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build().context("build service status client")
}

#[cfg(test)]
fn service_status_client_error(
    config: &ServiceStatusConfig,
    _error: anyhow::Error,
) -> ServiceStatusSnapshot {
    ServiceStatusSnapshot {
        generated_at_ms: unix_now_ms(),
        configured: config.is_active(),
        enabled: config.enabled,
        refresh_interval_secs: config.refresh_interval_secs,
        history_cells: effective_service_status_history_cells(config.history_cells),
        probes: Vec::new(),
        error: Some(SERVICE_STATUS_CLIENT_SETUP_FAILURE_REASON.to_string()),
    }
}

#[cfg(test)]
async fn fetch_service_status_snapshot(
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
) -> ServiceStatusSnapshot {
    let client = match service_status_client(config) {
        Ok(client) => client,
        Err(err) => return service_status_client_error(config, err),
    };

    let configured_probe_count = active_service_status_probe_count(config);
    let mut futures = config
        .probes
        .iter()
        .filter(|probe| probe_has_target(probe))
        .take(MAX_SERVICE_STATUS_PROBES)
        .map(|probe| fetch_probe(&client, config, runtime_route, service_name, probe))
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
        history_cells: effective_service_status_history_cells(config.history_cells),
        probes,
        error: probe_limit_error(configured_probe_count),
    }
}

async fn fetch_probe(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> ServiceStatusProbeSnapshot {
    let fetched_at_ms = unix_now_ms();
    let id = probe_id(probe);
    match fetch_probe_inner(client, config, runtime_route, service_name, probe).await {
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
            services: configured_probe_models(probe)
                .iter()
                .map(|model| missing_service_row(model, config.history_cells))
                .collect(),
            credential_readiness: None,
            credential_details: Vec::new(),
            error: Some(service_status_probe_failure_reason(&err).to_string()),
        },
    }
}

async fn fetch_probe_inner(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> Result<ServiceStatusProbeSnapshot> {
    if has_provider_probe_target(probe) {
        return fetch_provider_probe(client, config, runtime_route, service_name, probe).await;
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
        .context("service status request failed")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("status API returned HTTP {}", status.as_u16());
    }
    let body = read_bounded_service_status_body(response).await?;
    snapshot_from_status_json(
        probe,
        config.history_cells,
        probe.high_latency_ms.unwrap_or(config.high_latency_ms),
        &body,
    )
    .context("decode service status response")
}

async fn read_bounded_service_status_body(response: reqwest::Response) -> Result<String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SERVICE_STATUS_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("service status response body exceeded limit");
    }

    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read service status response body")?;
        if body.len().saturating_add(chunk.len()) > MAX_SERVICE_STATUS_RESPONSE_BYTES {
            anyhow::bail!("service status response body exceeded limit");
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).context("service status response was not UTF-8")
}

async fn fetch_provider_probe(
    client: &Client,
    config: &ServiceStatusConfig,
    runtime_route: Option<&CompiledRouteGraph>,
    service_name: &str,
    probe: &ServiceStatusProbeConfig,
) -> Result<ServiceStatusProbeSnapshot> {
    let target = provider_probe_target(runtime_route, service_name, probe)?;
    let timeout_ms = probe.timeout_ms.unwrap_or(config.timeout_ms).max(1);
    let high_latency_ms = probe.high_latency_ms.unwrap_or(config.high_latency_ms);
    let models = provider_probe_models(probe, &target);
    if models.is_empty() {
        anyhow::bail!(
            "provider probe has no model; configure models for {}",
            target.provider_id
        );
    }
    let (credential_readiness, credential_details) = provider_probe_credential_readiness(&target);
    if !credential_readiness.is_routable() {
        return Ok(ServiceStatusProbeSnapshot {
            id: probe_id(probe),
            url: provider_target_label(&target),
            fetched_at_ms: 0,
            generated_at_ms: Some(unix_now_ms()),
            all_ok: None,
            services: models
                .iter()
                .map(|model| missing_service_row(model, config.history_cells))
                .collect(),
            credential_readiness: Some(credential_readiness),
            credential_details,
            error: Some(UPSTREAM_AUTH_UNAVAILABLE_REASON.to_string()),
        });
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
        credential_readiness: Some(credential_readiness),
        credential_details,
        error: None,
    })
}

fn provider_probe_credential_readiness(
    target: &ProviderProbeTarget,
) -> (CredentialReadinessCode, Vec<CredentialReadinessDetail>) {
    let code = target_credential_readiness(
        target.service_name.as_str(),
        target.credential.configured_contract(),
        target.credential.allow_anonymous(),
        target.base_url.as_str(),
        target.credential.readiness_code(),
    );
    let mut details = target.credential.readiness_details();
    if details.is_empty() && code == CredentialReadinessCode::Missing {
        details.push(CredentialReadinessDetail {
            kind: None,
            code,
            stale_cause: None,
            source_kind: Some("configuration".to_string()),
            reference: None,
        });
    }
    (code, details)
}

fn blocked_provider_probe_snapshot(
    config: &ServiceStatusConfig,
    probe: &ServiceStatusProbeConfig,
    target: &ProviderProbeTarget,
) -> ServiceStatusProbeSnapshot {
    ServiceStatusProbeSnapshot {
        id: probe_id(probe),
        url: provider_target_label(target),
        fetched_at_ms: 0,
        generated_at_ms: None,
        all_ok: None,
        services: provider_probe_models(probe, target)
            .iter()
            .map(|model| missing_service_row(model, config.history_cells))
            .collect(),
        credential_readiness: None,
        credential_details: Vec::new(),
        error: None,
    }
}

fn pending_probe_snapshot(
    config: &ServiceStatusConfig,
    probe: &ServiceStatusProbeConfig,
    target: Option<&ProviderProbeTarget>,
) -> ServiceStatusProbeSnapshot {
    let models = target
        .map(|target| provider_probe_models(probe, target))
        .unwrap_or_else(|| configured_probe_models(probe));
    ServiceStatusProbeSnapshot {
        id: probe_id(probe),
        url: target
            .map(provider_target_label)
            .unwrap_or_else(|| probe_target_label(probe)),
        fetched_at_ms: 0,
        generated_at_ms: None,
        all_ok: None,
        services: models
            .iter()
            .map(|model| missing_service_row(model, config.history_cells))
            .collect(),
        credential_readiness: None,
        credential_details: Vec::new(),
        error: None,
    }
}

fn client_setup_failure_probe_snapshot(
    config: &ServiceStatusConfig,
    probe: &ServiceStatusProbeConfig,
    target: Option<&ProviderProbeTarget>,
) -> ServiceStatusProbeSnapshot {
    let mut snapshot = pending_probe_snapshot(config, probe, target);
    snapshot.fetched_at_ms = unix_now_ms();
    snapshot.error = Some(SERVICE_STATUS_CLIENT_SETUP_FAILURE_REASON.to_string());
    snapshot
}

fn apply_provider_readiness_overlay(
    snapshot: &mut ServiceStatusProbeSnapshot,
    readiness: CredentialReadinessCode,
    details: Vec<CredentialReadinessDetail>,
) {
    snapshot.credential_readiness = Some(readiness);
    snapshot.credential_details = details;
    if readiness.is_routable() {
        return;
    }
    snapshot.all_ok = None;
    snapshot.error = Some(UPSTREAM_AUTH_UNAVAILABLE_REASON.to_string());
    for service in &mut snapshot.services {
        service.latest_kind = ServiceStatusKind::Unknown;
        service.latest = None;
    }
}

async fn fetch_provider_model_probe(
    client: &Client,
    target: &ProviderProbeTarget,
    requested_model: &str,
    timeout_ms: u64,
    high_latency_ms: u64,
    history_cells: usize,
) -> ServiceStatusServiceSnapshot {
    let history_cells = effective_service_status_history_cells(history_cells);
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
) -> std::result::Result<(), ProviderProbeFailure> {
    if !provider_probe_credential_readiness(target).0.is_routable() {
        return Err(ProviderProbeFailure::CredentialUnavailable);
    }
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

    if let Some(token) = target.credential.bearer_header() {
        request = request.header("authorization", token);
    }
    if let Some(key) = target.credential.api_key_header() {
        request = request.header("x-api-key", key);
    }

    let response = request
        .send()
        .await
        .map_err(|_| ProviderProbeFailure::Transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProviderProbeFailure::Http(status));
    }
    Ok(())
}

fn provider_probe_target(
    runtime_route: Option<&CompiledRouteGraph>,
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
    let runtime_route =
        runtime_route.context("provider probes require a captured runtime route")?;
    if runtime_route.service_name() != service_name {
        anyhow::bail!("unknown service for provider probe: {service_name}");
    }

    provider_probe_targets(runtime_route, Some(provider), endpoint)?
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
    runtime_route: &CompiledRouteGraph,
    provider_filter: Option<&str>,
    endpoint: Option<&str>,
) -> Result<Vec<ProviderProbeTarget>> {
    let template = runtime_route.handshake_plan();
    let mut targets = Vec::new();
    for candidate in &template.candidates {
        if provider_filter.is_some_and(|filter| filter != candidate.provider_id.as_str())
            || endpoint.is_some_and(|filter| filter != candidate.endpoint_id.as_str())
        {
            continue;
        }
        let captured = template.capture_candidate(candidate)?;
        targets.push(ProviderProbeTarget {
            service_name: runtime_route.service_name().to_string(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            base_url: candidate.base_url.clone(),
            stable_index: candidate.stable_index,
            route_scope: captured.runtime_identity().policy_route_scope(),
            credential: captured.credential().clone(),
            tags: candidate.tags.clone().into_iter().collect(),
            supported_models: candidate.supported_models.clone().into_iter().collect(),
            model_mapping: candidate.model_mapping.clone().into_iter().collect(),
        });
    }
    targets.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.stable_index.cmp(&right.stable_index))
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    Ok(targets)
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
    models = models
        .into_iter()
        .map(|model| safe_service_status_label(&model))
        .filter(|model| !model.is_empty())
        .collect();
    models.sort();
    models.dedup();
    models.truncate(MAX_SERVICE_STATUS_MODELS_PER_PROBE);
    models
}

fn configured_probe_models(probe: &ServiceStatusProbeConfig) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for model in &probe.models {
        let model = safe_service_status_label(model);
        if !model.is_empty() && seen.insert(model.clone()) {
            models.push(model);
        }
        if models.len() == MAX_SERVICE_STATUS_MODELS_PER_PROBE {
            break;
        }
    }
    models
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
    let origin = crate::logging::upstream_origin(&target.base_url)
        .unwrap_or_else(|| "invalid-origin".to_string());
    safe_service_status_label(&format!(
        "{}/{} {}",
        safe_service_status_label(&target.provider_id),
        safe_service_status_label(&target.endpoint_id),
        origin
    ))
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
            Some(endpoint) => safe_service_status_label(&format!(
                "{}/{}",
                safe_service_status_label(provider),
                safe_service_status_label(endpoint)
            )),
            None => safe_service_status_label(provider),
        };
    }
    probe
        .url
        .as_deref()
        .and_then(crate::logging::upstream_origin)
        .unwrap_or_else(|| "invalid service status URL".to_string())
}

fn snapshot_from_status_json(
    probe: &ServiceStatusProbeConfig,
    history_cells: usize,
    high_latency_ms: u64,
    body: &str,
) -> Result<ServiceStatusProbeSnapshot> {
    let history_cells = effective_service_status_history_cells(history_cells);
    let raw: RawServiceStatusResponse = serde_json::from_str(body)?;
    let services_by_model = raw
        .services
        .into_iter()
        .take(MAX_SERVICE_STATUS_MODELS_PER_PROBE)
        .filter_map(|mut service| {
            let model = safe_service_status_label(&service.model);
            if model.is_empty() {
                return None;
            }
            service.model = model.clone();
            Some((model, service))
        })
        .collect::<HashMap<_, _>>();
    let models = if probe.models.is_empty() {
        let mut models = services_by_model.keys().cloned().collect::<Vec<_>>();
        models.sort();
        models.truncate(MAX_SERVICE_STATUS_MODELS_PER_PROBE);
        models
    } else {
        configured_probe_models(probe)
    };

    let services = models
        .into_iter()
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
        credential_readiness: None,
        credential_details: Vec::new(),
        error: None,
    })
}

fn service_snapshot(
    raw: &RawServiceStatusService,
    history_cells: usize,
    high_latency_ms: u64,
) -> ServiceStatusServiceSnapshot {
    let history_cells = effective_service_status_history_cells(history_cells);
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
        model: safe_service_status_label(&raw.model),
        uptime_pct: raw.uptime_pct.as_ref().and_then(decimal_string_from_json),
        latest_kind,
        latest,
        history: cells,
    }
}

fn missing_service_row(model: &str, history_cells: usize) -> ServiceStatusServiceSnapshot {
    let history_cells = effective_service_status_history_cells(history_cells);
    ServiceStatusServiceSnapshot {
        model: safe_service_status_label(model),
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
            .map(|_| UPSTREAM_STATUS_FAILURE_REASON.to_string()),
    }
}

fn probe_id(probe: &ServiceStatusProbeConfig) -> String {
    probe
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(safe_service_status_label)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| probe_target_label(probe))
}

fn safe_service_status_label(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_SERVICE_STATUS_LABEL_CHARS)
        .collect()
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
        serde_json::Value::Number(number) => Some(safe_service_status_label(&number.to_string())),
        serde_json::Value::String(text) => {
            let text = safe_service_status_label(text);
            (!text.is_empty()).then(|| text.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::{any, post};
    use axum::{Json, Router};

    use crate::config::{
        CredentialRef, HelperConfig, ProviderConcurrencyLimits, ProviderConfig,
        ProviderEndpointConfig, ServiceRouteConfig, UpstreamAuth,
    };
    use crate::credentials::{
        CredentialCandidateInput, CredentialRuntime, CredentialSourceCapabilities, SecretValue,
    };
    use crate::runtime_identity::ProviderEndpointKey;
    use crate::runtime_store::RuntimeStore;

    #[derive(Debug, Clone, Default)]
    struct CapturedProviderProbeRequest {
        body: serde_json::Value,
        authorization: Option<String>,
        api_key: Option<String>,
    }

    async fn spawn_server(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind service status test server");
        let address = listener.local_addr().expect("service status test address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve service status test server");
        });
        (address, handle)
    }

    struct RedirectFixture {
        source_address: SocketAddr,
        source_hits: Arc<Mutex<usize>>,
        target_headers: Arc<Mutex<Vec<HeaderMap>>>,
        source_handle: tokio::task::JoinHandle<()>,
        target_handle: tokio::task::JoinHandle<()>,
    }

    impl RedirectFixture {
        async fn spawn(status: axum::http::StatusCode) -> Self {
            let target_headers = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
            let target_headers_for_route = target_headers.clone();
            let target = Router::new().fallback(any(move |headers: HeaderMap| {
                let target_headers = target_headers_for_route.clone();
                async move {
                    target_headers
                        .lock()
                        .expect("redirect target headers")
                        .push(headers);
                    axum::http::StatusCode::OK
                }
            }));
            let (target_address, target_handle) = spawn_server(target).await;

            let source_hits = Arc::new(Mutex::new(0usize));
            let source_hits_for_route = source_hits.clone();
            let redirect_location = format!("http://{target_address}/capture");
            let source = Router::new().fallback(any(move || {
                let source_hits = source_hits_for_route.clone();
                let redirect_location = redirect_location.clone();
                async move {
                    *source_hits.lock().expect("redirect source hits") += 1;
                    (status, [(axum::http::header::LOCATION, redirect_location)])
                }
            }));
            let (source_address, source_handle) = spawn_server(source).await;

            Self {
                source_address,
                source_hits,
                target_headers,
                source_handle,
                target_handle,
            }
        }

        fn assert_not_followed(&self) {
            assert_eq!(*self.source_hits.lock().expect("redirect source hits"), 1);
            assert!(
                self.target_headers
                    .lock()
                    .expect("redirect target headers")
                    .is_empty(),
                "redirect target received a service-status request"
            );
        }
    }

    impl Drop for RedirectFixture {
        fn drop(&mut self) {
            self.source_handle.abort();
            self.target_handle.abort();
        }
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
                            auth_token: Some("test-token".to_string().into()),
                            api_key: Some("provider-api-key".to_string().into()),
                            ..UpstreamAuth::default()
                        },
                        inline_auth: UpstreamAuth {
                            api_key: Some("endpoint-api-key".to_string().into()),
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

    fn bound_route_graph(runtime: &HelperConfig, service_name: &str) -> CompiledRouteGraph {
        bind_route_graph_with_capabilities(
            runtime,
            service_name,
            CredentialSourceCapabilities::server(),
        )
    }

    fn bind_route_graph_with_capabilities(
        runtime: &HelperConfig,
        service_name: &str,
        capabilities: CredentialSourceCapabilities,
    ) -> CompiledRouteGraph {
        let service = match service_name {
            "codex" => &runtime.codex,
            "claude" => &runtime.claude,
            _ => panic!("unsupported test service: {service_name}"),
        };
        let graph =
            CompiledRouteGraph::compile(service_name, service).expect("compile route graph");
        let runtime_store = RuntimeStore::open_in_memory().expect("open credential runtime store");
        let credential_runtime =
            CredentialRuntime::from_runtime_store(capabilities, &runtime_store)
                .expect("build credential runtime");
        let generation =
            credential_runtime
                .build_generation(graph.candidates().iter().map(|candidate| {
                    CredentialCandidateInput {
                        provider_endpoint: ProviderEndpointKey::new(
                            service_name,
                            candidate.provider_id.clone(),
                            candidate.endpoint_id.clone(),
                        ),
                        auth: &candidate.auth,
                    }
                }))
                .expect("build credential generation");
        let digest = graph.digest().to_string();
        graph
            .with_credential_generation(generation, digest)
            .expect("bind credential generation")
    }

    fn rebound_route_graph_with_runtime(
        graph: &CompiledRouteGraph,
        credential_runtime: &CredentialRuntime,
        service_name: &str,
    ) -> CompiledRouteGraph {
        let generation =
            credential_runtime
                .build_generation(graph.candidates().iter().map(|candidate| {
                    CredentialCandidateInput {
                        provider_endpoint: ProviderEndpointKey::new(
                            service_name,
                            candidate.provider_id.clone(),
                            candidate.endpoint_id.clone(),
                        ),
                        auth: &candidate.auth,
                    }
                }))
                .expect("build credential generation");
        graph
            .rebound_credential_generation(generation)
            .expect("rebind credential generation")
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
        assert_eq!(
            snapshot.services[0].history[3]
                .probe
                .as_ref()
                .and_then(|sample| sample.error.as_deref()),
            Some(UPSTREAM_STATUS_FAILURE_REASON)
        );
    }

    #[test]
    fn custom_status_snapshot_redacts_url_secrets_and_upstream_errors() {
        let probe = ServiceStatusProbeConfig {
            url: Some(
                "https://user:password-canary@status.example.test/path?token=query-canary#fragment"
                    .to_string(),
            ),
            models: vec!["gpt-test".to_string()],
            ..ServiceStatusProbeConfig::default()
        };
        let snapshot = snapshot_from_status_json(
            &probe,
            4,
            3_000,
            r#"{
              "services": [{
                "model": "gpt-test",
                "last": { "ok": false, "error": "upstream-body-canary" }
              }]
            }"#,
        )
        .expect("decode sanitized custom status snapshot");
        let encoded = serde_json::to_string(&snapshot).expect("encode custom status snapshot");

        assert_eq!(snapshot.id, "https://status.example.test");
        assert_eq!(snapshot.url, "https://status.example.test");
        assert_eq!(
            snapshot.services[0]
                .latest
                .as_ref()
                .and_then(|sample| sample.error.as_deref()),
            Some(UPSTREAM_STATUS_FAILURE_REASON)
        );
        for canary in [
            "password-canary",
            "query-canary",
            "fragment",
            "upstream-body-canary",
        ] {
            assert!(!encoded.contains(canary), "snapshot leaked {canary}");
        }
    }

    #[tokio::test]
    async fn custom_status_rejects_chunked_body_over_limit() {
        let app = Router::new().fallback(any(|| async {
            let chunk_size = MAX_SERVICE_STATUS_RESPONSE_BYTES / 2 + 1;
            let chunks = (0..2).map(move |_| {
                Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b'x'; chunk_size]))
            });
            axum::body::Body::from_stream(futures_util::stream::iter(chunks))
        }));
        let (address, server) = spawn_server(app).await;
        let config = ServiceStatusConfig {
            enabled: true,
            probes: vec![ServiceStatusProbeConfig {
                id: Some("oversized".to_string()),
                url: Some(format!("http://{address}/status")),
                models: vec!["gpt-test".to_string()],
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };

        let snapshot = fetch_service_status_snapshot(&config, None, "codex").await;
        server.abort();

        assert_eq!(
            snapshot.probes[0].error.as_deref(),
            Some(SERVICE_STATUS_PROBE_FAILURE_REASON)
        );
        assert!(
            serde_json::to_vec(&snapshot)
                .expect("encode oversized response snapshot")
                .len()
                < 16 * 1024
        );
    }

    #[test]
    fn service_status_cache_does_not_clear_at_sixty_four_entries() {
        let mut cache = ServiceStatusCache::default();
        for index in 0..65_u64 {
            cache.store(
                (index, index),
                ServiceStatusProbeSnapshot {
                    id: format!("probe-{index}"),
                    url: "https://status.example.test".to_string(),
                    fetched_at_ms: index,
                    generated_at_ms: None,
                    all_ok: None,
                    services: Vec::new(),
                    credential_readiness: None,
                    credential_details: Vec::new(),
                    error: None,
                },
            );
        }

        assert_eq!(cache.probes.len(), 65);
    }

    #[test]
    fn service_status_projection_bounds_configured_collections() {
        let probe = probe(vec!["gpt-test"]);
        let config = ServiceStatusConfig {
            history_cells: usize::MAX,
            ..ServiceStatusConfig::default()
        };
        let client_failure = client_setup_failure_probe_snapshot(&config, &probe, None);

        assert_eq!(
            effective_service_status_history_cells(config.history_cells),
            MAX_SERVICE_STATUS_HISTORY_CELLS
        );
        assert_eq!(
            client_failure.services[0].history.len(),
            MAX_SERVICE_STATUS_HISTORY_CELLS
        );
        assert_ne!(client_failure.fetched_at_ms, 0);
        assert_eq!(
            client_failure.error.as_deref(),
            Some(SERVICE_STATUS_CLIENT_SETUP_FAILURE_REASON)
        );

        let over_limit = ServiceStatusConfig {
            probes: (0..=MAX_SERVICE_STATUS_PROBES)
                .map(|index| ServiceStatusProbeConfig {
                    url: Some(format!("https://status-{index}.example.test")),
                    ..ServiceStatusProbeConfig::default()
                })
                .collect(),
            ..ServiceStatusConfig::default()
        };
        assert_eq!(
            active_service_status_probe_count(&over_limit),
            MAX_SERVICE_STATUS_PROBES + 1
        );
        assert!(probe_limit_error(MAX_SERVICE_STATUS_PROBES + 1).is_some());
    }

    #[test]
    fn legacy_probe_json_defaults_missing_credential_readiness() {
        let snapshot: ServiceStatusProbeSnapshot = serde_json::from_str(
            r#"{
              "id": "legacy",
              "url": "https://status.example.test",
              "fetched_at_ms": 42,
              "services": []
            }"#,
        )
        .expect("decode pre-readiness service-status probe");

        assert_eq!(snapshot.credential_readiness, None);
        assert!(snapshot.credential_details.is_empty());
    }

    #[test]
    fn canonical_provider_probe_target_merges_provider_and_endpoint_fields() {
        let runtime = canonical_runtime_with_endpoint("https://relay.example/v1".to_string());
        let route = bound_route_graph(&runtime, "codex");

        let targets = provider_probe_targets(&route, Some("relay"), Some("fast"))
            .expect("capture provider targets");

        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(target.provider_id, "relay");
        assert_eq!(target.endpoint_id, "fast");
        assert_eq!(target.base_url, "https://relay.example/v1");
        assert_eq!(target.stable_index, 0);
        let mut poisoned_target = target.clone();
        poisoned_target.base_url =
            "https://user:password-canary@relay.example/v1?token=query-canary#fragment".to_string();
        assert_eq!(
            provider_target_label(&poisoned_target),
            "relay/fast https://relay.example"
        );
        assert_eq!(
            target
                .credential
                .bearer_header()
                .expect("captured bearer")
                .to_str()
                .expect("bearer text"),
            "Bearer test-token"
        );
        assert_eq!(
            target
                .credential
                .api_key_header()
                .expect("captured API key")
                .to_str()
                .expect("API key text"),
            "endpoint-api-key"
        );
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
    fn service_status_execution_key_tracks_canonical_endpoint_fields() {
        let config = ServiceStatusConfig {
            enabled: true,
            probes: vec![ServiceStatusProbeConfig {
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };
        let probe = &config.probes[0];
        let mut runtime = canonical_runtime_with_endpoint("https://relay.example/v1".to_string());
        let initial_route = bound_route_graph(&runtime, "codex");
        let initial_target = provider_probe_target(Some(&initial_route), "codex", probe)
            .expect("initial provider probe target");
        let initial_key = service_status_probe_execution_key(
            &config,
            Some(&initial_route),
            "codex",
            probe,
            Some(&initial_target),
        );

        runtime
            .codex
            .providers
            .get_mut("relay")
            .and_then(|provider| provider.endpoints.get_mut("fast"))
            .expect("canonical endpoint")
            .tags
            .insert("region".to_string(), "endpoint-region".to_string());

        let endpoint_tag_route = bound_route_graph(&runtime, "codex");
        let endpoint_tag_target = provider_probe_target(Some(&endpoint_tag_route), "codex", probe)
            .expect("tagged provider probe target");
        let endpoint_tag_key = service_status_probe_execution_key(
            &config,
            Some(&endpoint_tag_route),
            "codex",
            probe,
            Some(&endpoint_tag_target),
        );
        assert_ne!(initial_key, endpoint_tag_key);

        runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider")
            .inline_auth
            .allow_anonymous = Some(true);

        let anonymous_route = bound_route_graph(&runtime, "codex");
        let anonymous_target = provider_probe_target(Some(&anonymous_route), "codex", probe)
            .expect("anonymous provider probe target");
        let anonymous_key = service_status_probe_execution_key(
            &config,
            Some(&anonymous_route),
            "codex",
            probe,
            Some(&anonymous_target),
        );
        assert_ne!(endpoint_tag_key, anonymous_key);
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

        let mut runtime = canonical_runtime_with_endpoint(format!("http://{addr}/v1"));
        let provider = runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider");
        provider.auth.auth_token = None;
        provider.auth.auth_token_ref = Some(CredentialRef::Native {
            name: "relay.primary".to_string(),
        });
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

        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"test-token".to_vec()).expect("valid native credential"),
        );
        let route = bind_route_graph_with_capabilities(&runtime, "codex", capabilities);
        assert_eq!(control.read_count(), 1);
        control.set_value(
            SecretValue::new(b"rotated-token".to_vec()).expect("valid rotated credential"),
        );
        let snapshot = fetch_service_status_snapshot(&config, Some(&route), "codex").await;
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
        assert_eq!(
            control.read_count(),
            1,
            "provider probe execution must not re-read the native credential"
        );
    }

    #[tokio::test]
    async fn provider_probe_never_projects_upstream_error_body() {
        let canary = "provider-body-secret-canary";
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || async move { (axum::http::StatusCode::UNAUTHORIZED, canary) }),
        );
        let (address, server) = spawn_server(app).await;
        let runtime = canonical_runtime_with_endpoint(format!("http://{address}/v1"));
        let route = bound_route_graph(&runtime, "codex");
        let config = ServiceStatusConfig {
            enabled: true,
            probes: vec![ServiceStatusProbeConfig {
                id: Some(format!("redacted-body-{address}")),
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                models: vec!["gpt-5.5".to_string()],
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };

        let snapshot = fetch_service_status_snapshot(&config, Some(&route), "codex").await;
        server.abort();

        assert_eq!(
            snapshot.probes[0].services[0]
                .latest
                .as_ref()
                .and_then(|sample| sample.error.as_deref()),
            Some("provider probe HTTP 401 Unauthorized")
        );
        assert!(
            !serde_json::to_string(&snapshot)
                .expect("serialize service status snapshot")
                .contains(canary)
        );
    }

    #[tokio::test]
    async fn blocked_provider_probe_is_local_and_recovers_without_caching_unknown_facts() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_route = Arc::clone(&hits);
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let hits = Arc::clone(&hits_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({"choices": []}))
                }
            }),
        );
        let (address, server) = spawn_server(app).await;
        let mut runtime = canonical_runtime_with_endpoint(format!("http://{address}/v1"));
        let provider = runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider");
        provider.auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.cache-recovery".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        provider.inline_auth = UpstreamAuth::default();
        let graph = CompiledRouteGraph::compile("codex", &runtime.codex)
            .expect("compile provider route graph");
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"cache-recovery-token".to_vec()).expect("valid native credential"),
        );
        control.set_missing();
        let runtime_store = RuntimeStore::open_in_memory().expect("open credential runtime store");
        let credential_runtime =
            CredentialRuntime::from_runtime_store(capabilities, &runtime_store)
                .expect("build credential runtime");
        let config = ServiceStatusConfig {
            enabled: true,
            refresh_interval_secs: 3_600,
            probes: vec![ServiceStatusProbeConfig {
                id: Some(format!("blocked-recovery-{address}")),
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                models: vec!["gpt-5.5".to_string()],
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };

        let blocked_route = rebound_route_graph_with_runtime(&graph, &credential_runtime, "codex");
        let blocked = refresh_service_status_snapshot(&config, Some(&blocked_route), "codex").await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            blocked.probes[0].credential_readiness,
            Some(CredentialReadinessCode::Missing)
        );
        assert_eq!(blocked.probes[0].fetched_at_ms, 0);
        assert_eq!(blocked.probes[0].all_ok, None);
        assert_eq!(
            blocked.probes[0].services[0].latest_kind,
            ServiceStatusKind::Unknown
        );
        assert_eq!(blocked.probes[0].services[0].latest, None);
        assert_eq!(
            blocked.probes[0].credential_details[0].reference.as_deref(),
            Some("relay.cache-recovery")
        );

        control.set_value(
            SecretValue::new(b"cache-recovery-token".to_vec()).expect("valid recovered credential"),
        );
        let recovered_route =
            rebound_route_graph_with_runtime(&graph, &credential_runtime, "codex");
        let recovered =
            refresh_service_status_snapshot(&config, Some(&recovered_route), "codex").await;
        server.abort();

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            recovered.probes[0].credential_readiness,
            Some(CredentialReadinessCode::Ready)
        );
        assert_eq!(recovered.probes[0].all_ok, Some(true));
        assert_eq!(
            recovered.probes[0].services[0].latest_kind,
            ServiceStatusKind::Ok
        );
    }

    #[tokio::test]
    async fn operator_projection_refreshes_cold_probe_in_background_with_singleflight() {
        let hits = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let hits_for_route = Arc::clone(&hits);
        let release_for_route = Arc::clone(&release);
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let hits = Arc::clone(&hits_for_route);
                let release = Arc::clone(&release_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    release.notified().await;
                    Json(serde_json::json!({"choices": []}))
                }
            }),
        );
        let (address, server) = spawn_server(app).await;
        let runtime = canonical_runtime_with_endpoint(format!("http://{address}/v1"));
        let route = bound_route_graph(&runtime, "codex");
        let config = ServiceStatusConfig {
            enabled: true,
            refresh_interval_secs: 3_600,
            probes: vec![ServiceStatusProbeConfig {
                id: Some(format!("background-singleflight-{address}")),
                provider: Some("relay".to_string()),
                endpoint: Some("fast".to_string()),
                models: vec!["gpt-5.5".to_string()],
                ..ServiceStatusProbeConfig::default()
            }],
            ..ServiceStatusConfig::default()
        };

        let pending = project_service_status_snapshot(&config, Some(&route), "codex");
        assert_eq!(pending.probes[0].fetched_at_ms, 0);
        assert_eq!(
            pending.probes[0].services[0].latest_kind,
            ServiceStatusKind::Unknown
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            while hits.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background probe started");

        let still_pending = project_service_status_snapshot(&config, Some(&route), "codex");
        assert_eq!(
            still_pending.probes[0].services[0].latest_kind,
            ServiceStatusKind::Unknown
        );
        tokio::task::yield_now().await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        release.notify_waiters();
        let ready = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let snapshot = project_service_status_snapshot(&config, Some(&route), "codex");
                if snapshot.probes[0].services[0].latest_kind == ServiceStatusKind::Ok {
                    break snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background probe completed");
        server.abort();

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_ne!(ready.probes[0].fetched_at_ms, 0);
        assert_eq!(
            ready.probes[0].credential_readiness,
            Some(CredentialReadinessCode::Ready)
        );
    }

    #[tokio::test]
    async fn service_status_refuses_provider_redirects_with_native_api_keys() {
        let native_api_key = "native-api-key-must-not-leave-origin";

        for redirect_status in [
            axum::http::StatusCode::TEMPORARY_REDIRECT,
            axum::http::StatusCode::PERMANENT_REDIRECT,
        ] {
            let fixture = RedirectFixture::spawn(redirect_status).await;

            let mut runtime =
                canonical_runtime_with_endpoint(format!("http://{}/v1", fixture.source_address));
            let provider = runtime
                .codex
                .providers
                .get_mut("relay")
                .expect("relay provider");
            provider.auth = UpstreamAuth {
                api_key_ref: Some(CredentialRef::Native {
                    name: "relay.primary".to_string(),
                }),
                ..UpstreamAuth::default()
            };
            provider.inline_auth = UpstreamAuth::default();
            let (capabilities, _control) = CredentialSourceCapabilities::test_native(
                SecretValue::new(native_api_key.as_bytes().to_vec()).expect("valid native API key"),
            );
            let route = bind_route_graph_with_capabilities(&runtime, "codex", capabilities);
            let config = ServiceStatusConfig {
                enabled: true,
                timeout_ms: 3_000,
                probes: vec![ServiceStatusProbeConfig {
                    id: Some(format!("provider-{}", redirect_status.as_u16())),
                    provider: Some("relay".to_string()),
                    endpoint: Some("fast".to_string()),
                    models: vec!["gpt-5.5".to_string()],
                    ..ServiceStatusProbeConfig::default()
                }],
                ..ServiceStatusConfig::default()
            };

            let snapshot = fetch_service_status_snapshot(&config, Some(&route), "codex").await;
            let sample = snapshot.probes[0].services[0]
                .latest
                .as_ref()
                .expect("failed provider probe sample");
            let error = sample.error.as_deref().expect("provider redirect error");

            assert_eq!(sample.ok, Some(false));
            assert!(
                error.contains(&format!("HTTP {redirect_status}")),
                "unexpected provider redirect error: {error}"
            );
            assert!(!error.contains(native_api_key));
            fixture.assert_not_followed();
        }
    }

    #[tokio::test]
    async fn service_status_refuses_custom_header_redirects() {
        let custom_secret = "custom-header-must-not-leave-origin";

        for redirect_status in [
            axum::http::StatusCode::TEMPORARY_REDIRECT,
            axum::http::StatusCode::PERMANENT_REDIRECT,
        ] {
            let fixture = RedirectFixture::spawn(redirect_status).await;
            let config = ServiceStatusConfig {
                enabled: true,
                timeout_ms: 3_000,
                probes: vec![ServiceStatusProbeConfig {
                    id: Some(format!("custom-{}", redirect_status.as_u16())),
                    url: Some(format!("http://{}/status", fixture.source_address)),
                    models: vec!["gpt-5.5".to_string()],
                    headers: std::collections::BTreeMap::from([
                        ("x-api-key".to_string(), custom_secret.to_string()),
                        ("x-custom-secret".to_string(), custom_secret.to_string()),
                    ]),
                    ..ServiceStatusProbeConfig::default()
                }],
                ..ServiceStatusConfig::default()
            };

            let snapshot = fetch_service_status_snapshot(&config, None, "codex").await;
            let error = snapshot.probes[0]
                .error
                .as_deref()
                .expect("custom header redirect error");

            assert_eq!(error, SERVICE_STATUS_PROBE_FAILURE_REASON);
            assert!(!error.contains(custom_secret));
            fixture.assert_not_followed();
        }
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
        let route = bound_route_graph(&runtime, "codex");
        let target = provider_probe_targets(&route, Some("relay"), Some("fast"))
            .expect("capture provider probe targets")
            .into_iter()
            .next()
            .expect("provider probe target");
        let url = chat_completions_url(&target.base_url);

        let error = send_provider_probe_request(&client, &target, &url, "gpt-5.5", 3_000)
            .await
            .expect_err("remote target must fail closed without helper auth");

        assert_eq!(*hits.lock().expect("provider probe hits lock"), 0);
        assert_eq!(error.to_string(), UPSTREAM_AUTH_UNAVAILABLE_REASON);

        runtime
            .codex
            .providers
            .get_mut("relay")
            .expect("relay provider")
            .inline_auth
            .allow_anonymous = Some(true);
        let anonymous_route = bound_route_graph(&runtime, "codex");
        let anonymous_target =
            provider_probe_targets(&anonymous_route, Some("relay"), Some("fast"))
                .expect("capture anonymous provider probe targets")
                .into_iter()
                .next()
                .expect("anonymous provider probe target");
        send_provider_probe_request(&client, &anonymous_target, &url, "gpt-5.5", 3_000)
            .await
            .expect("explicit anonymous opt-in should allow the probe");

        assert_eq!(*hits.lock().expect("provider probe hits lock"), 1);

        server.abort();
    }
}
