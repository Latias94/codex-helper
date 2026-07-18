use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify, watch};
use tokio::time::Instant as TokioInstant;
use tracing::warn;

#[cfg(test)]
use crate::config::ServiceRouteConfig;
use crate::config::{HelperConfig, LoadedConfig};
use crate::credentials::{
    CapturedUpstreamCredential, CredentialCandidateInput, CredentialGeneration,
    CredentialGenerationMarker, CredentialHandle, CredentialName, CredentialRuntime,
    CredentialRuntimeRefreshCause, CredentialSourceCapabilities,
};
use crate::logging::log_control_trace_event;
use crate::pricing::{CapturedModelPriceCatalog, try_capture_operator_model_price_catalog};
use crate::provider_catalog::ProviderCatalogSnapshot;
use crate::routing_ir::{CompiledRouteGraph, RoutePlanTemplate, RouteRequestContext};
use crate::runtime_identity::{RuntimeUpstreamIdentity, diff_runtime_upstream_identities};
use crate::runtime_store::{ProviderPolicySnapshot, RuntimeStore};
use crate::state::{PreparedRoutingOperatorRouteGraph, ProxyState};
use crate::usage_providers::{
    credential_generation_catalog, usage_provider_source_revision_from_disk,
};

const AUTH_FAILURE_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(5);

struct TransientRuntimeConfig(HelperConfig);

impl Deref for TransientRuntimeConfig {
    type Target = HelperConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TransientRuntimeConfig {
    fn drop(&mut self) {
        self.0.zeroize_inline_credentials();
    }
}

struct TransientRouteGraph(CompiledRouteGraph);

impl Deref for TransientRouteGraph {
    type Target = CompiledRouteGraph;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TransientRouteGraph {
    fn drop(&mut self) {
        self.0.zeroize_inline_credentials();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RuntimeSourceStamp {
    config_mtime: Option<SystemTime>,
    pricing_mtime: Option<SystemTime>,
    usage_providers_revision: [u8; 32],
}

impl RuntimeSourceStamp {
    #[cfg(test)]
    fn config(config_mtime: Option<SystemTime>) -> Self {
        Self {
            config_mtime,
            pricing_mtime: None,
            usage_providers_revision: [0; 32],
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
    credential_generation: Arc<CredentialGeneration>,
    digest_seed: Sha256,
    loaded_at_ms: u64,
    source_stamp: RuntimeSourceStamp,
}

impl PreparedRuntimeSnapshot {
    fn build(
        config: Arc<HelperConfig>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        source_stamp: RuntimeSourceStamp,
        credential_runtime: &CredentialRuntime,
    ) -> Result<Self> {
        Self::build_with_previous(
            config,
            operator_pricing_catalog,
            source_stamp,
            credential_runtime,
            None,
        )
    }

    fn build_from_previous(
        config: Arc<HelperConfig>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        source_stamp: RuntimeSourceStamp,
        credential_runtime: &CredentialRuntime,
        previous_generation: &CredentialGeneration,
    ) -> Result<Self> {
        Self::build_with_previous(
            config,
            operator_pricing_catalog,
            source_stamp,
            credential_runtime,
            Some(previous_generation),
        )
    }

    fn build_with_previous(
        config: Arc<HelperConfig>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        source_stamp: RuntimeSourceStamp,
        credential_runtime: &CredentialRuntime,
        previous_generation: Option<&CredentialGeneration>,
    ) -> Result<Self> {
        let source_config = TransientRuntimeConfig(match Arc::try_unwrap(config) {
            Ok(config) => config,
            Err(config) => config.as_ref().clone(),
        });
        let source_codex_route_graph = TransientRouteGraph(
            CompiledRouteGraph::compile("codex", &source_config.codex)
                .context("compile codex route graph for credential ingestion")?,
        );
        let source_claude_route_graph = TransientRouteGraph(
            CompiledRouteGraph::compile("claude", &source_config.claude)
                .context("compile claude route graph for credential ingestion")?,
        );
        let candidates = || {
            source_codex_route_graph
                .candidates()
                .iter()
                .map(|candidate| CredentialCandidateInput {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex",
                        candidate.provider_id.clone(),
                        candidate.endpoint_id.clone(),
                    ),
                    auth: &candidate.auth,
                })
                .chain(
                    source_claude_route_graph
                        .candidates()
                        .iter()
                        .map(|candidate| CredentialCandidateInput {
                            provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                                "claude",
                                candidate.provider_id.clone(),
                                candidate.endpoint_id.clone(),
                            ),
                            auth: &candidate.auth,
                        }),
                )
        };
        let (named_credentials, named_catalog_revision) =
            credential_generation_catalog().into_parts();
        let credential_generation = match previous_generation {
            Some(previous) => credential_runtime.build_generation_from_previous_with_named(
                candidates(),
                named_credentials,
                named_catalog_revision.as_str(),
                previous,
            )?,
            None => credential_runtime.build_generation_with_named(
                candidates(),
                named_credentials,
                named_catalog_revision.as_str(),
            )?,
        };
        let config = Arc::new(redacted_runtime_config(source_config.0.clone()));
        let codex_route_graph = Arc::new(
            CompiledRouteGraph::compile("codex", &config.codex)
                .context("compile codex route graph for runtime snapshot")?
                .with_credential_generation(
                    Arc::clone(&credential_generation),
                    source_codex_route_graph.digest().to_string(),
                )?,
        );
        let claude_route_graph = Arc::new(
            CompiledRouteGraph::compile("claude", &config.claude)
                .context("compile claude route graph for runtime snapshot")?
                .with_credential_generation(
                    Arc::clone(&credential_generation),
                    source_claude_route_graph.digest().to_string(),
                )?,
        );
        let provider_catalog = Arc::new(ProviderCatalogSnapshot::bundled());
        let digest_seed = runtime_snapshot_digest_seed(
            config.as_ref(),
            codex_route_graph.as_ref(),
            claude_route_graph.as_ref(),
            provider_catalog.as_ref(),
            operator_pricing_catalog.revision(),
            credential_generation.as_ref(),
        )?;

        Ok(Self {
            config,
            codex_route_graph,
            claude_route_graph,
            provider_catalog,
            operator_pricing_catalog,
            credential_generation,
            digest_seed,
            loaded_at_ms: now_ms(),
            source_stamp,
        })
    }

    fn candidate_identities(&self) -> Result<Vec<RuntimeUpstreamIdentity>> {
        let mut identities = self.codex_route_graph.candidate_identities()?;
        identities.extend(self.claude_route_graph.candidate_identities()?);
        Ok(identities)
    }

    fn with_refreshed_credentials(
        snapshot: &RuntimeSnapshot,
        credential_generation: Arc<CredentialGeneration>,
    ) -> Result<Self> {
        let codex_route_graph = Arc::new(
            snapshot
                .codex_route_graph
                .rebound_credential_generation(Arc::clone(&credential_generation))?,
        );
        let claude_route_graph = Arc::new(
            snapshot
                .claude_route_graph
                .rebound_credential_generation(Arc::clone(&credential_generation))?,
        );
        let digest_seed = runtime_snapshot_digest_seed(
            snapshot.config.as_ref(),
            codex_route_graph.as_ref(),
            claude_route_graph.as_ref(),
            snapshot.provider_catalog.as_ref(),
            snapshot.operator_pricing_catalog.revision(),
            credential_generation.as_ref(),
        )?;
        Ok(Self {
            config: Arc::clone(&snapshot.config),
            codex_route_graph,
            claude_route_graph,
            provider_catalog: Arc::clone(&snapshot.provider_catalog),
            operator_pricing_catalog: Arc::clone(&snapshot.operator_pricing_catalog),
            credential_generation,
            digest_seed,
            loaded_at_ms: now_ms(),
            source_stamp: snapshot.source_stamp,
        })
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
            credential_generation: self.credential_generation,
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
    credential_generation: Arc<CredentialGeneration>,
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
        let provider_endpoint_candidates = template
            .candidates
            .iter()
            .map(|candidate| {
                let identity = template.candidate_identity(candidate)?;
                Ok(serde_json::json!({
                    "provider_id": candidate.provider_id,
                    "endpoint_id": candidate.endpoint_id,
                    "provider_endpoint_key": identity.provider_endpoint.stable_key(),
                    "preference_group": candidate.preference_group,
                    "route_path": candidate.route_path,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        log_control_trace_event(serde_json::json!({
            "event": "route_plan_selected",
            "service": template.service_name,
            "entry": template.entry,
            "route_graph_key": template.route_graph_key(),
            "runtime_revision": self.revision,
            "candidate_count": template.candidates.len(),
            "provider_endpoint_candidates": provider_endpoint_candidates,
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
            credential_generation: Arc::clone(&self.credential_generation),
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
            self.credential_generation.as_ref(),
        )?;
        let digest = runtime_snapshot_digest_from_seed(digest_seed.clone(), &self.provider_policy);
        Ok(Self {
            config: Arc::clone(&self.config),
            codex_route_graph: Arc::clone(&self.codex_route_graph),
            claude_route_graph: Arc::clone(&self.claude_route_graph),
            provider_catalog: Arc::clone(&self.provider_catalog),
            operator_pricing_catalog,
            credential_generation: Arc::clone(&self.credential_generation),
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

    pub(super) fn credential_generation(&self) -> Arc<CredentialGeneration> {
        Arc::clone(&self.credential_generation)
    }

    pub(super) fn provider_policy(&self) -> Arc<ProviderPolicySnapshot> {
        Arc::clone(&self.provider_policy)
    }

    fn candidate_identities(&self) -> Result<Vec<RuntimeUpstreamIdentity>> {
        let mut identities = self.codex_route_graph.candidate_identities()?;
        identities.extend(self.claude_route_graph.candidate_identities()?);
        Ok(identities)
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
    credential_runtime: CredentialRuntime,
    pending_credential_refresh:
        Mutex<BTreeMap<CredentialGenerationMarker, BTreeSet<CredentialHandle>>>,
    recent_auth_refresh:
        Mutex<BTreeMap<(CredentialGenerationMarker, CredentialHandle), TokioInstant>>,
    credential_refresh_notify: Notify,
    #[cfg(test)]
    credential_refresh_wait_epoch: AtomicU64,
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

enum CredentialRefreshDriverWork {
    Scheduled,
    AuthenticationFailure(BTreeMap<CredentialGenerationMarker, BTreeSet<CredentialHandle>>),
    Wait(Option<Instant>),
}

enum PreparedRuntimePublishOutcome {
    Published { changed: bool },
    Stale,
}

impl RuntimeConfig {
    pub(super) fn new_with_runtime_store_and_credential_sources(
        initial_config: Arc<HelperConfig>,
        runtime_store: Arc<RuntimeStore>,
        credential_sources: CredentialSourceCapabilities,
    ) -> Result<(Self, Arc<ProxyState>)> {
        let credential_runtime =
            CredentialRuntime::from_runtime_store(credential_sources, runtime_store.as_ref())?;
        let operator_pricing_catalog = try_capture_operator_model_price_catalog()
            .map(Arc::new)
            .map_err(anyhow::Error::msg)
            .context("load initial operator pricing catalog")?;
        let source_stamp = runtime_source_stamp_from_disk_sync();
        let prepared = PreparedRuntimeSnapshot::build(
            initial_config,
            operator_pricing_catalog,
            source_stamp,
            &credential_runtime,
        )?;
        let provider_policy = Arc::new(
            runtime_store
                .reconcile_runtime_upstream_identities(&prepared.candidate_identities()?, now_ms())
                .context("reconcile initial runtime upstream identities")?,
        );
        let state = ProxyState::new_with_runtime_store(runtime_store)?;
        let initial = prepared.finish(provider_policy, 1);
        #[cfg(not(test))]
        let automatic_reload = RuntimeAutomaticReload::disk();
        #[cfg(test)]
        let automatic_reload = RuntimeAutomaticReload::unchanged(source_stamp);
        let runtime = Self {
            current: RwLock::new(Arc::new(initial)),
            credential_runtime,
            pending_credential_refresh: Mutex::new(BTreeMap::new()),
            recent_auth_refresh: Mutex::new(BTreeMap::new()),
            credential_refresh_notify: Notify::new(),
            #[cfg(test)]
            credential_refresh_wait_epoch: AtomicU64::new(0),
            reload_check: AsyncMutex::new(RuntimeConfigReloadCheckState {
                last_check_at: Instant::now()
                    .checked_sub(Duration::from_secs(60))
                    .unwrap_or_else(Instant::now),
            }),
            publish: AsyncMutex::new(RuntimeConfigPublishState::default()),
            next_build_ticket: AtomicU64::new(1),
            policy_state: Some(Arc::clone(&state)),
            automatic_reload,
        };
        Ok((runtime, state))
    }

    #[cfg(test)]
    pub(super) fn new_with_config(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
    ) -> Result<Self> {
        Self::new_inner(initial_config, provider_policy, None)
    }

    #[cfg(test)]
    pub(super) fn new_with_policy_state(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
        policy_state: Arc<ProxyState>,
    ) -> Result<Self> {
        Self::new_inner(initial_config, provider_policy, Some(policy_state))
    }

    #[cfg(test)]
    fn new_inner(
        initial_config: Arc<HelperConfig>,
        provider_policy: Arc<ProviderPolicySnapshot>,
        policy_state: Option<Arc<ProxyState>>,
    ) -> Result<Self> {
        let credential_runtime = match policy_state.as_ref() {
            Some(state) => CredentialRuntime::from_runtime_store(
                CredentialSourceCapabilities::server(),
                state.runtime_store(),
            )?,
            None => {
                let runtime_store = RuntimeStore::open_in_memory()
                    .context("open isolated runtime credential store")?;
                CredentialRuntime::from_runtime_store(
                    CredentialSourceCapabilities::server(),
                    &runtime_store,
                )?
            }
        };
        let operator_pricing_catalog = try_capture_operator_model_price_catalog()
            .map(Arc::new)
            .map_err(anyhow::Error::msg)
            .context("load initial operator pricing catalog")?;
        let source_stamp = runtime_source_stamp_from_disk_sync();
        let initial = PreparedRuntimeSnapshot::build(
            initial_config,
            operator_pricing_catalog,
            source_stamp,
            &credential_runtime,
        )?
        .finish(provider_policy, 1);
        #[cfg(not(test))]
        let automatic_reload = RuntimeAutomaticReload::disk();
        #[cfg(test)]
        let automatic_reload = RuntimeAutomaticReload::unchanged(source_stamp);
        Ok(Self {
            current: RwLock::new(Arc::new(initial)),
            credential_runtime,
            pending_credential_refresh: Mutex::new(BTreeMap::new()),
            recent_auth_refresh: Mutex::new(BTreeMap::new()),
            credential_refresh_notify: Notify::new(),
            credential_refresh_wait_epoch: AtomicU64::new(0),
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
        let runtime_store = RuntimeStore::open_in_memory().context("open test credential store")?;
        let credential_runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            &runtime_store,
        )?;
        let operator_pricing_catalog = try_capture_operator_model_price_catalog()
            .map(Arc::new)
            .map_err(anyhow::Error::msg)
            .context("load initial operator pricing catalog")?;
        let initial = PreparedRuntimeSnapshot::build(
            initial_config,
            operator_pricing_catalog,
            source_stamp,
            &credential_runtime,
        )?
        .finish(provider_policy, 1);
        Ok(Self {
            current: RwLock::new(Arc::new(initial)),
            credential_runtime,
            pending_credential_refresh: Mutex::new(BTreeMap::new()),
            recent_auth_refresh: Mutex::new(BTreeMap::new()),
            credential_refresh_notify: Notify::new(),
            credential_refresh_wait_epoch: AtomicU64::new(0),
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

    #[cfg(test)]
    pub(super) fn reconcile_initial_provider_policy(
        config: &HelperConfig,
        runtime_store: &RuntimeStore,
    ) -> Result<Arc<ProviderPolicySnapshot>> {
        let credential_runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            runtime_store,
        )?;
        let source_codex = compile_route_graph("codex", &config.codex)?;
        let source_claude = compile_route_graph("claude", &config.claude)?;
        let generation = credential_runtime.build_generation(
            source_codex
                .candidates()
                .iter()
                .map(|candidate| CredentialCandidateInput {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex",
                        candidate.provider_id.clone(),
                        candidate.endpoint_id.clone(),
                    ),
                    auth: &candidate.auth,
                })
                .chain(source_claude.candidates().iter().map(|candidate| {
                    CredentialCandidateInput {
                        provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                            "claude",
                            candidate.provider_id.clone(),
                            candidate.endpoint_id.clone(),
                        ),
                        auth: &candidate.auth,
                    }
                })),
        )?;
        let runtime_config = redacted_runtime_config(config.clone());
        let codex_route_graph = Arc::new(
            CompiledRouteGraph::compile("codex", &runtime_config.codex)?
                .with_credential_generation(
                    Arc::clone(&generation),
                    source_codex.digest().into(),
                )?,
        );
        let claude_route_graph = Arc::new(
            CompiledRouteGraph::compile("claude", &runtime_config.claude)?
                .with_credential_generation(generation, source_claude.digest().into())?,
        );
        let mut identities = codex_route_graph.candidate_identities()?;
        identities.extend(claude_route_graph.candidate_identities()?);
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

    pub(super) fn automatic_reload_check_interval(&self) -> Duration {
        self.automatic_reload.min_check_interval
    }

    pub(super) async fn snapshot(&self) -> Arc<HelperConfig> {
        self.capture().await.config()
    }

    pub(super) fn schedule_credential_refresh(&self, credential: &CapturedUpstreamCredential) {
        self.schedule_credential_refresh_at(credential, TokioInstant::now());
    }

    fn schedule_credential_refresh_at(
        &self,
        credential: &CapturedUpstreamCredential,
        now: TokioInstant,
    ) {
        let handles = credential.refresh_handles();
        if handles.is_empty() {
            return;
        }
        let marker = credential.generation_marker();
        let current_marker = self.capture_current().credential_generation.marker();
        if marker != current_marker {
            return;
        }
        let mut recent = self
            .recent_auth_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.retain(|(generation, _), _| generation == &current_marker);
        let eligible = handles
            .iter()
            .filter(|handle| {
                recent
                    .get(&(marker.clone(), (*handle).clone()))
                    .is_none_or(|attempted_at| {
                        now.saturating_duration_since(*attempted_at)
                            >= AUTH_FAILURE_REFRESH_MIN_INTERVAL
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(recent);
        if eligible.is_empty() {
            return;
        }
        self.pending_credential_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(marker)
            .or_default()
            .extend(eligible);
        self.credential_refresh_notify.notify_one();
    }

    fn take_pending_credential_refresh(
        &self,
    ) -> BTreeMap<CredentialGenerationMarker, BTreeSet<CredentialHandle>> {
        let mut pending = self
            .pending_credential_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *pending)
    }

    async fn refresh_pending_credentials(
        &self,
        pending: BTreeMap<CredentialGenerationMarker, BTreeSet<CredentialHandle>>,
    ) -> Result<bool> {
        let current = self.capture_current();
        let generation = current.credential_generation();
        let marker = generation.marker();
        let handles = pending
            .into_values()
            .flatten()
            .filter(|handle| generation.contains_handle(handle))
            .collect::<BTreeSet<_>>();
        if handles.is_empty() {
            return Ok(false);
        }
        {
            let now = TokioInstant::now();
            let mut recent = self
                .recent_auth_refresh
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            recent.retain(|(generation, _), _| generation == &marker);
            recent.extend(
                handles
                    .iter()
                    .cloned()
                    .map(|handle| ((marker.clone(), handle), now)),
            );
        }
        self.refresh_credentials(
            Some(handles.into_iter().collect::<Vec<_>>().into()),
            CredentialRuntimeRefreshCause::AuthenticationFailure,
            Some(marker),
        )
        .await
    }

    // U9 exposes these operations only through the signed local operator boundary.
    #[allow(dead_code)]
    pub(super) async fn refresh_native_credential_after_upsert(
        &self,
        name: &CredentialName,
    ) -> Result<bool> {
        self.publish_named_native_credential(name, CredentialRuntimeRefreshCause::ExplicitRefresh)
            .await
    }

    #[allow(dead_code)]
    pub(super) async fn invalidate_deleted_native_credential(
        &self,
        name: &CredentialName,
    ) -> Result<bool> {
        self.publish_named_native_credential(name, CredentialRuntimeRefreshCause::ExplicitDelete)
            .await
    }

    async fn publish_named_native_credential(
        &self,
        name: &CredentialName,
        cause: CredentialRuntimeRefreshCause,
    ) -> Result<bool> {
        loop {
            let previous = self.capture_current();
            let generation = previous.credential_generation();
            let handles = generation.native_handles_for_name(name);
            if handles.is_empty() {
                return Ok(false);
            }
            let ticket = self.reserve_build_ticket();
            let next_generation = self
                .credential_runtime
                .refresh_generation(generation, Some(handles), cause)
                .await?;
            let prepared =
                PreparedRuntimeSnapshot::with_refreshed_credentials(&previous, next_generation)?;
            match self
                .publish_prepared_with_expected(ticket, prepared, Some(&previous))
                .await?
            {
                PreparedRuntimePublishOutcome::Published { changed } => return Ok(changed),
                PreparedRuntimePublishOutcome::Stale => continue,
            }
        }
    }

    fn take_credential_refresh_driver_work(&self) -> CredentialRefreshDriverWork {
        let deadline = self
            .capture_current()
            .credential_generation
            .next_native_deadline();
        if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
            return CredentialRefreshDriverWork::Scheduled;
        }
        let pending = self.take_pending_credential_refresh();
        if pending.is_empty() {
            CredentialRefreshDriverWork::Wait(deadline)
        } else {
            CredentialRefreshDriverWork::AuthenticationFailure(pending)
        }
    }

    async fn refresh_credentials(
        &self,
        requested: Option<Arc<[CredentialHandle]>>,
        cause: CredentialRuntimeRefreshCause,
        expected_generation: Option<CredentialGenerationMarker>,
    ) -> Result<bool> {
        let previous = self.capture_current();
        if expected_generation
            .as_ref()
            .is_some_and(|marker| !marker.matches(previous.credential_generation.as_ref()))
        {
            return Ok(false);
        }
        let ticket = self.reserve_build_ticket();
        let next_generation = self
            .credential_runtime
            .refresh_generation(previous.credential_generation(), requested.clone(), cause)
            .await?;
        if Arc::ptr_eq(&next_generation, &previous.credential_generation) {
            return Ok(false);
        }
        let prepared =
            PreparedRuntimeSnapshot::with_refreshed_credentials(&previous, next_generation)?;
        match self
            .publish_prepared_with_expected(ticket, prepared, Some(&previous))
            .await?
        {
            PreparedRuntimePublishOutcome::Published { changed } => Ok(changed),
            PreparedRuntimePublishOutcome::Stale => {
                let current = self.capture_current();
                if let (Some(requested), Some(expected_generation)) =
                    (requested, expected_generation)
                    && expected_generation.matches(current.credential_generation.as_ref())
                {
                    self.pending_credential_refresh
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .entry(expected_generation)
                        .or_default()
                        .extend(requested.iter().cloned());
                    self.credential_refresh_notify.notify_one();
                }
                Ok(false)
            }
        }
    }

    pub(super) async fn run_credential_refresh_driver(
        &self,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        loop {
            if *shutdown_rx.borrow() {
                return;
            }

            match self.take_credential_refresh_driver_work() {
                CredentialRefreshDriverWork::Scheduled => {
                    let refresh = self.refresh_credentials(
                        None,
                        CredentialRuntimeRefreshCause::Scheduled,
                        None,
                    );
                    match await_refresh_or_shutdown(&mut shutdown_rx, refresh).await {
                        None => return,
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            warn!(error = %error, "scheduled credential refresh failed");
                        }
                    }
                }
                CredentialRefreshDriverWork::AuthenticationFailure(requested) => {
                    let refresh = self.refresh_pending_credentials(requested);
                    match await_refresh_or_shutdown(&mut shutdown_rx, refresh).await {
                        None => return,
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            warn!(error = %error, "credential refresh after upstream auth failure failed");
                        }
                    }
                }
                CredentialRefreshDriverWork::Wait(Some(deadline)) => {
                    #[cfg(test)]
                    self.credential_refresh_wait_epoch
                        .fetch_add(1, Ordering::SeqCst);
                    tokio::select! {
                        biased;
                        _ = wait_for_shutdown(&mut shutdown_rx) => return,
                        _ = self.credential_refresh_notify.notified() => continue,
                        () = tokio::time::sleep_until(TokioInstant::from_std(deadline)) => {}
                    }
                }
                CredentialRefreshDriverWork::Wait(None) => {
                    #[cfg(test)]
                    self.credential_refresh_wait_epoch
                        .fetch_add(1, Ordering::SeqCst);
                    tokio::select! {
                        biased;
                        _ = wait_for_shutdown(&mut shutdown_rx) => return,
                        _ = self.credential_refresh_notify.notified() => {}
                    }
                }
            }
        }
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
        let previous_generation = self.capture_current().credential_generation();
        let prepared = prepare_runtime_snapshot(
            loaded,
            source_stamp,
            operator_pricing_catalog,
            self.credential_runtime.clone(),
            previous_generation,
        )
        .await?;
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
        let previous_generation = self.capture_current().credential_generation();
        let prepared = prepare_runtime_snapshot(
            loaded,
            source_stamp,
            operator_pricing_catalog,
            self.credential_runtime.clone(),
            previous_generation,
        )
        .await?;
        self.publish_prepared(ticket, prepared).await
    }

    async fn publish_prepared(
        &self,
        ticket: u64,
        prepared: PreparedRuntimeSnapshot,
    ) -> Result<bool> {
        match self
            .publish_prepared_with_expected(ticket, prepared, None)
            .await?
        {
            PreparedRuntimePublishOutcome::Published { changed } => Ok(changed),
            PreparedRuntimePublishOutcome::Stale => Ok(false),
        }
    }

    async fn publish_prepared_with_expected(
        &self,
        ticket: u64,
        prepared: PreparedRuntimeSnapshot,
        expected: Option<&Arc<RuntimeSnapshot>>,
    ) -> Result<PreparedRuntimePublishOutcome> {
        let mut publish_state = self.publish.lock().await;
        if ticket <= publish_state.highest_publish_ticket {
            return Ok(PreparedRuntimePublishOutcome::Stale);
        }

        let previous = self.capture_current();
        if expected.is_some_and(|expected| !Arc::ptr_eq(expected, &previous)) {
            return Ok(PreparedRuntimePublishOutcome::Stale);
        }
        publish_state.highest_publish_ticket = ticket;
        let next_identities = prepared.candidate_identities()?;
        let routing_control_graphs = [
            PreparedRoutingOperatorRouteGraph::new(
                "codex",
                prepared.codex_route_graph.digest().to_string(),
            )
            .context("prepare codex routing operator control reconciliation")?,
            PreparedRoutingOperatorRouteGraph::new(
                "claude",
                prepared.claude_route_graph.digest().to_string(),
            )
            .context("prepare claude routing operator control reconciliation")?,
        ];
        let identity_delta =
            diff_runtime_upstream_identities(&previous.candidate_identities()?, &next_identities);
        if let Some(policy_state) = self.policy_state.as_ref() {
            let reconcile_identities =
                !identity_delta.added.is_empty() || !identity_delta.removed.is_empty();
            let (_, outcome) = policy_state
                .commit_runtime_reload(
                    &routing_control_graphs,
                    &next_identities,
                    reconcile_identities,
                    now_ms(),
                    |provider_policy| {
                        let mut next = prepared.finish(provider_policy, previous.revision());
                        let changed = next.digest() != previous.digest();
                        if changed {
                            next.revision = previous.revision().saturating_add(1);
                        }
                        self.store_current(next);
                        PreparedRuntimePublishOutcome::Published { changed }
                    },
                )
                .await
                .context("commit reloaded runtime identities and snapshot")?;
            Ok(outcome)
        } else {
            let mut next = prepared.finish(previous.provider_policy(), previous.revision());
            let changed = next.digest() != previous.digest();
            if changed {
                next.revision = previous.revision().saturating_add(1);
            }
            self.store_current(next);
            Ok(PreparedRuntimePublishOutcome::Published { changed })
        }
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
    let (config_mtime, pricing_mtime, usage_providers_revision) = tokio::join!(
        source_mtime(crate::config::config_file_path()),
        source_mtime(crate::pricing::model_price_overrides_path()),
        tokio::task::spawn_blocking(usage_provider_source_revision_from_disk),
    );
    RuntimeSourceStamp {
        config_mtime,
        pricing_mtime,
        usage_providers_revision: usage_providers_revision.unwrap_or([0; 32]),
    }
}

fn runtime_source_stamp_from_disk_sync() -> RuntimeSourceStamp {
    RuntimeSourceStamp {
        config_mtime: source_mtime_sync(crate::config::config_file_path()),
        pricing_mtime: source_mtime_sync(crate::pricing::model_price_overrides_path()),
        usage_providers_revision: usage_provider_source_revision_from_disk(),
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

async fn await_refresh_or_shutdown<F>(
    shutdown_rx: &mut watch::Receiver<bool>,
    refresh: F,
) -> Option<Result<bool>>
where
    F: Future<Output = Result<bool>>,
{
    tokio::select! {
        biased;
        _ = wait_for_shutdown(shutdown_rx) => None,
        result = refresh => Some(result),
    }
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() || shutdown_rx.changed().await.is_err() {
            return;
        }
    }
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
    credential_runtime: CredentialRuntime,
    previous_generation: Arc<CredentialGeneration>,
) -> Result<PreparedRuntimeSnapshot> {
    tokio::task::spawn_blocking(move || {
        PreparedRuntimeSnapshot::build_from_previous(
            Arc::new(loaded.source),
            Arc::new(operator_pricing_catalog),
            source_stamp,
            &credential_runtime,
            previous_generation.as_ref(),
        )
    })
    .await
    .context("join runtime snapshot builder")?
}

#[cfg(test)]
fn compile_route_graph(
    service_name: &str,
    service: &ServiceRouteConfig,
) -> Result<Arc<CompiledRouteGraph>> {
    CompiledRouteGraph::compile(service_name, service)
        .with_context(|| format!("compile {service_name} route graph for runtime snapshot"))
        .map(Arc::new)
}

fn redacted_runtime_config(mut config: HelperConfig) -> HelperConfig {
    config.zeroize_inline_credentials();
    config
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
        CredentialGeneration::empty().as_ref(),
    )?;
    Ok(runtime_snapshot_digest_from_seed(seed, provider_policy))
}

fn runtime_snapshot_digest_seed(
    config: &HelperConfig,
    codex_route_graph: &CompiledRouteGraph,
    claude_route_graph: &CompiledRouteGraph,
    provider_catalog: &ProviderCatalogSnapshot,
    operator_pricing_revision: &str,
    credential_generation: &CredentialGeneration,
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
    hash_digest_part(&mut hasher, credential_generation.digest().as_bytes());
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

    use axum::http::{HeaderMap, header};

    use super::*;
    use crate::config::{
        CURRENT_CONFIG_VERSION, CredentialRef, ProviderConfig, RouteGraphConfig, SchedulingPreset,
        UpstreamAuth,
    };
    use crate::credentials::{CredentialName, SecretValue};
    use crate::pricing::capture_operator_model_price_catalog;
    use crate::runtime_identity::ProviderEndpointKey;
    use tokio::sync::oneshot;

    fn assert_secret_canary_absent(surface: &str, rendered: &str, canary: &str) {
        let raw_sha256 = format!("{:x}", sha2::Sha256::digest(canary.as_bytes()));
        for forbidden in [
            canary.to_string(),
            format!("Bearer {canary}"),
            canary[..16].to_string(),
            raw_sha256,
        ] {
            assert!(
                !rendered.contains(&forbidden),
                "{surface} leaked credential material matching {forbidden:?}: {rendered}"
            );
        }
    }

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

    fn native_route_graph_source(label: &str, credential_name: &str) -> HelperConfig {
        let provider_id = format!("codex-{label}");
        HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    provider_id.clone(),
                    ProviderConfig {
                        base_url: Some(format!("https://{provider_id}.example/v1")),
                        auth: UpstreamAuth {
                            auth_token_ref: Some(CredentialRef::Native {
                                name: credential_name.to_string(),
                            }),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![provider_id])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    fn two_native_route_graph_source(
        first_credential_name: &str,
        second_credential_name: &str,
    ) -> HelperConfig {
        let providers = [
            ("first", first_credential_name),
            ("second", second_credential_name),
        ]
        .into_iter()
        .map(|(provider_id, credential_name)| {
            (
                provider_id.to_string(),
                ProviderConfig {
                    base_url: Some(format!("https://{provider_id}.example/v1")),
                    auth: UpstreamAuth {
                        auth_token_ref: Some(CredentialRef::Native {
                            name: credential_name.to_string(),
                        }),
                        ..UpstreamAuth::default()
                    },
                    ..ProviderConfig::default()
                },
            )
        })
        .collect();
        HelperConfig {
            codex: ServiceRouteConfig {
                providers,
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "first".to_string(),
                    "second".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
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

    #[tokio::test]
    async fn published_runtime_surfaces_do_not_debug_or_serialize_credential_material() {
        const CANARY: &str = "RzT4Yq9P2nK8vL6cF3sW7mX5hJ1dB0aQ";

        let mut source = stable_identity_source("https://relay.example/v1", "relay-account");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some(CANARY.to_string().into()),
            ..UpstreamAuth::default()
        };
        assert!(!format!("{source:?}").contains(CANARY));

        let runtime = RuntimeConfig::new_with_config(Arc::new(source), provider_policy())
            .expect("build canary-backed runtime");
        let snapshot = runtime.capture().await;
        let redacted_config = snapshot.config();
        let graph = snapshot.route_graph("codex").expect("codex route graph");
        let route_plan = snapshot
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture route plan")
            .expect("configured route plan");
        let candidate = route_plan
            .template()
            .candidates
            .first()
            .expect("route candidate");
        let captured = route_plan
            .template()
            .capture_candidate(candidate)
            .expect("capture route candidate credential");
        let serialized_config =
            serde_json::to_string(redacted_config.as_ref()).expect("serialize redacted config");

        for (surface, rendered) in [
            ("runtime snapshot", format!("{snapshot:?}")),
            ("redacted helper config", format!("{redacted_config:?}")),
            ("serialized helper config", serialized_config),
            ("compiled route graph", format!("{graph:?}")),
            (
                "route plan template",
                format!("{:?}", route_plan.template()),
            ),
            (
                "credential generation",
                format!("{:?}", snapshot.credential_generation()),
            ),
            ("captured route candidate", format!("{captured:?}")),
        ] {
            assert_secret_canary_absent(surface, &rendered, CANARY);
        }
    }

    fn blocked_policy_runtime(
        source: HelperConfig,
    ) -> (RuntimeConfig, Arc<ProxyState>, Arc<RuntimeStore>) {
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let graph = compile_route_graph("codex", &source.codex).expect("compile initial graph");
        let candidate = graph.candidates().first().expect("initial candidate");
        let provider_endpoint = ProviderEndpointKey::new(
            "codex",
            candidate.provider_id.clone(),
            candidate.endpoint_id.clone(),
        );
        let credential_runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            runtime_store.as_ref(),
        )
        .expect("build test credential runtime");
        let generation = credential_runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: provider_endpoint.clone(),
                auth: &candidate.auth,
            }])
            .expect("build initial credential generation");
        let identity = generation
            .bind_upstream_identity(
                provider_endpoint,
                candidate.base_url.clone(),
                candidate.continuity_domain.clone(),
            )
            .expect("bind initial runtime identity");
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

    fn replace_credential_generation_for_test(
        runtime: &RuntimeConfig,
        previous: &RuntimeSnapshot,
        generation: Arc<CredentialGeneration>,
    ) -> Arc<RuntimeSnapshot> {
        let prepared = PreparedRuntimeSnapshot::with_refreshed_credentials(previous, generation)
            .expect("rebind test credential generation");
        runtime.store_current(prepared.finish(previous.provider_policy(), previous.revision()));
        runtime.capture_current()
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
    async fn request_capture_uses_lkg_while_background_native_reload_is_blocked() {
        let initial_source = native_route_graph_source("old", "relay.old");
        let reloaded_source = native_route_graph_source("new", "relay.new");
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (credential_sources, native_control) = CredentialRuntime::test_blocking_sources(
            SecretValue::new(b"generation-a".to_vec()).expect("valid initial credential"),
        );
        let mut proxy = super::super::ProxyService::new_with_runtime_store_and_credential_sources(
            reqwest::Client::new(),
            Arc::new(initial_source),
            "codex",
            runtime_store,
            credential_sources,
        )
        .expect("build native-backed proxy");
        assert_eq!(native_control.read_count(), 1);

        let before = proxy.config.capture().await;
        let changed_stamp = RuntimeSourceStamp {
            config_mtime: Some(
                before
                    .source_stamp()
                    .config_mtime
                    .and_then(|stamp| stamp.checked_add(Duration::from_secs(1)))
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            ),
            pricing_mtime: before.source_stamp().pricing_mtime,
            usage_providers_revision: before.source_stamp().usage_providers_revision,
        };
        let pricing = before.operator_pricing_catalog().as_ref().clone();
        Arc::get_mut(&mut proxy.config)
            .expect("proxy exclusively owns runtime config before driver start")
            .automatic_reload = RuntimeAutomaticReload {
            min_check_interval: RuntimeAutomaticReload::MIN_CHECK_INTERVAL,
            metadata: Arc::new(move || Box::pin(std::future::ready(changed_stamp))),
            config: Arc::new(move || {
                let source = reloaded_source.clone();
                Box::pin(async move { Ok(LoadedConfig { source }) })
            }),
            pricing: Arc::new(move || {
                let pricing = pricing.clone();
                Box::pin(async move { Ok(pricing) })
            }),
        };

        native_control.block();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let driver = proxy.spawn_runtime_config_driver(shutdown_rx);
        tokio::time::timeout(Duration::from_secs(2), async {
            while native_control.read_count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background reload should reach the blocked native backend");

        let context = tokio::time::timeout(
            Duration::from_secs(1),
            super::super::request_preparation::load_request_config_context(&proxy, None),
        )
        .await
        .expect("request capture must not wait for background credential I/O");
        assert!(Arc::ptr_eq(&before, &context.runtime_snapshot));
        let route_plan = context
            .runtime_snapshot
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture route plan")
            .expect("configured route plan");
        let candidate = route_plan
            .template()
            .candidates
            .first()
            .expect("old route candidate");
        assert_eq!(candidate.provider_id, "codex-old");
        let target = route_plan
            .template()
            .capture_candidate(candidate)
            .expect("capture old credential generation");
        let mut headers = HeaderMap::new();
        super::super::attempt_request::inject_auth_headers(
            "codex",
            target.credential(),
            target.base_url(),
            &mut headers,
        )
        .expect("inject captured native credential");
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer generation-a")
        );

        native_control.release();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let current = proxy.config.capture().await;
                if current.revision() > before.revision() {
                    assert_eq!(
                        current
                            .route_graph("codex")
                            .expect("reloaded codex graph")
                            .candidates()[0]
                            .provider_id,
                        "codex-new"
                    );
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background reload should publish after native I/O completes");

        shutdown_tx.send(true).expect("request driver shutdown");
        driver.await.expect("join runtime config driver");
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
            auth_token: Some("account-a-secret".to_string().into()),
            ..UpstreamAuth::default()
        };
        let mut reloaded = initial.clone();
        reloaded
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("account-b-secret".to_string().into()),
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
    async fn reload_preserves_automatic_policy_when_only_legacy_config_version_migrates() {
        let mut initial = stable_identity_source("https://old.example/v1", "continuity-a");
        initial.version = 5;
        initial
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("unchanged-legacy-secret".to_string().into()),
            ..UpstreamAuth::default()
        };
        let mut migrated = initial.clone();
        migrated.version = CURRENT_CONFIG_VERSION;
        let (runtime, state, runtime_store) = blocked_policy_runtime(initial);
        let before = runtime.capture().await;
        let before_projection = before.provider_policy().projections[0].clone();
        assert_eq!(
            before_projection.automatic,
            crate::runtime_store::ProviderAutomaticEligibility::Blocked
        );
        assert!(before_projection.active_action.is_some());
        assert!(before_projection.incarnation_id.is_some());

        let changed = runtime
            .reload_with_source(|| async { Ok((LoadedConfig { source: migrated }, None)) })
            .await
            .expect("reload migrated legacy configuration");

        assert!(changed, "the canonical config revision must advance");
        let after = runtime.capture().await;
        assert_eq!(after.config.version, CURRENT_CONFIG_VERSION);
        assert_eq!(after.provider_policy().projections[0], before_projection);
        assert_eq!(
            *state.capture_provider_policy_snapshot().await,
            runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy after migration reload")
        );
    }

    #[tokio::test]
    async fn published_rotation_preserves_old_request_capture_and_updates_later_requests() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let runtime = Arc::new(runtime);
        let before = runtime.capture().await;
        let old_plan = before
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture old route plan")
            .expect("old route plan");
        let old_candidate = old_plan
            .template()
            .candidates
            .first()
            .expect("old candidate");
        let old_target = old_plan
            .template()
            .capture_candidate(old_candidate)
            .expect("capture old target credential binding");
        let old_route_graph_key = old_plan.template().route_graph_key();
        assert_eq!(
            old_target
                .credential()
                .bearer_header()
                .expect("old bearer")
                .as_bytes(),
            b"Bearer generation-a"
        );
        let old_generation_revision = before.credential_generation().revision();

        control.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let driver_runtime = Arc::clone(&runtime);
        let driver = tokio::spawn(async move {
            driver_runtime
                .run_credential_refresh_driver(shutdown_rx)
                .await;
        });
        runtime.schedule_credential_refresh(old_target.credential());
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime.capture().await.credential_generation().revision()
                    > old_generation_revision
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("credential rotation should publish");

        assert_eq!(control.read_count(), 2);
        assert_eq!(
            old_target
                .credential()
                .bearer_header()
                .expect("captured old bearer")
                .as_bytes(),
            b"Bearer generation-a"
        );
        let after = runtime.capture().await;
        let new_plan = after
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture new route plan")
            .expect("new route plan");
        let new_target = new_plan
            .template()
            .capture_candidate(
                new_plan
                    .template()
                    .candidates
                    .first()
                    .expect("new candidate"),
            )
            .expect("capture new target credential binding");
        assert_eq!(
            new_target
                .credential()
                .bearer_header()
                .expect("new bearer")
                .as_bytes(),
            b"Bearer generation-b"
        );
        assert_ne!(old_route_graph_key, new_plan.template().route_graph_key());

        runtime.schedule_credential_refresh(old_target.credential());
        let delayed_old_refresh = runtime.take_pending_credential_refresh();
        assert!(
            delayed_old_refresh.is_empty(),
            "a superseded generation must be discarded before entering the refresh queue"
        );
        assert_eq!(
            control.read_count(),
            2,
            "a delayed auth failure from generation A must be discarded before native I/O"
        );

        let _ = shutdown_tx.send(true);
        driver.await.expect("join credential refresh driver");
    }

    #[tokio::test]
    async fn explicit_native_delete_then_upsert_publishes_new_generations() {
        let name = CredentialName::parse("relay.primary").expect("valid credential name");
        let source = native_route_graph_source("managed", name.as_str());
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");

        let before = runtime.capture().await;
        let old_plan = before
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture old route plan")
            .expect("old route plan");
        let old_target = old_plan
            .template()
            .capture_candidate(
                old_plan
                    .template()
                    .candidates
                    .first()
                    .expect("old candidate"),
            )
            .expect("capture old target");
        let old_route_graph_key = old_plan.template().route_graph_key();
        let old_policy_route_scope = old_target.runtime_identity().policy_route_scope();
        assert_eq!(control.read_count(), 1);

        control.set_missing();
        assert!(
            runtime
                .invalidate_deleted_native_credential(&name)
                .await
                .expect("publish explicit delete")
        );
        assert_eq!(
            control.read_count(),
            1,
            "an explicit delete must publish unavailability without rereading the backend"
        );
        let deleted = runtime.capture().await;
        let deleted_plan = deleted
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture deleted route plan")
            .expect("deleted route plan");
        let deleted_target = deleted_plan
            .template()
            .capture_candidate(
                deleted_plan
                    .template()
                    .candidates
                    .first()
                    .expect("deleted candidate"),
            )
            .expect("capture deleted target");
        assert!(!deleted_target.credential().is_available());
        assert!(deleted_target.credential().bearer_header().is_none());
        assert_eq!(
            old_target
                .credential()
                .bearer_header()
                .expect("captured A remains available")
                .as_bytes(),
            b"Bearer generation-a"
        );

        control.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        assert!(
            runtime
                .refresh_native_credential_after_upsert(&name)
                .await
                .expect("publish explicit upsert")
        );
        assert_eq!(control.read_count(), 2);

        let after = runtime.capture().await;
        let new_plan = after
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture new route plan")
            .expect("new route plan");
        let new_target = new_plan
            .template()
            .capture_candidate(
                new_plan
                    .template()
                    .candidates
                    .first()
                    .expect("new candidate"),
            )
            .expect("capture new target");
        assert_eq!(
            new_target
                .credential()
                .bearer_header()
                .expect("new B bearer")
                .as_bytes(),
            b"Bearer generation-b"
        );
        assert_eq!(
            old_target
                .credential()
                .bearer_header()
                .expect("old request remains bound to A")
                .as_bytes(),
            b"Bearer generation-a"
        );
        assert_ne!(old_route_graph_key, new_plan.template().route_graph_key());
        assert_ne!(
            old_policy_route_scope,
            new_target.runtime_identity().policy_route_scope()
        );
    }

    #[tokio::test]
    async fn queued_auth_refresh_rebinds_other_handle_to_current_generation() {
        let source = two_native_route_graph_source("relay.first", "relay.second");
        let (capabilities, control) = CredentialRuntime::test_blocking_sources(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build two-handle credential runtime");
        let runtime = Arc::new(runtime);
        let before = runtime.capture().await;
        let old_plan = before
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture old route plan")
            .expect("old route plan");
        let old_targets = old_plan
            .template()
            .candidates
            .iter()
            .map(|candidate| old_plan.template().capture_candidate(candidate))
            .collect::<Result<Vec<_>>>()
            .expect("capture both old targets");
        assert_eq!(old_targets.len(), 2);
        assert_eq!(control.read_count(), 2);

        control.block();
        runtime.schedule_credential_refresh(old_targets[0].credential());
        let first_pending = runtime.take_pending_credential_refresh();
        let refresh_runtime = Arc::clone(&runtime);
        let first_refresh = tokio::spawn(async move {
            refresh_runtime
                .refresh_pending_credentials(first_pending)
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while control.read_count() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first handle refresh reaches blocked backend");

        runtime.schedule_credential_refresh(old_targets[1].credential());
        let second_pending = runtime.take_pending_credential_refresh();
        assert_eq!(
            second_pending.values().map(BTreeSet::len).sum::<usize>(),
            1,
            "the second handle must remain queued under the captured old generation"
        );

        control.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        control.release();
        assert!(
            first_refresh
                .await
                .expect("join first handle refresh")
                .expect("publish first handle refresh")
        );

        assert!(
            runtime
                .refresh_pending_credentials(second_pending)
                .await
                .expect("refresh queued second handle against current generation")
        );
        assert_eq!(control.read_count(), 4);

        let after = runtime.capture().await;
        let new_plan = after
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture new route plan")
            .expect("new route plan");
        let new_targets = new_plan
            .template()
            .candidates
            .iter()
            .map(|candidate| new_plan.template().capture_candidate(candidate))
            .collect::<Result<Vec<_>>>()
            .expect("capture both new targets");
        for target in &new_targets {
            assert_eq!(
                target
                    .credential()
                    .bearer_header()
                    .expect("new bearer")
                    .as_bytes(),
                b"Bearer generation-b"
            );
        }
        for target in &old_targets {
            assert_eq!(
                target
                    .credential()
                    .bearer_header()
                    .expect("captured old bearer")
                    .as_bytes(),
                b"Bearer generation-a"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn idle_driver_refreshes_stale_native_credential_through_hard_expiry_and_recovery() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let runtime = Arc::new(runtime);
        let endpoint = ProviderEndpointKey::new("codex", "stable", "default");
        let initial = runtime.capture_current();
        let initial_generation = initial.credential_generation();
        assert_eq!(control.read_count(), 1);

        // Keep the std::time deadline in the near future so the real driver registers a
        // Tokio timer before virtual time advances.
        let soft_due_generation = initial_generation.aged_for_test(Duration::from_secs(55));
        assert!(
            soft_due_generation
                .next_native_deadline()
                .is_some_and(|deadline| deadline > Instant::now())
        );
        assert!(
            soft_due_generation
                .capture_bound(&endpoint)
                .expect("capture nearly soft-due credential")
                .is_available()
        );
        let soft_due = replace_credential_generation_for_test(
            runtime.as_ref(),
            initial.as_ref(),
            soft_due_generation,
        );
        let soft_due_revision = soft_due.credential_generation().revision();

        control.set_missing();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let driver_runtime = Arc::clone(&runtime);
        let driver = tokio::spawn(async move {
            driver_runtime
                .run_credential_refresh_driver(shutdown_rx)
                .await;
        });

        while runtime.credential_refresh_wait_epoch.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(control.read_count(), 1);
        tokio::time::advance(Duration::from_secs(6)).await;

        while control.read_count() < 2 {
            tokio::task::yield_now().await;
        }
        let stale = loop {
            let snapshot = runtime.capture_current();
            if snapshot.credential_generation().revision() > soft_due_revision {
                break snapshot;
            }
            tokio::task::yield_now().await;
        };
        let stale_generation = stale.credential_generation();
        let stale_credential = stale_generation
            .capture_bound(&endpoint)
            .expect("capture stale credential");
        assert!(stale_credential.is_available());
        assert_eq!(
            stale_credential
                .bearer_header()
                .expect("stale bearer remains usable")
                .as_bytes(),
            b"Bearer generation-a"
        );
        assert_ne!(
            stale_generation.digest(),
            soft_due.credential_generation().digest()
        );

        while runtime.credential_refresh_wait_epoch.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        assert_eq!(control.read_count(), 2);
        let _scheduled_retry = stale_generation
            .next_native_deadline()
            .expect("stale native retry deadline");

        // Cross the real-clock hard boundary without waking the already-registered retry timer.
        let hard_expired_generation = (0_u32..=16)
            .map(|power| stale_generation.aged_for_test(Duration::from_secs(1_u64 << power)))
            .find(|generation| {
                !generation
                    .capture_bound(&endpoint)
                    .expect("capture aged stale credential")
                    .is_available()
            })
            .expect("find an age past native hard expiry");
        let hard_expired = replace_credential_generation_for_test(
            runtime.as_ref(),
            stale.as_ref(),
            hard_expired_generation,
        );
        let unavailable = hard_expired
            .credential_generation()
            .capture_bound(&endpoint)
            .expect("capture hard-expired credential");
        assert!(!unavailable.is_available());
        assert!(unavailable.bearer_header().is_none());
        assert_eq!(control.read_count(), 2);

        control.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        // The test-only generation replacement bypasses the driver's normal publication loop.
        // Wake its registered wait so it recalculates the now-expired native deadline.
        runtime.credential_refresh_notify.notify_one();

        while control.read_count() < 3 {
            tokio::task::yield_now().await;
        }
        let recovered = loop {
            let snapshot = runtime.capture_current();
            let credential = snapshot
                .credential_generation()
                .capture_bound(&endpoint)
                .expect("capture recovered credential");
            if credential
                .bearer_header()
                .is_some_and(|header| header.as_bytes() == b"Bearer generation-b")
            {
                break snapshot;
            }
            tokio::task::yield_now().await;
        };
        assert!(
            recovered
                .credential_generation()
                .capture_bound(&endpoint)
                .expect("capture published recovery")
                .is_available()
        );

        shutdown_tx.send(true).expect("request driver shutdown");
        driver.await.expect("join idle credential refresh driver");
        assert_eq!(control.read_count(), 3);
    }

    #[tokio::test]
    async fn credential_rotation_ticket_rejects_an_older_reload_finishing_last() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let reload_source = source.clone();
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let runtime = Arc::new(runtime);
        let before = runtime.capture().await;
        let plan = before
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture route plan")
            .expect("configured route plan");
        let target = plan
            .template()
            .capture_candidate(plan.template().candidates.first().expect("route candidate"))
            .expect("capture credential target");

        let (reload_started_tx, reload_started_rx) = oneshot::channel();
        let (release_reload_tx, release_reload_rx) = oneshot::channel();
        let reload_runtime = Arc::clone(&runtime);
        let reload = tokio::spawn(async move {
            reload_runtime
                .reload_with_source(|| async move {
                    let _ = reload_started_tx.send(());
                    release_reload_rx.await.expect("release old reload");
                    Ok((
                        LoadedConfig {
                            source: reload_source,
                        },
                        None,
                    ))
                })
                .await
        });
        reload_started_rx.await.expect("old reload started");

        control.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        runtime.schedule_credential_refresh(target.credential());
        let pending = runtime.take_pending_credential_refresh();
        assert!(
            runtime
                .refresh_pending_credentials(pending)
                .await
                .expect("publish credential rotation")
        );

        control.set_value(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        release_reload_tx.send(()).expect("release old reload");
        assert!(
            !reload
                .await
                .expect("join old reload")
                .expect("reject old reload")
        );

        let after = runtime.capture().await;
        let plan = after
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture final route plan")
            .expect("configured final route plan");
        let target = plan
            .template()
            .capture_candidate(plan.template().candidates.first().expect("final candidate"))
            .expect("capture final credential target");
        assert_eq!(
            target
                .credential()
                .bearer_header()
                .expect("final bearer")
                .as_bytes(),
            b"Bearer generation-b"
        );
        assert_eq!(control.read_count(), 3);
    }

    #[tokio::test]
    async fn repeated_auth_failures_are_throttled_per_generation_and_handle() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let runtime = Arc::new(runtime);
        let snapshot = runtime.capture().await;
        let plan = snapshot
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture route plan")
            .expect("configured route plan");
        let target = plan
            .template()
            .capture_candidate(plan.template().candidates.first().expect("route candidate"))
            .expect("capture credential target");

        runtime.schedule_credential_refresh(target.credential());
        runtime
            .refresh_pending_credentials(runtime.take_pending_credential_refresh())
            .await
            .expect("first auth refresh");
        assert_eq!(control.read_count(), 2);

        let attempted_at = *runtime
            .recent_auth_refresh
            .lock()
            .expect("recent auth refresh lock")
            .values()
            .next()
            .expect("recorded auth refresh attempt");

        runtime.schedule_credential_refresh_at(
            target.credential(),
            attempted_at + AUTH_FAILURE_REFRESH_MIN_INTERVAL - Duration::from_nanos(1),
        );
        assert!(runtime.take_pending_credential_refresh().is_empty());
        assert_eq!(control.read_count(), 2);

        runtime.schedule_credential_refresh_at(
            target.credential(),
            attempted_at + AUTH_FAILURE_REFRESH_MIN_INTERVAL,
        );
        runtime
            .refresh_pending_credentials(runtime.take_pending_credential_refresh())
            .await
            .expect("auth refresh after throttle window");
        assert_eq!(control.read_count(), 3);
    }

    #[tokio::test]
    async fn driver_shutdown_detaches_blocked_native_read_without_starting_another() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let (capabilities, control) = CredentialRuntime::test_blocking_sources(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let runtime = Arc::new(runtime);
        let snapshot = runtime.capture().await;
        let plan = snapshot
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture route plan")
            .expect("configured route plan");
        let target = plan
            .template()
            .capture_candidate(plan.template().candidates.first().expect("route candidate"))
            .expect("capture credential target");

        control.block();
        runtime.schedule_credential_refresh(target.credential());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let driver_runtime = Arc::clone(&runtime);
        let driver = tokio::spawn(async move {
            driver_runtime
                .run_credential_refresh_driver(shutdown_rx)
                .await;
        });
        while control.read_count() < 2 {
            tokio::task::yield_now().await;
        }
        shutdown_tx.send(true).expect("request driver shutdown");
        driver.await.expect("driver exits while native read blocks");

        let second_runtime = runtime.credential_runtime.clone();
        let generation = snapshot.credential_generation();
        let second = tokio::spawn(async move {
            second_runtime
                .refresh_generation(
                    generation,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        while runtime
            .credential_runtime
            .native_inflight_owner_count_for_test()
            < 3
        {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            control.read_count(),
            2,
            "the blocked flight must remain the singleflight owner"
        );
        control.release();
        second
            .await
            .expect("join second refresh")
            .expect("second refresh joins original native read");
        assert_eq!(control.read_count(), 2);
    }

    #[tokio::test]
    async fn unchanged_native_refresh_advances_freshness_without_identity_churn() {
        let mut source = stable_identity_source("https://relay.example/v1", "continuity-a");
        source
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"generation-a".to_vec()).expect("valid credential"),
        );
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let (runtime, _state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(source),
            runtime_store,
            capabilities,
        )
        .expect("build credential-backed runtime");
        let before = runtime.capture().await;
        let old_plan = before
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture old route plan")
            .expect("old route plan");
        let old_candidate = old_plan
            .template()
            .candidates
            .first()
            .expect("old candidate");
        let old_target = old_plan
            .template()
            .capture_candidate(old_candidate)
            .expect("capture old target credential binding");
        let before_deadline = before
            .credential_generation()
            .next_native_deadline()
            .expect("native refresh deadline");
        let before_identities = before
            .candidate_identities()
            .expect("capture old runtime identities");
        let before_policy = before.provider_policy();
        let before_route_graph_key = old_plan.template().route_graph_key().to_string();

        tokio::time::sleep(Duration::from_millis(1)).await;
        runtime.schedule_credential_refresh(old_target.credential());
        let pending = runtime.take_pending_credential_refresh();
        assert!(
            !runtime
                .refresh_pending_credentials(pending)
                .await
                .expect("refresh unchanged native credential"),
            "the same credential content must not advance the runtime revision"
        );

        let after = runtime.capture().await;
        let new_plan = after
            .capture_route_plan("codex", &RouteRequestContext::default())
            .expect("capture refreshed route plan")
            .expect("refreshed route plan");
        assert_eq!(control.read_count(), 2);
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.digest(), before.digest());
        assert_eq!(
            after.credential_generation().revision(),
            before.credential_generation().revision()
        );
        assert_eq!(
            new_plan.template().route_graph_key(),
            before_route_graph_key
        );
        assert_eq!(
            after
                .candidate_identities()
                .expect("capture refreshed runtime identities"),
            before_identities
        );
        assert_eq!(after.provider_policy().as_ref(), before_policy.as_ref());
        assert!(
            after
                .credential_generation()
                .next_native_deadline()
                .expect("refreshed native deadline")
                > before_deadline,
            "an unchanged successful read must still extend native freshness"
        );
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
        let identity = runtime
            .capture_current()
            .route_graph("codex")
            .expect("captured codex graph")
            .candidate_identities()
            .expect("capture runtime identities")
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
    async fn cancelled_reload_waiting_for_health_publication_preserves_old_composite() {
        let initial = stable_identity_source("https://old.example/v1", "continuity-a");
        let reloaded = stable_identity_source("https://new.example/v1", "continuity-a");
        let (runtime, state, runtime_store) = blocked_policy_runtime(initial);
        let runtime = Arc::new(runtime);
        let before = runtime.capture().await;
        let policy_before = state.capture_provider_policy_snapshot().await;
        let durable_before = runtime_store
            .provider_policy_snapshot()
            .expect("read durable policy before cancelled reload");
        let health_publication = state
            .hold_provider_runtime_health_publication_for_test()
            .await;

        let reload_runtime = Arc::clone(&runtime);
        let cancelled_source = reloaded.clone();
        let reload = tokio::spawn(async move {
            reload_runtime
                .reload_with_source(|| async move {
                    Ok((
                        LoadedConfig {
                            source: cancelled_source,
                        },
                        None,
                    ))
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if tokio::time::timeout(
                    Duration::from_millis(10),
                    state.capture_provider_policy_snapshot(),
                )
                .await
                .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reload must reach the health publication lock before durable commit");
        assert_eq!(
            runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy while reload is blocked"),
            durable_before
        );

        reload.abort();
        drop(health_publication);
        assert!(
            reload
                .await
                .expect_err("blocked reload must be cancelled")
                .is_cancelled()
        );
        assert!(Arc::ptr_eq(&before, &runtime.capture().await));
        assert_eq!(
            *state.capture_provider_policy_snapshot().await,
            *policy_before
        );
        assert_eq!(
            runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy after cancelled reload"),
            durable_before
        );

        assert!(
            runtime
                .reload_with_source(|| async { Ok((LoadedConfig { source: reloaded }, None)) })
                .await
                .expect("publish retry after cancelled reload")
        );
        let published = runtime.capture().await;
        assert!(!Arc::ptr_eq(&before, &published));
        assert_eq!(
            published.provider_policy(),
            state.capture_provider_policy_snapshot().await
        );
        assert_eq!(
            published.provider_policy().as_ref(),
            &runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy after successful retry")
        );
    }

    #[tokio::test]
    async fn cancelled_reload_waiting_for_operator_control_preserves_old_composite() {
        let initial = stable_identity_source("https://old.example/v1", "continuity-a");
        let mut reloaded = stable_identity_source("https://new.example/v1", "continuity-a");
        reloaded
            .codex
            .routing
            .as_mut()
            .expect("configured route graph")
            .scheduling_preset = SchedulingPreset::ThroughputFirst;
        let (runtime, state, runtime_store) = blocked_policy_runtime(initial);
        let runtime = Arc::new(runtime);
        let before = runtime.capture_current();
        let policy_before = state.capture_provider_policy_snapshot().await;
        let durable_before = runtime_store
            .provider_policy_snapshot()
            .expect("read durable policy before cancelled reload");
        let control_before = state.capture_routing_operator_control().await;
        let operator_publication = state
            .hold_routing_operator_control_publication_for_test()
            .await;

        let reload_runtime = Arc::clone(&runtime);
        let cancelled_source = reloaded.clone();
        let reload = tokio::spawn(async move {
            reload_runtime
                .reload_with_source(|| async move {
                    Ok((
                        LoadedConfig {
                            source: cancelled_source,
                        },
                        None,
                    ))
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if tokio::time::timeout(
                    Duration::from_millis(10),
                    state.capture_provider_policy_snapshot(),
                )
                .await
                .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reload must reach operator-control publication");
        assert!(Arc::ptr_eq(&before, &runtime.capture_current()));
        assert_eq!(
            runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy while reload is blocked"),
            durable_before
        );

        reload.abort();
        drop(operator_publication);
        assert!(
            reload
                .await
                .expect_err("blocked reload must be cancelled")
                .is_cancelled()
        );
        assert!(Arc::ptr_eq(&before, &runtime.capture_current()));
        assert_eq!(
            state.capture_routing_operator_control().await,
            control_before
        );
        assert_eq!(
            *state.capture_provider_policy_snapshot().await,
            *policy_before
        );
        assert_eq!(
            runtime_store
                .provider_policy_snapshot()
                .expect("read durable policy after cancelled reload"),
            durable_before
        );

        assert!(
            runtime
                .reload_with_source(|| async { Ok((LoadedConfig { source: reloaded }, None)) })
                .await
                .expect("publish retry after cancelled reload")
        );
        let published = runtime.capture().await;
        let published_graph_key = published
            .route_graph("codex")
            .expect("published codex route graph")
            .digest()
            .to_string();
        assert_eq!(
            state
                .capture_routing_operator_control()
                .await
                .route_graph_key("codex"),
            Some(published_graph_key.as_str())
        );
    }

    #[tokio::test]
    async fn restart_rebuilds_matching_snapshot_after_durable_identity_commit() {
        let helper_home = std::env::temp_dir().join(format!(
            "codex-helper-runtime-publication-recovery-test-{}",
            uuid::Uuid::new_v4()
        ));
        let runtime_store =
            Arc::new(RuntimeStore::open_in_home(&helper_home).expect("open runtime store"));
        let mut initial = stable_identity_source("https://relay.example/v1", "continuity-a");
        initial
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("generation-a".to_string().into()),
            ..UpstreamAuth::default()
        };
        let (runtime, state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            Arc::new(initial.clone()),
            Arc::clone(&runtime_store),
            CredentialSourceCapabilities::server(),
        )
        .expect("build initial runtime");
        let old_identities = runtime
            .capture_current()
            .candidate_identities()
            .expect("capture old identities");

        let mut rotated = initial;
        rotated
            .codex
            .providers
            .get_mut("stable")
            .expect("stable provider")
            .auth = UpstreamAuth {
            auth_token: Some("generation-b".to_string().into()),
            ..UpstreamAuth::default()
        };
        let prepared = PreparedRuntimeSnapshot::build(
            Arc::new(rotated.clone()),
            runtime.capture_current().operator_pricing_catalog(),
            RuntimeSourceStamp::default(),
            &runtime.credential_runtime,
        )
        .expect("prepare rotated runtime snapshot");
        let committed_identities = prepared
            .candidate_identities()
            .expect("capture committed identities");
        assert_ne!(old_identities, committed_identities);
        let committed_policy = state
            .reconcile_runtime_upstream_identities(&committed_identities, now_ms())
            .await
            .expect("commit rotated runtime identities");
        assert_eq!(
            runtime
                .capture_current()
                .candidate_identities()
                .expect("capture current identities"),
            old_identities
        );

        drop(prepared);
        drop(runtime);
        drop(state);
        drop(runtime_store);

        let reopened_store =
            Arc::new(RuntimeStore::open_in_home(&helper_home).expect("reopen runtime store"));
        let (restarted, restarted_state) =
            RuntimeConfig::new_with_runtime_store_and_credential_sources(
                Arc::new(rotated),
                Arc::clone(&reopened_store),
                CredentialSourceCapabilities::server(),
            )
            .expect("rebuild runtime after interrupted publication");
        let restarted_snapshot = restarted.capture_current();

        assert_eq!(
            restarted_snapshot
                .candidate_identities()
                .expect("capture restarted identities"),
            committed_identities
        );
        assert_eq!(restarted_snapshot.provider_policy(), committed_policy);
        assert_eq!(
            restarted_snapshot.provider_policy().as_ref(),
            &reopened_store
                .provider_policy_snapshot()
                .expect("read restarted durable policy")
        );

        drop(restarted);
        drop(restarted_state);
        drop(reopened_store);
        let _ = std::fs::remove_dir_all(helper_home);
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
                        ..RuntimeSourceStamp::default()
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
    async fn usage_provider_source_change_triggers_runtime_rebuild() {
        let runtime = runtime_config("old");
        let before = runtime.capture().await;
        let pricing = before.operator_pricing_catalog().as_ref().clone();
        let config_loads = AtomicUsize::new(0);
        let pricing_loads = AtomicUsize::new(0);
        let mut changed_stamp = before.source_stamp();
        changed_stamp.usage_providers_revision = [7; 32];
        assert_ne!(changed_stamp, before.source_stamp());

        let changed = runtime
            .maybe_reload_with(
                Duration::ZERO,
                || async { changed_stamp },
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
            .expect("usage-provider source reload");

        assert!(!changed);
        assert_eq!(config_loads.load(Ordering::SeqCst), 1);
        assert_eq!(pricing_loads.load(Ordering::SeqCst), 1);
        let after = runtime.capture().await;
        assert_eq!(after.revision(), before.revision());
        assert_eq!(
            after.source_stamp().usage_providers_revision,
            changed_stamp.usage_providers_revision
        );
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
                            ..RuntimeSourceStamp::default()
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
