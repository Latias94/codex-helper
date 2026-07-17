use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use tracing::warn;

use crate::config::{HelperConfig, LoadedConfig, ServiceRouteConfig};
use crate::logging::log_control_trace_event;
use crate::pricing::{CapturedModelPriceCatalog, try_capture_operator_model_price_catalog};
use crate::provider_catalog::ProviderCatalogSnapshot;
use crate::routing_ir::{CompiledRouteGraph, RoutePlanTemplate, RouteRequestContext};
use crate::runtime_identity::{RuntimeUpstreamIdentity, diff_runtime_upstream_identities};
use crate::runtime_store::{ProviderPolicySnapshot, RuntimeStore};
use crate::state::ProxyState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RuntimeSourceStamp {
    config_mtime: Option<SystemTime>,
    pricing_mtime: Option<SystemTime>,
}

impl RuntimeSourceStamp {
    #[cfg(test)]
    fn config(config_mtime: Option<SystemTime>) -> Self {
        Self {
            config_mtime,
            pricing_mtime: None,
        }
    }
}

#[derive(Debug)]
struct PreparedRuntimeSnapshot {
    config: Arc<HelperConfig>,
    codex_route_graph: Arc<CompiledRouteGraph>,
    claude_route_graph: Arc<CompiledRouteGraph>,
    provider_catalog: Arc<ProviderCatalogSnapshot>,
    operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
    digest_seed: Sha256,
    loaded_at_ms: u64,
    source_stamp: RuntimeSourceStamp,
}

impl PreparedRuntimeSnapshot {
    fn build(
        config: Arc<HelperConfig>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        source_stamp: RuntimeSourceStamp,
    ) -> Result<Self> {
        let codex_route_graph = compile_route_graph("codex", &config.codex)?;
        let claude_route_graph = compile_route_graph("claude", &config.claude)?;
        let provider_catalog = Arc::new(ProviderCatalogSnapshot::bundled());
        let digest_seed = runtime_snapshot_digest_seed(
            config.as_ref(),
            codex_route_graph.as_ref(),
            claude_route_graph.as_ref(),
            provider_catalog.as_ref(),
            operator_pricing_catalog.revision(),
        )?;

        Ok(Self {
            config,
            codex_route_graph,
            claude_route_graph,
            provider_catalog,
            operator_pricing_catalog,
            digest_seed,
            loaded_at_ms: now_ms(),
            source_stamp,
        })
    }

    fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        let mut identities = self.codex_route_graph.candidate_identities();
        identities.extend(self.claude_route_graph.candidate_identities());
        identities
    }

    fn finish(
        self,
        provider_policy: Arc<ProviderPolicySnapshot>,
        revision: u64,
    ) -> RuntimeSnapshot {
        let digest = runtime_snapshot_digest_from_seed(self.digest_seed.clone(), &provider_policy);
        RuntimeSnapshot {
            config: self.config,
            codex_route_graph: self.codex_route_graph,
            claude_route_graph: self.claude_route_graph,
            provider_catalog: self.provider_catalog,
            operator_pricing_catalog: self.operator_pricing_catalog,
            provider_policy,
            revision,
            digest,
            digest_seed: self.digest_seed,
            loaded_at_ms: self.loaded_at_ms,
            source_stamp: self.source_stamp,
        }
    }
}

#[derive(Debug)]
pub(super) struct RuntimeSnapshot {
    config: Arc<HelperConfig>,
    codex_route_graph: Arc<CompiledRouteGraph>,
    claude_route_graph: Arc<CompiledRouteGraph>,
    provider_catalog: Arc<ProviderCatalogSnapshot>,
    operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
    provider_policy: Arc<ProviderPolicySnapshot>,
    revision: u64,
    digest: String,
    digest_seed: Sha256,
    loaded_at_ms: u64,
    source_stamp: RuntimeSourceStamp,
}

pub(super) struct CapturedRoutePlan {
    snapshot: Arc<RuntimeSnapshot>,
    template: RoutePlanTemplate,
    routing_control_graph_key: String,
}

impl CapturedRoutePlan {
    pub(super) fn template(&self) -> &RoutePlanTemplate {
        &self.template
    }

    pub(super) fn runtime_revision(&self) -> u64 {
        self.snapshot.revision()
    }

    pub(super) fn routing_control_graph_key(&self) -> &str {
        self.routing_control_graph_key.as_str()
    }

    pub(super) fn provider_policy(&self) -> &ProviderPolicySnapshot {
        self.snapshot.provider_policy.as_ref()
    }

    pub(super) fn runtime_snapshot(&self) -> Arc<RuntimeSnapshot> {
        Arc::clone(&self.snapshot)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.template.candidates.is_empty()
    }
}

impl RuntimeSnapshot {
    pub(super) fn capture_route_plan(
        self: &Arc<Self>,
        service_name: &str,
        request: &RouteRequestContext,
    ) -> Result<Option<CapturedRoutePlan>> {
        let Some(graph) = self.route_graph(service_name) else {
            return Ok(None);
        };
        let template = graph.route_plan(request)?;
        log_control_trace_event(serde_json::json!({
            "event": "route_plan_selected",
            "service": template.service_name,
            "entry": template.entry,
            "route_graph_key": template.route_graph_key(),
            "runtime_revision": self.revision,
            "candidate_count": template.candidates.len(),
            "provider_endpoint_candidates": template
                .candidates
                .iter()
                .map(|candidate| {
                    let identity = template.candidate_identity(candidate);
                    serde_json::json!({
                        "provider_id": candidate.provider_id,
                        "endpoint_id": candidate.endpoint_id,
                        "provider_endpoint_key": identity.provider_endpoint.stable_key(),
                        "preference_group": candidate.preference_group,
                        "route_path": candidate.route_path,
                    })
                })
                .collect::<Vec<_>>(),
        }));
        Ok(Some(CapturedRoutePlan {
            snapshot: Arc::clone(self),
            template,
            routing_control_graph_key: graph.digest().to_string(),
        }))
    }

    fn with_provider_policy(
        &self,
        provider_policy: Arc<ProviderPolicySnapshot>,
        revision: u64,
        loaded_at_ms: u64,
    ) -> Self {
        let digest = runtime_snapshot_digest_from_seed(self.digest_seed.clone(), &provider_policy);
        Self {
            config: Arc::clone(&self.config),
            codex_route_graph: Arc::clone(&self.codex_route_graph),
            claude_route_graph: Arc::clone(&self.claude_route_graph),
            provider_catalog: Arc::clone(&self.provider_catalog),
            operator_pricing_catalog: Arc::clone(&self.operator_pricing_catalog),
            provider_policy,
            revision,
            digest,
            digest_seed: self.digest_seed.clone(),
            loaded_at_ms,
            source_stamp: self.source_stamp,
        }
    }

    fn with_operator_pricing_catalog(
        &self,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        revision: u64,
        loaded_at_ms: u64,
    ) -> Result<Self> {
        let digest_seed = runtime_snapshot_digest_seed(
            self.config.as_ref(),
            self.codex_route_graph.as_ref(),
            self.claude_route_graph.as_ref(),
            self.provider_catalog.as_ref(),
            operator_pricing_catalog.revision(),
        )?;
        let digest = runtime_snapshot_digest_from_seed(digest_seed.clone(), &self.provider_policy);
        Ok(Self {
            config: Arc::clone(&self.config),
            codex_route_graph: Arc::clone(&self.codex_route_graph),
            claude_route_graph: Arc::clone(&self.claude_route_graph),
            provider_catalog: Arc::clone(&self.provider_catalog),
            operator_pricing_catalog,
            provider_policy: Arc::clone(&self.provider_policy),
            revision,
            digest,
            digest_seed,
            loaded_at_ms,
            source_stamp: self.source_stamp,
        })
    }

    pub(super) fn config(&self) -> Arc<HelperConfig> {
        Arc::clone(&self.config)
    }

    pub(super) fn route_graph(&self, service_name: &str) -> Option<Arc<CompiledRouteGraph>> {
        match service_name {
            "codex" => Some(Arc::clone(&self.codex_route_graph)),
            "claude" => Some(Arc::clone(&self.claude_route_graph)),
            _ => None,
        }
    }

    pub(super) fn provider_catalog(&self) -> Arc<ProviderCatalogSnapshot> {
        Arc::clone(&self.provider_catalog)
    }

    pub(super) fn operator_pricing_catalog(&self) -> Arc<CapturedModelPriceCatalog> {
        Arc::clone(&self.operator_pricing_catalog)
    }

    pub(super) fn provider_policy(&self) -> Arc<ProviderPolicySnapshot> {
        Arc::clone(&self.provider_policy)
    }

    fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        let mut identities = self.codex_route_graph.candidate_identities();
        identities.extend(self.claude_route_graph.candidate_identities());
        identities
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn digest(&self) -> &str {
        self.digest.as_str()
    }

    pub(super) fn loaded_at_ms(&self) -> u64 {
        self.loaded_at_ms
    }

    fn source_mtime(&self) -> Option<SystemTime> {
        self.source_stamp.config_mtime
    }

    fn source_stamp(&self) -> RuntimeSourceStamp {
        self.source_stamp
    }

    pub(super) fn source_mtime_ms(&self) -> Option<u64> {
        self.source_mtime()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
    }
}

pub(super) struct RuntimeConfig {
    current: RwLock<Arc<RuntimeSnapshot>>,
    reload_check: AsyncMutex<RuntimeConfigReloadCheckState>,
    publish: AsyncMutex<RuntimeConfigPublishState>,
    next_build_ticket: AtomicU64,
    policy_state: Option<Arc<ProxyState>>,
    automatic_reload: RuntimeAutomaticReload,
}

type RuntimeSourceStampFuture = Pin<Box<dyn Future<Output = RuntimeSourceStamp> + Send>>;
type RuntimeConfigLoadFuture = Pin<Box<dyn Future<Output = Result<LoadedConfig>> + Send>>;
type RuntimePricingLoadFuture =
    Pin<Box<dyn Future<Output = Result<CapturedModelPriceCatalog>> + Send>>;

struct RuntimeAutomaticReload {
    min_check_interval: Duration,
    metadata: Arc<dyn Fn() -> RuntimeSourceStampFuture + Send + Sync>,
    config: Arc<dyn Fn() -> RuntimeConfigLoadFuture + Send + Sync>,
    pricing: Arc<dyn Fn() -> RuntimePricingLoadFuture + Send + Sync>,
}

impl RuntimeAutomaticReload {
    const MIN_CHECK_INTERVAL: Duration = Duration::from_millis(800);

    #[cfg(not(test))]
    fn disk() -> Self {
        Self {
            min_check_interval: Self::MIN_CHECK_INTERVAL,
            metadata: Arc::new(|| Box::pin(runtime_source_stamp_from_disk())),
            config: Arc::new(|| Box::pin(crate::config::load_config_with_source())),
            pricing: Arc::new(|| Box::pin(capture_strict_operator_pricing_catalog())),
        }
    }

    #[cfg(test)]
    fn unchanged(source_stamp: RuntimeSourceStamp) -> Self {
        Self {
            min_check_interval: Self::MIN_CHECK_INTERVAL,
            metadata: Arc::new(move || Box::pin(std::future::ready(source_stamp))),
            config: Arc::new(|| {
                Box::pin(async { anyhow::bail!("unchanged source stamp must skip config loading") })
            }),
            pricing: Arc::new(|| {
                Box::pin(async {
                    anyhow::bail!("unchanged source stamp must skip pricing loading")
                })
            }),
        }
    }
}

#[derive(Debug)]
struct RuntimeConfigReloadCheckState {
    last_check_at: Instant,
}

#[derive(Debug, Default)]
struct RuntimeConfigPublishState {
    highest_publish_ticket: u64,
}

impl RuntimeConfig {
    #[cfg(test)]
    pub(super) fn new_with_config(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
    ) -> Result<Self> {
        Self::new_inner(initial_config, provider_policy, None)
    }

    pub(super) fn new_with_policy_state(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
        policy_state: Arc<ProxyState>,
    ) -> Result<Self> {
        Self::new_inner(initial_config, provider_policy, Some(policy_state))
    }

    fn new_inner(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
        policy_state: Option<Arc<ProxyState>>,
    ) -> Result<Self> {
        let operator_pricing_catalog = try_capture_operator_model_price_catalog()
            .map(Arc::new)
            .map_err(anyhow::Error::msg)
            .context("load initial operator pricing catalog")?;
        let source_stamp = runtime_source_stamp_from_disk_sync();
        let initial =
            PreparedRuntimeSnapshot::build(initial_config, operator_pricing_catalog, source_stamp)?
                .finish(provider_policy, 1);
        #[cfg(not(test))]
        let automatic_reload = RuntimeAutomaticReload::disk();
        #[cfg(test)]
        let automatic_reload = RuntimeAutomaticReload::unchanged(source_stamp);
        Ok(Self {
            current: RwLock::new(Arc::new(initial)),
            reload_check: AsyncMutex::new(RuntimeConfigReloadCheckState {
                last_check_at: Instant::now()
                    .checked_sub(Duration::from_secs(60))
                    .unwrap_or_else(Instant::now),
            }),
            publish: AsyncMutex::new(RuntimeConfigPublishState::default()),
            next_build_ticket: AtomicU64::new(1),
            policy_state,
            automatic_reload,
        })
    }

    #[cfg(test)]
    fn new_with_config_and_automatic_reload(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
        source_stamp: RuntimeSourceStamp,
        automatic_reload: RuntimeAutomaticReload,
    ) -> Result<Self> {
        let operator_pricing_catalog = try_capture_operator_model_price_catalog()
            .map(Arc::new)
            .map_err(anyhow::Error::msg)
            .context("load initial operator pricing catalog")?;
        let initial =
            PreparedRuntimeSnapshot::build(initial_config, operator_pricing_catalog, source_stamp)?
                .finish(provider_policy, 1);
        Ok(Self {
            current: RwLock::new(Arc::new(initial)),
            reload_check: AsyncMutex::new(RuntimeConfigReloadCheckState {
                last_check_at: Instant::now()
                    .checked_sub(Duration::from_secs(60))
                    .unwrap_or_else(Instant::now),
            }),
            publish: AsyncMutex::new(RuntimeConfigPublishState::default()),
            next_build_ticket: AtomicU64::new(1),
            policy_state: None,
            automatic_reload,
        })
    }

    pub(super) fn reconcile_initial_provider_policy(
        config: &HelperConfig,
        runtime_store: &RuntimeStore,
    ) -> Result<Arc<ProviderPolicySnapshot>> {
        let codex_route_graph = compile_route_graph("codex", &config.codex)?;
        let claude_route_graph = compile_route_graph("claude", &config.claude)?;
        let mut identities = codex_route_graph.candidate_identities();
        identities.extend(claude_route_graph.candidate_identities());
        runtime_store
            .reconcile_runtime_upstream_identities(&identities, now_ms())
            .map(Arc::new)
            .context("reconcile initial runtime upstream identities")
    }

    fn capture_current(&self) -> Arc<RuntimeSnapshot> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn store_current(&self, snapshot: RuntimeSnapshot) {
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(snapshot);
    }

    fn reserve_build_ticket(&self) -> u64 {
        self.next_build_ticket.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) async fn capture(&self) -> Arc<RuntimeSnapshot> {
        let current = self.capture_current();
        let Some(policy_state) = self.policy_state.as_ref() else {
            return current;
        };
        let provider_policy = policy_state.capture_provider_policy_snapshot().await;
        if current.provider_policy.as_ref() == provider_policy.as_ref() {
            return current;
        }
        if let Err(error) = self.publish_provider_policy(provider_policy).await {
            warn!(error = %error, "failed to synchronize committed provider policy into runtime snapshot");
        }
        self.capture_current()
    }

    pub(super) async fn snapshot(&self) -> Arc<HelperConfig> {
        self.capture().await.config()
    }

    pub(super) async fn force_reload_from_disk(&self) -> Result<bool> {
        self.reload_with_source_and_pricing(
            || async {
                let source_stamp = runtime_source_stamp_from_disk().await;
                let loaded = crate::config::load_config_with_source().await?;
                Ok((loaded, source_stamp))
            },
            capture_strict_operator_pricing_catalog,
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn reload_with_source<Source, SourceFuture>(
        &self,
        source: Source,
    ) -> Result<bool>
    where
        Source: FnOnce() -> SourceFuture,
        SourceFuture: Future<Output = Result<(LoadedConfig, Option<SystemTime>)>>,
    {
        let pricing = self
            .capture_current()
            .operator_pricing_catalog()
            .as_ref()
            .clone();
        self.reload_with_source_and_pricing(
            || async {
                let (loaded, source_mtime) = source().await?;
                Ok((loaded, RuntimeSourceStamp::config(source_mtime)))
            },
            || async move { Ok(pricing) },
        )
        .await
    }

    async fn reload_with_source_and_pricing<Source, SourceFuture, Pricing, PricingFuture>(
        &self,
        source: Source,
        pricing: Pricing,
    ) -> Result<bool>
    where
        Source: FnOnce() -> SourceFuture,
        SourceFuture: Future<Output = Result<(LoadedConfig, RuntimeSourceStamp)>>,
        Pricing: FnOnce() -> PricingFuture,
        PricingFuture: Future<Output = Result<CapturedModelPriceCatalog>>,
    {
        let ticket = self.reserve_build_ticket();
        let (loaded, source_stamp) = source().await?;
        let operator_pricing_catalog = pricing().await?;
        let prepared =
            prepare_runtime_snapshot(loaded, source_stamp, operator_pricing_catalog).await?;
        self.publish_prepared(ticket, prepared).await
    }

    async fn maybe_reload_with<
        Metadata,
        MetadataFuture,
        Loader,
        LoaderFuture,
        Pricing,
        PricingFuture,
    >(
        &self,
        min_check_interval: Duration,
        metadata: Metadata,
        loader: Loader,
        pricing: Pricing,
    ) -> Result<bool>
    where
        Metadata: FnOnce() -> MetadataFuture,
        MetadataFuture: Future<Output = RuntimeSourceStamp>,
        Loader: FnOnce() -> LoaderFuture,
        LoaderFuture: Future<Output = Result<LoadedConfig>>,
        Pricing: FnOnce() -> PricingFuture,
        PricingFuture: Future<Output = Result<CapturedModelPriceCatalog>>,
    {
        let source_stamp = {
            let mut check_state = self.reload_check.lock().await;
            if check_state.last_check_at.elapsed() < min_check_interval {
                return Ok(false);
            }
            let source_stamp = metadata().await;
            check_state.last_check_at = Instant::now();
            source_stamp
        };
        if source_stamp == self.capture_current().source_stamp() {
            return Ok(false);
        }

        let ticket = self.reserve_build_ticket();
        let loaded = loader().await?;
        let operator_pricing_catalog = pricing().await?;
        let prepared =
            prepare_runtime_snapshot(loaded, source_stamp, operator_pricing_catalog).await?;
        self.publish_prepared(ticket, prepared).await
    }

    async fn publish_prepared(
        &self,
        ticket: u64,
        prepared: PreparedRuntimeSnapshot,
    ) -> Result<bool> {
        let mut publish_state = self.publish.lock().await;
        if ticket <= publish_state.highest_publish_ticket {
            return Ok(false);
        }
        publish_state.highest_publish_ticket = ticket;

        let previous = self.capture_current();
        let next_identities = prepared.candidate_identities();
        let routing_control_graphs = [
            ("codex", prepared.codex_route_graph.digest().to_string()),
            ("claude", prepared.claude_route_graph.digest().to_string()),
        ];
        let identity_delta =
            diff_runtime_upstream_identities(&previous.candidate_identities(), &next_identities);
        let provider_policy = match self.policy_state.as_ref() {
            Some(policy_state)
                if !identity_delta.added.is_empty() || !identity_delta.removed.is_empty() =>
            {
                policy_state
                    .reconcile_runtime_upstream_identities(&next_identities, now_ms())
                    .await
                    .context("reconcile reloaded runtime upstream identities")?
            }
            Some(policy_state) => policy_state.capture_provider_policy_snapshot().await,
            None => previous.provider_policy(),
        };
        if let Some(policy_state) = self.policy_state.as_ref() {
            for (service_name, route_graph_key) in routing_control_graphs {
                policy_state
                    .reconcile_routing_operator_route_graph(service_name, route_graph_key.as_str())
                    .await
                    .with_context(|| {
                        format!(
                            "reconcile {service_name} routing operator control after config reload"
                        )
                    })?;
            }
        }
        let mut next = prepared.finish(provider_policy, previous.revision());
        let changed = next.digest() != previous.digest();
        if changed {
            next.revision = previous.revision().saturating_add(1);
        }
        self.store_current(next);
        Ok(changed)
    }

    pub(super) async fn publish_provider_policy(
        &self,
        provider_policy: Arc<ProviderPolicySnapshot>,
    ) -> Result<bool> {
        let _publisher_guard = self.publish.lock().await;
        let previous = self.capture_current();
        if provider_policy.policy_revision < previous.provider_policy.policy_revision {
            return Ok(false);
        }
        if provider_policy.policy_revision == previous.provider_policy.policy_revision
            && provider_policy.as_ref() != previous.provider_policy.as_ref()
        {
            anyhow::bail!(
                "provider policy revision {} conflicts with the current runtime snapshot",
                provider_policy.policy_revision
            );
        }
        if previous.provider_policy.as_ref() == provider_policy.as_ref() {
            return Ok(false);
        }
        let next = previous.with_provider_policy(
            provider_policy,
            previous.revision().saturating_add(1),
            now_ms(),
        );
        self.store_current(next);
        Ok(true)
    }

    pub(super) async fn publish_operator_pricing_catalog(&self) -> Result<bool> {
        let operator_pricing_catalog =
            tokio::task::spawn_blocking(try_capture_operator_model_price_catalog)
                .await
                .context("join operator pricing catalog loader")?
                .map(Arc::new)
                .map_err(anyhow::Error::msg)
                .context("load operator pricing catalog")?;

        self.publish_captured_operator_pricing_catalog(operator_pricing_catalog)
            .await
    }

    async fn publish_captured_operator_pricing_catalog(
        &self,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
    ) -> Result<bool> {
        let _publisher_guard = self.publish.lock().await;
        let previous = self.capture_current();
        if previous.operator_pricing_catalog().revision() == operator_pricing_catalog.revision() {
            return Ok(false);
        }
        let next = previous.with_operator_pricing_catalog(
            operator_pricing_catalog,
            previous.revision().saturating_add(1),
            now_ms(),
        )?;
        self.store_current(next);
        Ok(true)
    }

    pub(super) async fn maybe_reload_from_disk(&self) -> bool {
        let result = self
            .maybe_reload_with(
                self.automatic_reload.min_check_interval,
                || (self.automatic_reload.metadata)(),
                || (self.automatic_reload.config)(),
                || (self.automatic_reload.pricing)(),
            )
            .await;
        match result {
            Ok(changed) => changed,
            Err(error) => {
                warn!("failed to reload config from disk: {}", error);
                false
            }
        }
    }
}

async fn runtime_source_stamp_from_disk() -> RuntimeSourceStamp {
    let (config_mtime, pricing_mtime) = tokio::join!(
        source_mtime(crate::config::config_file_path()),
        source_mtime(crate::pricing::model_price_overrides_path()),
    );
    RuntimeSourceStamp {
        config_mtime,
        pricing_mtime,
    }
}

fn runtime_source_stamp_from_disk_sync() -> RuntimeSourceStamp {
    RuntimeSourceStamp {
        config_mtime: source_mtime_sync(crate::config::config_file_path()),
        pricing_mtime: source_mtime_sync(crate::pricing::model_price_overrides_path()),
    }
}

fn source_mtime_sync(path: std::path::PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

async fn source_mtime(path: std::path::PathBuf) -> Option<SystemTime> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

async fn capture_strict_operator_pricing_catalog() -> Result<CapturedModelPriceCatalog> {
    tokio::task::spawn_blocking(try_capture_operator_model_price_catalog)
        .await
        .context("join operator pricing catalog loader")?
        .map_err(anyhow::Error::msg)
        .context("load operator pricing catalog")
}

async fn prepare_runtime_snapshot(
    loaded: LoadedConfig,
    source_stamp: RuntimeSourceStamp,
    operator_pricing_catalog: CapturedModelPriceCatalog,
) -> Result<PreparedRuntimeSnapshot> {
    tokio::task::spawn_blocking(move || {
        PreparedRuntimeSnapshot::build(
            Arc::new(loaded.source),
            Arc::new(operator_pricing_catalog),
            source_stamp,
        )
    })
    .await
    .context("join runtime snapshot builder")?
}

fn compile_route_graph(
    service_name: &str,
    service: &ServiceRouteConfig,
) -> Result<Arc<CompiledRouteGraph>> {
    CompiledRouteGraph::compile(service_name, service)
        .with_context(|| format!("compile {service_name} route graph for runtime snapshot"))
        .map(Arc::new)
}

#[cfg(test)]
fn runtime_snapshot_digest(
    config: &HelperConfig,
    codex_route_graph: &CompiledRouteGraph,
    claude_route_graph: &CompiledRouteGraph,
    provider_catalog: &ProviderCatalogSnapshot,
    operator_pricing_revision: &str,
    provider_policy: &ProviderPolicySnapshot,
) -> Result<String> {
    let seed = runtime_snapshot_digest_seed(
        config,
        codex_route_graph,
        claude_route_graph,
        provider_catalog,
        operator_pricing_revision,
    )?;
    Ok(runtime_snapshot_digest_from_seed(seed, provider_policy))
}

fn runtime_snapshot_digest_seed(
    config: &HelperConfig,
    codex_route_graph: &CompiledRouteGraph,
    claude_route_graph: &CompiledRouteGraph,
    provider_catalog: &ProviderCatalogSnapshot,
    operator_pricing_revision: &str,
) -> Result<Sha256> {
    let mut hasher = Sha256::new();
    hash_digest_part(&mut hasher, b"codex-helper:runtime-snapshot:v1");
    hash_digest_part(&mut hasher, b"canonical-config");
    hash_json_part(&mut hasher, config)
        .context("serialize canonical config for snapshot digest")?;
    for (service_name, graph) in [("codex", codex_route_graph), ("claude", claude_route_graph)] {
        hash_digest_part(&mut hasher, service_name.as_bytes());
        hash_digest_part(&mut hasher, graph.digest().as_bytes());
    }
    hash_digest_part(
        &mut hasher,
        provider_catalog.catalog_revision().as_str().as_bytes(),
    );
    hash_digest_part(
        &mut hasher,
        provider_catalog.pricing_revision().as_str().as_bytes(),
    );
    hash_digest_part(&mut hasher, operator_pricing_revision.as_bytes());
    Ok(hasher)
}

fn runtime_snapshot_digest_from_seed(
    mut hasher: Sha256,
    provider_policy: &ProviderPolicySnapshot,
) -> String {
    hash_digest_part(&mut hasher, &provider_policy.policy_revision.to_be_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn hash_json_part<T>(hasher: &mut Sha256, value: &T) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let canonical = canonicalize_json(serde_json::to_value(value)?);
    let encoded = serde_json::to_vec(&canonical)?;
    hash_digest_part(hasher, encoded.as_slice());
    Ok(())
}

fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

fn hash_digest_part(hasher: &mut Sha256, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hasher.update(length.to_be_bytes());
    hasher.update(value);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::config::{ProviderConfig, RouteGraphConfig, UpstreamAuth};
    use crate::pricing::capture_operator_model_price_catalog;
    use crate::runtime_identity::ProviderEndpointKey;
    use tokio::sync::oneshot;

    fn loaded_route_graph(label: &str) -> LoadedConfig {
        let source = route_graph_source(label);
        LoadedConfig { source }
    }

    fn route_graph_source(label: &str) -> HelperConfig {
        let codex_provider = format!("codex-{label}");
        let claude_provider = format!("claude-{label}");
        HelperConfig {
            codex: service_view(&codex_provider),
            claude: service_view(&claude_provider),
            ..HelperConfig::default()
        }
    }

    fn implicit_route_graph_source(label: &str) -> HelperConfig {
        let provider_id = format!("codex-{label}");
        HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    provider_id.clone(),
                    ProviderConfig {
                        base_url: Some(format!("https://{provider_id}.example/v1")),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: None,
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    fn service_view(provider_id: &str) -> ServiceRouteConfig {
        ServiceRouteConfig {
            providers: BTreeMap::from([(
                provider_id.to_string(),
                ProviderConfig {
                    base_url: Some(format!("https://{provider_id}.example/v1")),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                provider_id.to_string(),
            ])),
            ..ServiceRouteConfig::default()
        }
    }

    fn stable_identity_source(base_url: &str, continuity_domain: &str) -> HelperConfig {
        HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "stable".to_string(),
                    ProviderConfig {
                        base_url: Some(base_url.to_string()),
                        continuity_domain: Some(continuity_domain.to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "stable".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    fn blocked_policy_runtime(
        source: HelperConfig,
    ) -> (RuntimeConfig, Arc<ProxyState>, Arc<RuntimeStore>) {
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let graph = compile_route_graph("codex", &source.codex).expect("compile initial graph");
        let identity = graph
            .candidate_identities()
            .into_iter()
            .next()
            .expect("initial identity");
        let scope = crate::runtime_store::ProviderObservationScope::new(
            identity.provider_endpoint.clone(),
            identity.base_url.as_str(),
            identity.policy_route_scope(),
            "test:runtime-reload",
            "https://console.example.test/v1/usage",
            "sha256:account-a",
            "sha256:config-a",
        )
        .expect("build initial observation scope");
        let reservation = runtime_store
            .reserve_provider_observation(scope, 10)
            .expect("reserve initial observation");
        runtime_store
            .commit_provider_observation(
                reservation.ticket,
                crate::runtime_store::ProviderObservation {
                    observed_at_unix_ms: 10,
                    completed_at_unix_ms: 11,
                    authority: crate::runtime_store::ProviderObservationAuthority::Authoritative,
                    evidence: serde_json::json!({"remaining": 0}),
                    effect: crate::runtime_store::ProviderPolicyEffect::Block {
                        action_kind: "balance_exhausted".to_string(),
                        code: Some("balance_exhausted".to_string()),
                        reason: "test exhaustion".to_string(),
                        expires_at_unix_ms: None,
                    },
                },
            )
            .expect("commit initial automatic block");
        let provider_policy =
            RuntimeConfig::reconcile_initial_provider_policy(&source, runtime_store.as_ref())
                .expect("reconcile initial provider policy");
        let state = ProxyState::new_with_runtime_store(Arc::clone(&runtime_store))
            .expect("build proxy state");
        let runtime = RuntimeConfig::new_with_policy_state(
            Arc::new(source),
            provider_policy,
            Arc::clone(&state),
        )
        .expect("build policy-backed runtime config");
        (runtime, state, runtime_store)
    }

    fn invalid_loaded_route_graph() -> LoadedConfig {
        let mut source = route_graph_source("invalid");
        source.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "codex-invalid".to_string(),
            "codex-invalid".to_string(),
        ]));
        LoadedConfig { source }
    }

    fn provider_policy() -> Arc<ProviderPolicySnapshot> {
        Arc::new(ProviderPolicySnapshot {
            policy_revision: 0,
            projections: Vec::new(),
        })
    }

    fn runtime_config(label: &str) -> RuntimeConfig {
        let loaded = loaded_route_graph(label);
        RuntimeConfig::new_with_config(Arc::new(loaded.source), provider_policy())
            .expect("build test runtime config")
    }

    fn assert_snapshot_label(snapshot: &RuntimeSnapshot, label: &str) {
        let codex_provider = format!("codex-{label}");
        let claude_provider = format!("claude-{label}");
        let source = snapshot.config.as_ref();
        assert!(source.codex.providers.contains_key(&codex_provider));
        assert!(source.claude.providers.contains_key(&claude_provider));

        let codex_graph = snapshot.codex_route_graph.as_ref();
        let claude_graph = snapshot.claude_route_graph.as_ref();
        assert_eq!(codex_graph.candidates()[0].provider_id, codex_provider);
        assert_eq!(claude_graph.candidates()[0].provider_id, claude_provider);
    }

    #[tokio::test]
    async fn automatic_reload_uses_the_injected_production_composition() {
        let initial_stamp =
            RuntimeSourceStamp::config(Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)));
        let changed_stamp =
            RuntimeSourceStamp::config(Some(SystemTime::UNIX_EPOCH + Duration::from_secs(2)));
        let metadata_calls = Arc::new(AtomicUsize::new(0));
        let config_calls = Arc::new(AtomicUsize::new(0));
        let pricing_calls = Arc::new(AtomicUsize::new(0));
        let pricing = capture_operator_model_price_catalog();
        let automatic_reload = RuntimeAutomaticReload {
            min_check_interval: RuntimeAutomaticReload::MIN_CHECK_INTERVAL,
            metadata: {
                let metadata_calls = Arc::clone(&metadata_calls);
                Arc::new(move || {
                    metadata_calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(std::future::ready(changed_stamp))
                })
            },
            config: {
                let config_calls = Arc::clone(&config_calls);
                Arc::new(move || {
                    config_calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(loaded_route_graph("new")) })
                })
            },
            pricing: {
                let pricing_calls = Arc::clone(&pricing_calls);
                Arc::new(move || {
                    pricing_calls.fetch_add(1, Ordering::SeqCst);
                    let pricing = pricing.clone();
                    Box::pin(async move { Ok(pricing) })
                })
            },
        };
        let runtime = RuntimeConfig::new_with_config_and_automatic_reload(
            Arc::new(route_graph_source("old")),
            provider_policy(),
            initial_stamp,
            automatic_reload,
        )
        .expect("build injected automatic reload runtime");

        assert_eq!(
            runtime.automatic_reload.min_check_interval,
            Duration::from_millis(800)
        );
        runtime.reload_check.lock().await.last_check_at = Instant::now();
        assert!(!runtime.maybe_reload_from_disk().await);
        assert_eq!(metadata_calls.load(Ordering::SeqCst), 0);
        assert_eq!(config_calls.load(Ordering::SeqCst), 0);
        assert_eq!(pricing_calls.load(Ordering::SeqCst), 0);

        runtime.reload_check.lock().await.last_check_at = Instant::now()
            .checked_sub(RuntimeAutomaticReload::MIN_CHECK_INTERVAL)
            .expect("automatic reload interval should fit in monotonic time");
        assert!(runtime.maybe_reload_from_disk().await);

        assert_eq!(metadata_calls.load(Ordering::SeqCst), 1);
        assert_eq!(config_calls.load(Ordering::SeqCst), 1);
        assert_eq!(pricing_calls.load(Ordering::SeqCst), 1);
        let snapshot = runtime.capture().await;
        assert_snapshot_label(snapshot.as_ref(), "new");
        assert_eq!(snapshot.source_stamp(), changed_stamp);
    }

    #[tokio::test]
    async fn captured_snapshot_is_all_old_or_all_new_across_reload() {
        let runtime = runtime_config("old");
        let old = runtime.capture().await;
        let old_revision = old.revision();
        let old_digest = old.digest().to_string();

        let changed = runtime
            .reload_with_source(|| async {
                Ok((
                    loaded_route_graph("new"),
                    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(2)),
                ))
            })
            .await
            .expect("reload new snapshot");
        let new = runtime.capture().await;

        assert!(changed);
        assert_snapshot_label(old.as_ref(), "old");
        assert_snapshot_label(new.as_ref(), "new");
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(new.revision(), old_revision + 1);
        assert_ne!(new.digest(), old_digest);
    }

    #[tokio::test]
    async fn reload_resets_automatic_policy_when_origin_or_continuity_identity_changes() {
        for reloaded in [
            stable_identity_source("https://new.example/v1", "continuity-a"),
            stable_identity_source("https://old.example/v1", "continuity-b"),
        ] {
            let initial = stable_identity_source("https://old.example/v1", "continuity-a");
            let (runtime, state, runtime_store) = blocked_policy_runtime(initial);
            let before = runtime.capture().await;
            assert_eq!(
                before.provider_policy().projections[0].automatic,
                crate::runtime_store::ProviderAutomaticEligibility::Blocked
            );

            let changed = runtime
                .reload_with_source(|| async { Ok((LoadedConfig { source: reloaded }, None)) })
                .await
                .expect("reload replaced runtime identity");
            assert!(changed);

            let after = runtime.capture().await;
            let projection = after
                .provider_policy()
                .projections
                .first()
                .cloned()
                .expect("reconciled policy projection");
            assert_eq!(
                projection.automatic,
                crate::runtime_store::ProviderAutomaticEligibility::Eligible
            );
            assert!(projection.active_action.is_none());
            assert!(projection.incarnation_id.is_none());
            assert_eq!(
                *state.capture_provider_policy_snapshot().await,
                runtime_store
                    .provider_policy_snapshot()
                    .expect("read durable reconciled policy")
            );
        }
    }

    #[tokio::test]
    async fn reload_resets_automatic_policy_when_upstream_credentials_change() {
        let mut initial = stable_identity_source("https://old.example/v1", "continuity-a");
        initial
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("account-a-secret".to_string()),
            ..UpstreamAuth::default()
        };
        let mut reloaded = initial.clone();
        reloaded
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("account-b-secret".to_string()),
            ..UpstreamAuth::default()
        };
        let (runtime, state, _) = blocked_policy_runtime(initial);
        state
            .set_provider_manual_eligibility(
                ProviderEndpointKey::new("codex", "stable", "default"),
                crate::runtime_store::ProviderManualEligibility::Disabled,
                Some("operator stop".to_string()),
                12,
            )
            .await
            .expect("set manual policy");

        runtime
            .reload_with_source(|| async { Ok((LoadedConfig { source: reloaded }, None)) })
            .await
            .expect("reload changed credentials");

        let projection = runtime.capture().await.provider_policy().projections[0].clone();
        assert_eq!(
            projection.automatic,
            crate::runtime_store::ProviderAutomaticEligibility::Eligible
        );
        assert_eq!(
            projection.manual,
            crate::runtime_store::ProviderManualEligibility::Disabled
        );
        assert!(projection.active_action.is_none());
    }

    #[tokio::test]
    async fn capture_publishes_newer_policy_from_durable_state_and_rejects_stale_publish() {
        let source = stable_identity_source("https://old.example/v1", "continuity-a");
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let initial_policy =
            RuntimeConfig::reconcile_initial_provider_policy(&source, runtime_store.as_ref())
                .expect("reconcile initial policy");
        let state = ProxyState::new_with_runtime_store(Arc::clone(&runtime_store))
            .expect("build proxy state");
        let runtime = RuntimeConfig::new_with_policy_state(
            Arc::new(source.clone()),
            initial_policy,
            Arc::clone(&state),
        )
        .expect("build runtime");
        let identity = compile_route_graph("codex", &source.codex)
            .expect("compile graph")
            .candidate_identities()
            .into_iter()
            .next()
            .expect("runtime identity");
        let scope = crate::runtime_store::ProviderObservationScope::new(
            identity.provider_endpoint.clone(),
            identity.base_url.as_str(),
            identity.policy_route_scope(),
            "test:runtime-policy-sync",
            "https://console.example.test/v1/usage",
            "sha256:account-a",
            "sha256:config-a",
        )
        .expect("build observation scope");
        let reservation = state
            .reserve_provider_observation(scope, 20)
            .await
            .expect("reserve observation");
        state
            .commit_provider_observation(
                reservation,
                crate::runtime_store::ProviderObservation {
                    observed_at_unix_ms: 20,
                    completed_at_unix_ms: 21,
                    authority: crate::runtime_store::ProviderObservationAuthority::Authoritative,
                    evidence: serde_json::json!({"remaining": 0}),
                    effect: crate::runtime_store::ProviderPolicyEffect::Block {
                        action_kind: "balance_exhausted".to_string(),
                        code: Some("balance_exhausted".to_string()),
                        reason: "test exhaustion".to_string(),
                        expires_at_unix_ms: None,
                    },
                },
            )
            .await
            .expect("commit observation");

        let synchronized = runtime.capture().await;
        assert_eq!(
            synchronized.provider_policy(),
            state.capture_provider_policy_snapshot().await
        );
        assert_eq!(
            synchronized.provider_policy().projections[0].automatic,
            crate::runtime_store::ProviderAutomaticEligibility::Blocked
        );

        assert!(
            !runtime
                .publish_provider_policy(Arc::new(ProviderPolicySnapshot {
                    policy_revision: 0,
                    projections: Vec::new(),
                }))
                .await
                .expect("stale publish is ignored")
        );
        assert_eq!(
            runtime.capture().await.provider_policy(),
            synchronized.provider_policy()
        );
    }

    #[tokio::test]
    async fn identity_reconciliation_failure_preserves_last_known_good_snapshot() {
        let initial = stable_identity_source("https://old.example/v1", "continuity-a");
        let reloaded = stable_identity_source("https://new.example/v1", "continuity-a");
        let (runtime, state, runtime_store) = blocked_policy_runtime(initial);
        let before = runtime.capture().await;
        let policy_before = state.capture_provider_policy_snapshot().await;
        runtime_store.fail_next_policy_commit_for_test();

        let error = runtime
            .reload_with_source(|| async { Ok((LoadedConfig { source: reloaded }, None)) })
            .await
            .expect_err("identity policy commit failure must reject reload");

        assert!(
            error.chain().any(|cause| {
                matches!(
                    cause.downcast_ref::<crate::runtime_store::RuntimeStoreError>(),
                    Some(crate::runtime_store::RuntimeStoreError::InjectedFailure {
                        operation: "reconcile runtime upstream identities",
                    })
                )
            }),
            "unexpected reconciliation error: {error:#}"
        );
        let after_failure = runtime.capture().await;
        assert!(Arc::ptr_eq(&before, &after_failure));
        assert_eq!(
            *policy_before,
            *state.capture_provider_policy_snapshot().await
        );
        assert_eq!(
            *policy_before,
            runtime_store
                .provider_policy_snapshot()
                .expect("durable LKG policy")
        );
    }

    #[tokio::test]
    async fn implicit_ordered_failover_is_compiled_into_snapshot() {
        let source = implicit_route_graph_source("implicit");
        let runtime = RuntimeConfig::new_with_config(Arc::new(source), provider_policy())
            .expect("build implicit route graph snapshot");

        let snapshot = runtime.capture().await;
        let graph = snapshot
            .route_graph("codex")
            .expect("implicit route graph must be compiled");
        assert_eq!(graph.candidates().len(), 1);
        assert_eq!(graph.candidates()[0].provider_id, "codex-implicit");
        assert!(snapshot.route_graph("claude").is_some());
    }

    #[test]
    fn snapshot_digest_is_stable_for_equivalent_canonical_config() {
        let catalog = ProviderCatalogSnapshot::bundled();
        let pricing = capture_operator_model_price_catalog();
        let first = route_graph_source("digest");
        let encoded = toml::to_string(&first).expect("encode canonical config");
        let second: HelperConfig =
            toml::from_str(encoded.as_str()).expect("decode canonical config");
        let first_codex = compile_route_graph("codex", &first.codex).expect("compile codex graph");
        let first_claude =
            compile_route_graph("claude", &first.claude).expect("compile claude graph");
        let second_codex =
            compile_route_graph("codex", &second.codex).expect("compile codex graph");
        let second_claude =
            compile_route_graph("claude", &second.claude).expect("compile claude graph");
        let policy = provider_policy();
        let first_digest = runtime_snapshot_digest(
            &first,
            &first_codex,
            &first_claude,
            &catalog,
            pricing.revision(),
            &policy,
        )
        .expect("digest first config");
        let second_digest = runtime_snapshot_digest(
            &second,
            &second_codex,
            &second_claude,
            &catalog,
            pricing.revision(),
            &policy,
        )
        .expect("digest second config");
        assert_eq!(first_digest, second_digest);
    }

    #[tokio::test]
    async fn failed_reload_keeps_lkg_and_retries_the_same_mtime() {
        let runtime = runtime_config("old");
        let before = runtime.capture().await;
        let pricing = before.operator_pricing_catalog().as_ref().clone();
        let attempted_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(7);
        let attempts = AtomicUsize::new(0);

        let first = runtime
            .maybe_reload_with(
                Duration::ZERO,
                || async { RuntimeSourceStamp::config(Some(attempted_mtime)) },
                || async {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("invalid test config")
                },
                || async { Ok(pricing.clone()) },
            )
            .await;
        assert!(first.is_err());
        let after_failure = runtime.capture().await;
        assert!(Arc::ptr_eq(&before, &after_failure));
        assert_eq!(after_failure.source_mtime(), None);

        let second = runtime
            .maybe_reload_with(
                Duration::ZERO,
                || async { RuntimeSourceStamp::config(Some(attempted_mtime)) },
                || async {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Ok(loaded_route_graph("new"))
                },
                || async { Ok(pricing) },
            )
            .await
            .expect("retry the same source mtime");

        assert!(second);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let after_success = runtime.capture().await;
        assert_snapshot_label(after_success.as_ref(), "new");
        assert_eq!(after_success.source_mtime(), Some(attempted_mtime));
    }

    #[tokio::test]
    async fn route_graph_compile_failure_does_not_publish() {
        let runtime = runtime_config("old");
        let before = runtime.capture().await;

        let error = runtime
            .reload_with_source(|| async {
                Ok((
                    invalid_loaded_route_graph(),
                    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(11)),
                ))
            })
            .await
            .expect_err("invalid route graph must fail before publication");

        assert!(error.to_string().contains("compile codex route graph"));
        let after = runtime.capture().await;
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.source_mtime(), None);
        assert_snapshot_label(after.as_ref(), "old");
    }

    #[tokio::test]
    async fn slow_reload_source_does_not_block_policy_publication() {
        let runtime = Arc::new(runtime_config("old"));
        let before = runtime.capture().await;
        let (source_started_tx, source_started_rx) = oneshot::channel();
        let (release_source_tx, release_source_rx) = oneshot::channel();
        let reload_runtime = Arc::clone(&runtime);
        let reload = tokio::spawn(async move {
            reload_runtime
                .reload_with_source(|| async move {
                    let _ = source_started_tx.send(());
                    release_source_rx.await.expect("release slow source");
                    Ok((loaded_route_graph("new"), None))
                })
                .await
        });
        source_started_rx.await.expect("slow source started");

        let published = tokio::time::timeout(
            Duration::from_millis(200),
            runtime.publish_provider_policy(Arc::new(ProviderPolicySnapshot {
                policy_revision: 1,
                projections: Vec::new(),
            })),
        )
        .await
        .expect("policy publish must not wait for source loading")
        .expect("publish policy");
        assert!(published);
        let after_policy = runtime.capture().await;
        assert!(Arc::ptr_eq(&before.config(), &after_policy.config()));
        assert!(Arc::ptr_eq(
            &before.route_graph("codex").expect("old codex graph"),
            &after_policy
                .route_graph("codex")
                .expect("policy codex graph")
        ));
        assert!(Arc::ptr_eq(
            &before.route_graph("claude").expect("old claude graph"),
            &after_policy
                .route_graph("claude")
                .expect("policy claude graph")
        ));
        assert!(Arc::ptr_eq(
            &before.provider_catalog(),
            &after_policy.provider_catalog()
        ));
        assert!(Arc::ptr_eq(
            &before.operator_pricing_catalog(),
            &after_policy.operator_pricing_catalog()
        ));

        release_source_tx.send(()).expect("release source");
        assert!(reload.await.expect("join reload").expect("reload succeeds"));
        let after_reload = runtime.capture().await;
        assert_snapshot_label(after_reload.as_ref(), "new");
        assert_eq!(after_reload.provider_policy().policy_revision, 1);
    }

    #[tokio::test]
    async fn later_reload_ticket_wins_when_an_older_builder_finishes_last() {
        let runtime = Arc::new(runtime_config("old"));
        let (first_started_tx, first_started_rx) = oneshot::channel();
        let (release_first_tx, release_first_rx) = oneshot::channel();
        let first_runtime = Arc::clone(&runtime);
        let first = tokio::spawn(async move {
            first_runtime
                .reload_with_source(|| async move {
                    let _ = first_started_tx.send(());
                    release_first_rx.await.expect("release first builder");
                    Ok((loaded_route_graph("first"), None))
                })
                .await
        });
        first_started_rx.await.expect("first builder started");

        assert!(
            runtime
                .reload_with_source(|| async { Ok((loaded_route_graph("second"), None)) })
                .await
                .expect("second reload")
        );
        release_first_tx.send(()).expect("release first builder");
        assert!(
            !first
                .await
                .expect("join first reload")
                .expect("stale reload is ignored")
        );

        assert_snapshot_label(runtime.capture().await.as_ref(), "second");
    }

    #[tokio::test]
    async fn pricing_source_change_is_checked_and_published_without_config_change() {
        let runtime = runtime_config("old");
        let before = runtime.capture().await;
        let pricing = before.operator_pricing_catalog().as_ref().clone();
        let config_loads = AtomicUsize::new(0);
        let pricing_loads = AtomicUsize::new(0);
        let pricing_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(31);

        let changed = runtime
            .maybe_reload_with(
                Duration::ZERO,
                || async {
                    RuntimeSourceStamp {
                        config_mtime: None,
                        pricing_mtime: Some(pricing_mtime),
                    }
                },
                || async {
                    config_loads.fetch_add(1, Ordering::SeqCst);
                    Ok(loaded_route_graph("old"))
                },
                || async {
                    pricing_loads.fetch_add(1, Ordering::SeqCst);
                    Ok(pricing)
                },
            )
            .await
            .expect("pricing-only reload");

        assert!(!changed);
        assert_eq!(config_loads.load(Ordering::SeqCst), 1);
        assert_eq!(pricing_loads.load(Ordering::SeqCst), 1);
        let after = runtime.capture().await;
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.source_stamp().pricing_mtime, Some(pricing_mtime));
    }

    #[tokio::test]
    async fn pricing_build_failure_preserves_the_complete_lkg_snapshot() {
        let runtime = runtime_config("old");
        let before = runtime.capture().await;

        let error = runtime
            .reload_with_source_and_pricing(
                || async {
                    Ok((
                        loaded_route_graph("new"),
                        RuntimeSourceStamp {
                            config_mtime: None,
                            pricing_mtime: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(41)),
                        },
                    ))
                },
                || async { anyhow::bail!("invalid pricing override") },
            )
            .await
            .expect_err("invalid pricing must fail before publication");

        assert!(error.to_string().contains("invalid pricing override"));
        assert!(Arc::ptr_eq(&before, &runtime.capture().await));
    }

    #[tokio::test]
    async fn basellm_publish_advances_only_the_pricing_runtime_generation() {
        let runtime = runtime_config("pricing-publish");
        let before = runtime.capture().await;
        let replacement = Arc::new(
            before
                .operator_pricing_catalog()
                .as_ref()
                .clone()
                .with_test_revision("test:basellm-runtime-revision"),
        );

        assert!(
            runtime
                .publish_captured_operator_pricing_catalog(Arc::clone(&replacement))
                .await
                .expect("publish BaseLLM pricing")
        );

        let after = runtime.capture().await;
        assert_eq!(after.revision(), before.revision().saturating_add(1));
        assert_eq!(
            after.operator_pricing_catalog().revision(),
            replacement.revision()
        );
        assert!(Arc::ptr_eq(&before.config(), &after.config()));
        assert!(Arc::ptr_eq(
            &before.provider_catalog(),
            &after.provider_catalog()
        ));
        assert_eq!(before.provider_policy(), after.provider_policy());
        assert!(
            !runtime
                .publish_captured_operator_pricing_catalog(replacement)
                .await
                .expect("skip duplicate BaseLLM pricing")
        );
    }
}
