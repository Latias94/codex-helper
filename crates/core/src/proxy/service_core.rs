use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::Client;

use crate::config::{
    PersistedProviderSpec, PersistedProvidersCatalog, PersistedRoutingSpec, ProxyConfig,
    ProxyConfigV4, ServiceConfigManager,
};
use crate::filter::RequestFilter;
use crate::lb::LbState;
use crate::routing_explain::RoutingExplainResponse;
use crate::routing_ir::RouteRequestContext;
use crate::state::{ProxyState, SessionBinding, SessionContinuityMode};

use super::profile_defaults::effective_default_profile_name;
use super::{
    PersistedRoutingUpsertRequest, ProfilesResponse, ProviderBalanceRefreshResponse,
    ProxyControlError, ProxyService, ReloadResult, RuntimeConfig, RuntimeStatusResponse,
};

impl ProxyService {
    pub fn new(
        client: Client,
        config: Arc<ProxyConfig>,
        service_name: &'static str,
        lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    ) -> Self {
        Self::new_with_v4_source(client, config, None, service_name, lb_states)
    }

    pub fn new_with_v4_source(
        client: Client,
        config: Arc<ProxyConfig>,
        v4_source: Option<Arc<ProxyConfigV4>>,
        service_name: &'static str,
        lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    ) -> Self {
        let state = ProxyState::new_with_lb_states(Some(lb_states.clone()));
        ProxyState::spawn_cleanup_task(state.clone());
        if !cfg!(test) {
            let state = state.clone();
            let log_path = crate::logging::request_log_path();
            let mut base_url_to_provider_id = HashMap::new();
            let mgr = match service_name {
                "claude" => &config.claude,
                _ => &config.codex,
            };
            for svc in mgr.stations().values() {
                for up in &svc.upstreams {
                    if let Some(pid) = up.tags.get("provider_id") {
                        base_url_to_provider_id.insert(up.base_url.clone(), pid.clone());
                    }
                }
            }
            tokio::spawn(async move {
                let _ = state
                    .replay_usage_from_requests_log(service_name, log_path, base_url_to_provider_id)
                    .await;
            });
        }
        Self {
            client,
            config: Arc::new(RuntimeConfig::new_with_v4(config, v4_source)),
            service_name,
            lb_states,
            filter: RequestFilter::new(),
            state,
        }
    }

    pub(super) fn service_manager<'a>(&self, cfg: &'a ProxyConfig) -> &'a ServiceConfigManager {
        match self.service_name {
            "codex" => &cfg.codex,
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        }
    }

    pub(super) async fn ensure_default_session_binding(
        &self,
        mgr: &ServiceConfigManager,
        session_id: &str,
        now_ms: u64,
    ) -> Option<SessionBinding> {
        if let Some(binding) = self.state.get_session_binding(session_id).await {
            self.state.touch_session_binding(session_id, now_ms).await;
            return Some(binding);
        }

        let profile_name =
            effective_default_profile_name(self.state.as_ref(), self.service_name, mgr).await?;
        let profile = crate::config::resolve_service_profile(mgr, profile_name.as_str()).ok()?;
        let binding = SessionBinding {
            session_id: session_id.to_string(),
            profile_name: Some(profile_name),
            station_name: profile.station.clone(),
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

    pub fn spawn_initial_balance_refresh(&self) {
        if cfg!(test) {
            return;
        }

        let proxy = self.clone();
        tokio::spawn(async move {
            match super::providers_api::refresh_provider_balances_for_proxy(&proxy, None, None)
                .await
            {
                Ok(summary) => {
                    tracing::info!(
                        "initial provider balance refresh finished: attempted={}, refreshed={}, failed={}, missing_token={}, auto_refreshed={}",
                        summary.attempted,
                        summary.refreshed,
                        summary.failed,
                        summary.missing_token,
                        summary.auto_refreshed
                    );
                }
                Err((status, message)) => {
                    tracing::warn!(
                        "initial provider balance refresh failed before polling: status={}, {}",
                        status,
                        message
                    );
                }
            }
        });
    }

    pub async fn refresh_provider_balances(
        &self,
        station_name_filter: Option<&str>,
        provider_id_filter: Option<&str>,
    ) -> Result<ProviderBalanceRefreshResponse, ProxyControlError> {
        let refresh = super::providers_api::refresh_provider_balances_for_proxy(
            self,
            station_name_filter,
            provider_id_filter,
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

    pub async fn runtime_status(&self) -> RuntimeStatusResponse {
        super::api_responses::build_runtime_status_response(self).await
    }

    pub async fn reload_runtime_config(&self) -> Result<ReloadResult, ProxyControlError> {
        let changed = self.config.force_reload_from_disk().await.map_err(|err| {
            ProxyControlError::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                err.to_string(),
            )
        })?;
        if changed {
            super::control_plane_service::prune_runtime_observability_after_reload(self).await;
        }
        let status = self.runtime_status().await;
        Ok(super::api_responses::build_reload_result(changed, status))
    }

    pub async fn profiles(&self) -> ProfilesResponse {
        super::api_responses::make_profiles_response(self).await
    }

    pub async fn set_runtime_default_profile(
        &self,
        profile_name: Option<String>,
    ) -> Result<(), ProxyControlError> {
        let profile_name = normalize_optional_control_name(profile_name);

        if let Some(profile_name) = profile_name {
            let cfg = self.config.snapshot().await;
            let mgr = self.service_manager(cfg.as_ref());
            if mgr.profile(profile_name.as_str()).is_none() {
                return Err(ProxyControlError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    format!("profile '{}' not found", profile_name),
                ));
            }
            let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
                .map_err(|err| {
                    ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
                })?;
            crate::config::validate_profile_station_compatibility(
                self.service_name,
                mgr,
                profile_name.as_str(),
                &resolved,
            )
            .map_err(|err| {
                ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
            })?;
            self.state
                .set_runtime_default_profile_override(
                    self.service_name.to_string(),
                    profile_name,
                    crate::logging::now_ms(),
                )
                .await;
        } else {
            self.state
                .clear_runtime_default_profile_override(self.service_name)
                .await;
        }

        Ok(())
    }

    pub async fn set_persisted_default_profile(
        &self,
        profile_name: Option<String>,
    ) -> Result<ProfilesResponse, ProxyControlError> {
        let profile_name = normalize_optional_control_name(profile_name);

        if let super::control_plane_service::PersistedProxySettingsDocument::V4(mut document) =
            super::control_plane_service::load_persisted_proxy_settings_document()
                .await
                .map_err(ProxyControlError::from)?
        {
            if let Some(profile_name) = profile_name.as_deref() {
                let view =
                    super::control_plane_service::service_view_v4(&document, self.service_name);
                if !view.profiles.contains_key(profile_name) {
                    return Err(ProxyControlError::new(
                        axum::http::StatusCode::NOT_FOUND,
                        format!("profile '{}' not found", profile_name),
                    ));
                }
                let runtime = crate::config::compile_v4_to_runtime(&document).map_err(|err| {
                    ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
                })?;
                let mgr = super::persisted_registry_api::runtime_service_manager_for_document(
                    &runtime,
                    self.service_name,
                );
                let resolved =
                    crate::config::resolve_service_profile(mgr, profile_name).map_err(|err| {
                        ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
                    })?;
                crate::config::validate_profile_station_compatibility(
                    self.service_name,
                    mgr,
                    profile_name,
                    &resolved,
                )
                .map_err(|err| {
                    ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
                })?;
            }
            let view =
                super::control_plane_service::service_view_v4_mut(&mut document, self.service_name);
            view.default_profile = profile_name;
            super::control_plane_service::save_persisted_proxy_settings_document_and_reload(
                self,
                super::control_plane_service::PersistedProxySettingsDocument::V4(document),
            )
            .await
            .map_err(ProxyControlError::from)?;
            return Ok(self.profiles().await);
        }

        let cfg_snapshot = self.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr =
            super::control_plane_service::runtime_service_manager_mut(&mut cfg, self.service_name);

        if let Some(profile_name) = profile_name.as_deref() {
            if mgr.profile(profile_name).is_none() {
                return Err(ProxyControlError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    format!("profile '{}' not found", profile_name),
                ));
            }
            let resolved =
                crate::config::resolve_service_profile(mgr, profile_name).map_err(|err| {
                    ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
                })?;
            crate::config::validate_profile_station_compatibility(
                self.service_name,
                mgr,
                profile_name,
                &resolved,
            )
            .map_err(|err| {
                ProxyControlError::new(axum::http::StatusCode::BAD_REQUEST, err.to_string())
            })?;
        }
        mgr.default_profile = profile_name;

        super::control_plane_service::save_runtime_profile_settings_and_reload(self, cfg)
            .await
            .map_err(ProxyControlError::from)
    }

    pub async fn persisted_provider_specs(
        &self,
    ) -> Result<PersistedProvidersCatalog, ProxyControlError> {
        let catalog = match super::control_plane_service::load_persisted_proxy_settings_document()
            .await
            .map_err(ProxyControlError::from)?
        {
            super::control_plane_service::PersistedProxySettingsDocument::V2(cfg) => {
                crate::config::build_persisted_provider_catalog(
                    super::control_plane_service::service_view_v2(&cfg, self.service_name),
                )
            }
            super::control_plane_service::PersistedProxySettingsDocument::V4(cfg) => {
                PersistedProvidersCatalog {
                    providers: super::control_plane_service::service_view_v4(
                        &cfg,
                        self.service_name,
                    )
                    .providers
                    .iter()
                    .map(|(name, provider)| {
                        persisted_provider_spec_from_v4_for_service(name, provider)
                    })
                    .collect(),
                }
            }
        };
        Ok(catalog)
    }

    pub async fn upsert_persisted_provider_spec(
        &self,
        provider_name: String,
        provider: PersistedProviderSpec,
    ) -> Result<(), ProxyControlError> {
        super::persisted_registry_api::upsert_persisted_provider_spec_for_proxy(
            self,
            provider_name,
            provider.into(),
        )
        .await
        .map(|_| ())
        .map_err(ProxyControlError::from)
    }

    pub async fn persisted_routing_spec(&self) -> Result<PersistedRoutingSpec, ProxyControlError> {
        match super::control_plane_service::load_persisted_proxy_settings_document()
            .await
            .map_err(ProxyControlError::from)?
        {
            super::control_plane_service::PersistedProxySettingsDocument::V4(cfg) => {
                let view = super::control_plane_service::service_view_v4(&cfg, self.service_name);
                Ok(persisted_routing_spec_from_v4_for_service(view))
            }
            super::control_plane_service::PersistedProxySettingsDocument::V2(_) => {
                Err(ProxyControlError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "routing API requires a version = 5 route graph config",
                ))
            }
        }
    }

    pub async fn upsert_persisted_routing_spec(
        &self,
        payload: PersistedRoutingUpsertRequest,
    ) -> Result<PersistedRoutingSpec, ProxyControlError> {
        super::persisted_registry_api::upsert_persisted_routing_spec_for_proxy(self, payload)
            .await
            .map_err(ProxyControlError::from)
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

fn normalize_optional_control_name(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn persisted_provider_spec_from_v4_for_service(
    name: &str,
    provider: &crate::config::ProviderConfigV4,
) -> PersistedProviderSpec {
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
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
        }
    }));

    PersistedProviderSpec {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth_token_env: provider.inline_auth.auth_token_env.clone(),
        api_key_env: provider.inline_auth.api_key_env.clone(),
        tags: provider.tags.clone(),
        endpoints,
    }
}

fn persisted_routing_spec_from_v4_for_service(
    view: &crate::config::ServiceViewV4,
) -> PersistedRoutingSpec {
    let routing = crate::config::effective_v4_routing(view);
    let order = crate::config::resolved_v4_provider_order("persisted-routing", view)
        .unwrap_or_else(|_| view.providers.keys().cloned().collect::<Vec<_>>());
    let entry_node = routing.entry_node();
    PersistedRoutingSpec {
        entry: routing.entry.clone(),
        affinity_policy: routing.affinity_policy,
        fallback_ttl_ms: routing.fallback_ttl_ms,
        reprobe_preferred_after_ms: routing.reprobe_preferred_after_ms,
        routes: routing.routes.clone(),
        policy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover),
        order: order.clone(),
        target: entry_node.and_then(|node| node.target.clone()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: entry_node
            .map(|node| node.on_exhausted)
            .unwrap_or(crate::config::RoutingExhaustedActionV4::Continue),
        entry_strategy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover),
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
