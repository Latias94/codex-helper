use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::Client;

use crate::config::{ProxyConfig, ProxyConfigV4, ServiceConfigManager};
use crate::filter::RequestFilter;
use crate::lb::LbState;
use crate::state::{ProxyState, SessionBinding, SessionContinuityMode};

use super::profile_defaults::effective_default_profile_name;
use super::{ProxyService, RuntimeConfig};

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
}
