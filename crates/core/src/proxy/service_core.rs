use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::Client;

use crate::config::{
    HelperConfig, PersistedProviderSpec, PersistedProvidersCatalog, PersistedRoutingSpec,
};
use crate::credentials::CredentialSourceCapabilities;
use crate::filter::RequestFilter;
use crate::routing_explain::RoutingExplainResponse;
use crate::routing_ir::RouteRequestContext;
use crate::runtime_store::RuntimeStore;
use crate::service_target::{
    LocalCredentialRefreshAction, LocalCredentialRefreshStatus, ServiceInstallGeneration,
};
use crate::state::{ProxyState, SessionBinding, SessionContinuityMode};

use super::profile_defaults::effective_default_profile_name;
use super::{
    CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse, CodexRelayLiveSmokeRequest,
    CodexRelayLiveSmokeResponse, ProfilesResponse, ProviderBalanceRefreshResponse,
    ProxyControlError, ProxyService, RuntimeConfig,
};

impl ProxyService {
    pub(crate) async fn captured_runtime_config(&self) -> Arc<HelperConfig> {
        self.config.snapshot().await
    }

    #[cfg(test)]
    pub(crate) fn spawn_credential_refresh_driver(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let config = Arc::clone(&self.config);
        tokio::spawn(async move {
            config.run_credential_refresh_driver(shutdown_rx).await;
        })
    }

    pub(crate) fn spawn_runtime_config_driver(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let credential_config = Arc::clone(&self.config);
        let automatic_reload_proxy = self.clone();
        let credential_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = credential_config.run_credential_refresh_driver(credential_shutdown_rx) => {}
                _ = automatic_reload_proxy.run_automatic_reload_driver(shutdown_rx) => {}
            }
        })
    }

    async fn run_automatic_reload_driver(
        &self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let check_interval = self.config.automatic_reload_check_interval();
        loop {
            let changed = tokio::select! {
                biased;
                _ = wait_for_proxy_shutdown(&mut shutdown_rx) => return,
                changed = self.config.maybe_reload_from_disk() => changed,
            };
            if changed {
                super::control_plane_service::prune_runtime_observability_after_reload(self).await;
            }

            tokio::select! {
                biased;
                _ = wait_for_proxy_shutdown(&mut shutdown_rx) => return,
                () = tokio::time::sleep(check_interval) => {}
            }
        }
    }

    pub(crate) async fn publish_operator_pricing_catalog(&self) -> anyhow::Result<bool> {
        self.config.publish_operator_pricing_catalog().await
    }

    #[cfg(test)]
    pub(crate) fn new(
        client: Client,
        config: Arc<HelperConfig>,
        service_name: &'static str,
    ) -> Self {
        let runtime_store = Arc::new(
            RuntimeStore::open_in_memory()
                .expect("an isolated in-memory runtime store should open"),
        );
        Self::new_with_runtime_store_inner(
            client,
            config,
            service_name,
            runtime_store,
            CredentialSourceCapabilities::server(),
            false,
        )
        .expect("test proxy route graph must compile")
    }

    pub(crate) fn new_with_runtime_store(
        client: Client,
        config: Arc<HelperConfig>,
        service_name: &'static str,
        runtime_store: Arc<RuntimeStore>,
    ) -> anyhow::Result<Self> {
        Self::new_with_runtime_store_inner(
            client,
            config,
            service_name,
            runtime_store,
            CredentialSourceCapabilities::server(),
            true,
        )
    }

    pub(crate) fn new_with_runtime_store_and_credential_sources(
        client: Client,
        config: Arc<HelperConfig>,
        service_name: &'static str,
        runtime_store: Arc<RuntimeStore>,
        credential_sources: CredentialSourceCapabilities,
    ) -> anyhow::Result<Self> {
        Self::new_with_runtime_store_inner(
            client,
            config,
            service_name,
            runtime_store,
            credential_sources,
            true,
        )
    }

    fn new_with_runtime_store_inner(
        client: Client,
        config: Arc<HelperConfig>,
        service_name: &'static str,
        runtime_store: Arc<RuntimeStore>,
        credential_sources: CredentialSourceCapabilities,
        spawn_cleanup_task: bool,
    ) -> anyhow::Result<Self> {
        let (runtime_config, state) = RuntimeConfig::new_with_runtime_store_and_credential_sources(
            config,
            runtime_store,
            credential_sources,
        )?;
        let runtime_config = Arc::new(runtime_config);
        if spawn_cleanup_task {
            ProxyState::spawn_cleanup_task(&state);
        }
        Ok(Self {
            client,
            config: runtime_config,
            service_name,
            concurrency_limiter: Arc::new(super::concurrency_limits::ConcurrencyLimiter::default()),
            filter: RequestFilter::new(),
            state,
            service_install_generation: None,
        })
    }

    pub(crate) fn with_service_install_generation(
        mut self,
        generation: Option<ServiceInstallGeneration>,
    ) -> Self {
        self.service_install_generation = generation;
        self
    }

    pub(crate) fn service_install_generation(&self) -> Option<&ServiceInstallGeneration> {
        self.service_install_generation.as_ref()
    }

    pub(crate) async fn refresh_native_credential(
        &self,
        name: &crate::credentials::CredentialName,
        action: LocalCredentialRefreshAction,
    ) -> Result<(LocalCredentialRefreshStatus, u64), ProxyControlError> {
        let result = match action {
            LocalCredentialRefreshAction::Upsert => {
                self.config
                    .refresh_native_credential_after_upsert(name)
                    .await
            }
            LocalCredentialRefreshAction::Delete => {
                self.config.invalidate_deleted_native_credential(name).await
            }
        }
        .map_err(|error| {
            tracing::warn!(
                service = self.service_name,
                error = %error,
                "local credential runtime refresh failed"
            );
            ProxyControlError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "credential runtime refresh failed",
            )
        })?;
        Ok((result.status, result.runtime_revision))
    }

    pub fn new_ephemeral_diagnostic(
        config: Arc<HelperConfig>,
        service_name: &'static str,
    ) -> anyhow::Result<Self> {
        let client = super::upstream_http_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("build ephemeral diagnostic upstream client")?;
        let runtime_store = Arc::new(
            RuntimeStore::open_in_memory().context("open ephemeral diagnostic runtime store")?,
        );
        Self::new_with_runtime_store(client, config, service_name, runtime_store)
    }

    pub(super) async fn ensure_default_session_binding(
        &self,
        view: &crate::config::ServiceRouteConfig,
        session_id: &str,
        now_ms: u64,
    ) -> Option<SessionBinding> {
        if let Some(binding) = self.state.get_session_binding(session_id).await {
            self.state.touch_session_binding(session_id, now_ms).await;
            return Some(binding);
        }

        let profile_name = effective_default_profile_name(view)?;
        let profile = crate::config::resolve_service_profile_from_catalog(
            &view.profiles,
            profile_name.as_str(),
        )
        .ok()?;
        let binding = SessionBinding {
            session_id: session_id.to_string(),
            profile_name: Some(profile_name),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            continuity_mode: SessionContinuityMode::DefaultProfile,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_seen_ms: now_ms,
        };
        self.state.set_session_binding(binding.clone()).await;
        Some(binding)
    }

    pub fn state_handle(&self) -> Arc<ProxyState> {
        self.state.clone()
    }

    #[cfg(test)]
    pub(crate) async fn runtime_identity_for_provider_endpoint_for_test(
        &self,
        provider_endpoint: &crate::runtime_identity::ProviderEndpointKey,
    ) -> crate::runtime_identity::RuntimeUpstreamIdentity {
        let snapshot = self.config.capture().await;
        snapshot
            .route_graph(self.service_name)
            .into_iter()
            .flat_map(|graph| {
                graph
                    .candidate_identities()
                    .expect("published route graph credential bindings")
            })
            .find(|identity| identity.provider_endpoint == *provider_endpoint)
            .unwrap_or_else(|| {
                panic!(
                    "provider endpoint {provider_endpoint} is not active in the current runtime snapshot"
                )
            })
    }

    #[cfg(test)]
    pub(crate) async fn set_provider_automatic_block_for_test(
        &self,
        provider_endpoint: crate::runtime_identity::ProviderEndpointKey,
        blocked: bool,
        observed_at_ms: u64,
    ) -> crate::runtime_store::ProviderObservationCommit {
        let identity = self
            .runtime_identity_for_provider_endpoint_for_test(&provider_endpoint)
            .await;
        self.state
            .set_provider_automatic_block_for_runtime_identity_for_test(
                identity,
                blocked,
                observed_at_ms,
            )
            .await
    }

    pub async fn refresh_provider_balances(
        &self,
        route_provider_id_filter: Option<&str>,
        provider_id_filter: Option<&str>,
        force: bool,
    ) -> Result<ProviderBalanceRefreshResponse, ProxyControlError> {
        let refresh = super::providers_api::refresh_provider_balances_for_proxy(
            self,
            route_provider_id_filter,
            provider_id_filter,
            force,
        )
        .await
        .map_err(ProxyControlError::from)?;
        let provider_balances = self
            .state
            .get_provider_balance_view(self.service_name)
            .await;

        Ok(ProviderBalanceRefreshResponse {
            service_name: self.service_name.to_string(),
            refresh,
            provider_balances,
        })
    }

    pub async fn operator_read_capture(
        &self,
    ) -> Result<crate::dashboard_core::OperatorReadCapture, ProxyControlError> {
        super::api_responses::build_operator_read_capture(self)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "failed to build operator read model");
                ProxyControlError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "operator read model unavailable",
                )
            })
    }

    pub async fn operator_read_model(
        &self,
    ) -> Result<crate::dashboard_core::OperatorReadModel, ProxyControlError> {
        self.operator_read_capture()
            .await
            .map(|capture| capture.model)
    }

    pub async fn mutate_operator_routing(
        &self,
        request: super::OperatorRoutingMutationRequest,
    ) -> Result<super::OperatorRoutingMutationResponse, ProxyControlError> {
        super::routing_control::mutate_operator_routing(self, request).await
    }

    pub async fn mutate_operator_session_affinity(
        &self,
        request: super::OperatorSessionAffinityMutationRequest,
    ) -> Result<super::OperatorSessionAffinityMutationResponse, ProxyControlError> {
        super::session_affinity_control::mutate_operator_session_affinity(self, request).await
    }

    pub async fn codex_relay_capabilities(
        &self,
        request: CodexRelayCapabilitiesRequest,
    ) -> Result<CodexRelayCapabilitiesResponse, ProxyControlError> {
        super::codex_relay_capabilities::codex_relay_capabilities_for_proxy(self, request).await
    }

    pub async fn codex_relay_live_smoke(
        &self,
        request: CodexRelayLiveSmokeRequest,
    ) -> Result<CodexRelayLiveSmokeResponse, ProxyControlError> {
        super::codex_relay_live_smoke::codex_relay_live_smoke_for_proxy(self, request).await
    }

    pub async fn reload_runtime_config(&self) -> Result<bool, ProxyControlError> {
        let changed = self.config.force_reload_from_disk().await.map_err(|err| {
            ProxyControlError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                err.to_string(),
            )
        })?;
        if changed {
            super::control_plane_service::prune_runtime_observability_after_reload(self).await;
        }
        Ok(changed)
    }

    pub async fn profiles(&self) -> ProfilesResponse {
        super::api_responses::make_profiles_response(self).await
    }

    pub async fn persisted_provider_specs(
        &self,
    ) -> Result<PersistedProvidersCatalog, ProxyControlError> {
        let snapshot = self.config.capture().await;
        let source = snapshot.config();
        let view =
            super::control_plane_service::service_route_config(source.as_ref(), self.service_name);
        Ok(PersistedProvidersCatalog {
            providers: view
                .providers
                .iter()
                .map(|(name, provider)| {
                    persisted_provider_spec_from_config_for_service(name, provider)
                })
                .collect(),
        })
    }

    pub async fn persisted_routing_spec(&self) -> Result<PersistedRoutingSpec, ProxyControlError> {
        let snapshot = self.config.capture().await;
        let source = snapshot.config();
        let view =
            super::control_plane_service::service_route_config(source.as_ref(), self.service_name);
        Ok(persisted_routing_spec_from_config_for_service(view))
    }

    pub async fn routing_explain(
        &self,
        request: RouteRequestContext,
        session_id: Option<String>,
    ) -> Result<RoutingExplainResponse, ProxyControlError> {
        super::runtime_admin_api::routing_explain_for_proxy(self, request, session_id)
            .await
            .map_err(ProxyControlError::from)
    }
}

async fn wait_for_proxy_shutdown(shutdown_rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() || shutdown_rx.changed().await.is_err() {
            return;
        }
    }
}

fn persisted_provider_spec_from_config_for_service(
    name: &str,
    provider: &crate::config::ProviderConfig,
) -> PersistedProviderSpec {
    let auth = provider.effective_auth();
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
            priority: 0,
            tags: std::collections::BTreeMap::new(),
            limits: crate::config::ProviderConcurrencyLimits::default(),
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
            limits: endpoint.limits.clone(),
        }
    }));

    PersistedProviderSpec {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth_token_env: auth.auth_token_env,
        auth_token_ref: auth.auth_token_ref,
        api_key_env: auth.api_key_env,
        api_key_ref: auth.api_key_ref,
        tags: provider.tags.clone(),
        limits: provider.limits.clone(),
        endpoints,
    }
}

fn persisted_routing_spec_from_config_for_service(
    view: &crate::config::ServiceRouteConfig,
) -> PersistedRoutingSpec {
    let routing = crate::config::effective_routing(view);
    let order = crate::config::resolved_provider_order("persisted-routing", view)
        .unwrap_or_else(|_| view.providers.keys().cloned().collect::<Vec<_>>());
    let entry_node = routing.entry_node();
    PersistedRoutingSpec {
        entry: routing.entry.clone(),
        affinity_policy: routing.affinity_policy,
        scheduling_preset: routing.scheduling_preset,
        fallback_ttl_ms: routing.fallback_ttl_ms,
        reprobe_preferred_after_ms: routing.reprobe_preferred_after_ms,
        routes: routing.routes.clone(),
        policy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RouteStrategy::OrderedFailover),
        order: order.clone(),
        target: entry_node.and_then(|node| node.target.clone()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: entry_node
            .map(|node| node.on_exhausted)
            .unwrap_or(crate::config::RouteExhaustedAction::Continue),
        entry_strategy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RouteStrategy::OrderedFailover),
        expanded_order: order,
        entry_target: entry_node.and_then(|node| node.target.clone()),
        providers: view
            .providers
            .iter()
            .map(
                |(name, provider)| crate::config::PersistedRoutingProviderRef {
                    name: name.clone(),
                    alias: provider.alias.clone(),
                    enabled: provider.enabled,
                    tags: provider.tags.clone(),
                },
            )
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ephemeral_diagnostic_factory_is_explicit_and_isolated() {
        let proxy =
            ProxyService::new_ephemeral_diagnostic(Arc::new(HelperConfig::default()), "codex")
                .expect("build ephemeral diagnostic proxy");

        assert_eq!(proxy.state_handle().runtime_store_handle().path(), None);
    }

    #[test]
    fn persisted_provider_spec_projects_effective_credential_references() {
        let provider = crate::config::ProviderConfig {
            base_url: Some("https://relay.example/v1".to_string()),
            auth: crate::config::UpstreamAuth {
                auth_token_ref: Some(crate::config::CredentialRef::Native {
                    name: "relay.primary".to_string(),
                }),
                api_key_env: Some("RELAY_API_KEY".to_string()),
                ..crate::config::UpstreamAuth::default()
            },
            ..crate::config::ProviderConfig::default()
        };

        let spec = persisted_provider_spec_from_config_for_service("relay", &provider);
        let serialized = serde_json::to_value(&spec).expect("serialize provider spec");

        assert_eq!(serialized["auth_token_ref"]["source"], "native");
        assert_eq!(serialized["auth_token_ref"]["name"], "relay.primary");
        assert_eq!(serialized["api_key_env"], "RELAY_API_KEY");
    }
}
