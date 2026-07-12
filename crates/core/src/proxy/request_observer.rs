use std::sync::Arc;

use axum::http::Method;

use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RetryInfo, ServiceTierLog, log_finished_request_with_debug,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{
    FinishRequestMetadata, FinishRequestParams, ProxyState, RouteDecisionProvenance,
    SessionIdentitySource,
};
use crate::usage::UsageMetrics;

use super::ProxyService;

#[derive(Debug, Default)]
pub(super) struct RequestPublicationGate {
    published: bool,
}

impl RequestPublicationGate {
    pub(super) fn mark_published(&mut self) -> bool {
        if self.published {
            return false;
        }
        self.published = true;
        true
    }
}

#[derive(Clone)]
pub(super) struct RequestObserver {
    state: Arc<ProxyState>,
    service_name: String,
}

impl RequestObserver {
    pub(super) fn new(proxy: &ProxyService, method: &Method, path: &str) -> Self {
        Self::from_parts(
            proxy.state.clone(),
            proxy.service_name,
            method.as_str(),
            path,
        )
    }

    pub(super) fn from_parts(
        state: Arc<ProxyState>,
        service_name: impl Into<String>,
        _method: impl Into<String>,
        _path: impl Into<String>,
    ) -> Self {
        Self {
            state,
            service_name: service_name.into(),
        }
    }

    pub(super) async fn publish_terminal_once(&self, publication: RequestPublication) -> bool {
        let RequestPublication {
            request_id,
            status_code,
            duration_ms,
            ended_at_ms,
            ttfb_ms,
            station_name,
            provider_id,
            endpoint_id,
            provider_endpoint_key,
            upstream_base_url,
            session_id,
            session_identity_source,
            cwd,
            model,
            reasoning_effort,
            service_tier,
            codex_bridge,
            usage,
            route_decision,
            retry,
            http_debug,
            streaming,
        } = publication;
        let provider_endpoint = provider_endpoint_from_publication(
            self.service_name.as_str(),
            provider_id.as_deref(),
            endpoint_id.as_deref(),
            provider_endpoint_key.as_deref(),
        );

        let Some(finished) = self
            .state
            .finish_request_with_endpoint(
                FinishRequestParams {
                    id: request_id,
                    status_code,
                    duration_ms,
                    ended_at_ms,
                    observed_service_tier: service_tier.actual.clone(),
                    usage,
                    retry,
                    ttfb_ms,
                    streaming,
                },
                provider_endpoint,
                FinishRequestMetadata {
                    station_name,
                    provider_id,
                    upstream_base_url: (!upstream_base_url.trim().is_empty()
                        && upstream_base_url != "-")
                        .then_some(upstream_base_url),
                    session_id,
                    session_identity_source,
                    cwd,
                    model,
                    reasoning_effort,
                    route_decision,
                },
            )
            .await
        else {
            return false;
        };

        let outcome =
            log_finished_request_with_debug(&finished, service_tier, codex_bridge, http_debug);
        if !outcome.accounting_appended {
            self.state.record_accounting_append_failure(&finished).await;
        }
        true
    }
}

fn provider_endpoint_from_publication(
    service_name: &str,
    provider_id: Option<&str>,
    endpoint_id: Option<&str>,
    stable_key: Option<&str>,
) -> Option<ProviderEndpointKey> {
    if let (Some(provider_id), Some(endpoint_id)) = (provider_id, endpoint_id) {
        return Some(ProviderEndpointKey::new(
            service_name,
            provider_id,
            endpoint_id,
        ));
    }
    let mut parts = stable_key?.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(service), Some(provider), Some(endpoint), None)
            if !service.is_empty() && !provider.is_empty() && !endpoint.is_empty() =>
        {
            Some(ProviderEndpointKey::new(service, provider, endpoint))
        }
        _ => None,
    }
}

pub(super) struct RequestPublication {
    pub(super) request_id: u64,
    pub(super) status_code: u16,
    pub(super) duration_ms: u64,
    pub(super) ended_at_ms: u64,
    pub(super) ttfb_ms: Option<u64>,
    pub(super) station_name: Option<String>,
    pub(super) provider_id: Option<String>,
    pub(super) endpoint_id: Option<String>,
    pub(super) provider_endpoint_key: Option<String>,
    pub(super) upstream_base_url: String,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) service_tier: ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) usage: Option<UsageMetrics>,
    pub(super) route_decision: Option<RouteDecisionProvenance>,
    pub(super) retry: Option<RetryInfo>,
    pub(super) http_debug: Option<HttpDebugLog>,
    pub(super) streaming: bool,
}

impl RequestPublication {
    pub(super) fn new_terminal(
        request_id: u64,
        status_code: u16,
        duration_ms: u64,
        started_at_ms: u64,
        streaming: bool,
    ) -> Self {
        Self {
            request_id,
            status_code,
            duration_ms,
            ended_at_ms: started_at_ms + duration_ms,
            ttfb_ms: None,
            station_name: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: "-".to_string(),
            session_id: None,
            session_identity_source: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: ServiceTierLog::default(),
            codex_bridge: None,
            usage: None,
            route_decision: None,
            retry: None,
            http_debug: None,
            streaming,
        }
    }

    pub(super) fn with_route_decision_model(mut self) -> Self {
        self.model = self
            .route_decision
            .as_ref()
            .and_then(|decision| decision.effective_model.as_ref())
            .map(|model| model.value.clone());
        self
    }

    pub(super) fn failure_without_upstream(
        request_id: u64,
        status_code: u16,
        duration_ms: u64,
        started_at_ms: u64,
    ) -> Self {
        Self::new_terminal(request_id, status_code, duration_ms, started_at_ms, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::ProviderBalanceSnapshot;
    use crate::quota_pool::{QuotaObservationContext, QuotaScope};
    use crate::state::ProxyState;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    #[derive(Default)]
    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        unsafe fn set(&mut self, key: &str, value: &str) {
            if !self.saved.iter().any(|(saved_key, _)| saved_key == key) {
                self.saved.push((key.to_string(), std::env::var(key).ok()));
            }
            unsafe {
                std::env::set_var(key, value);
            }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            unsafe {
                self.set(key, value.to_string_lossy().as_ref());
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => unsafe {
                        std::env::set_var(key, value);
                    },
                    None => unsafe {
                        std::env::remove_var(key);
                    },
                }
            }
        }
    }

    #[test]
    fn publication_gate_allows_only_one_terminal_publish() {
        let mut gate = RequestPublicationGate::default();

        assert!(gate.mark_published());
        assert!(!gate.mark_published());
        assert!(!gate.mark_published());
    }

    #[tokio::test]
    async fn non_stream_publication_is_exactly_once() {
        let _env_guard = env_lock().await;
        let mut scoped = ScopedEnv::default();
        let temp_home = temp_proxy_home("non_stream_publication_is_exactly_once");
        unsafe {
            scoped.set_path("CODEX_HELPER_HOME", temp_home.as_path());
            scoped.set("CODEX_HELPER_QUOTA_SAMPLE_STATE", "off");
            scoped.set("CODEX_HELPER_QUOTA_IDENTITY_PATH", "off");
        }

        let state = ProxyState::new();
        let repository = temp_home.join("repository");
        let nested = repository.join("crates").join("core");
        std::fs::create_dir_all(repository.join(".git")).expect("create git marker");
        std::fs::create_dir_all(&nested).expect("create nested cwd");
        let endpoint = ProviderEndpointKey::new("codex", "provider-a", "endpoint-a");
        let mut quota_context = QuotaObservationContext::new("https://relay.example");
        quota_context.scope = QuotaScope::Account;
        quota_context.explicit_pool_id = Some("shared-package".to_string());
        let mut quota_snapshot =
            ProviderBalanceSnapshot::new("provider-a", "station-a", 0, "test", 900, None);
        quota_snapshot.quota_used_usd = Some("1".to_string());
        quota_snapshot.quota_remaining_usd = Some("9".to_string());
        quota_snapshot.quota_limit_usd = Some("10".to_string());
        quota_snapshot.refresh_status(900);
        state
            .record_quota_snapshot(endpoint, quota_context, &quota_snapshot)
            .await;
        let request_id = state
            .begin_request_for_test()
            .cwd(nested.to_string_lossy())
            .started_at_ms(1_000)
            .begin()
            .await;
        let observer = RequestObserver::from_parts(state.clone(), "codex", "POST", "/v1/responses");

        assert!(
            observer
                .publish_terminal_once(test_publication(request_id, false, 200))
                .await
        );
        assert!(
            !observer
                .publish_terminal_once(test_publication(request_id, false, 500))
                .await
        );

        let recent = state.list_recent_finished(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, request_id);
        assert_eq!(recent[0].status_code, 200);
        assert!(!recent[0].streaming);
        assert_eq!(
            recent[0].accounting.project.kind,
            crate::state::ProjectIdentityKind::GitRoot
        );
        assert_eq!(
            recent[0]
                .accounting
                .pool_membership
                .as_ref()
                .map(|membership| membership.revision),
            Some(0)
        );

        let request_log = std::fs::read_to_string(crate::logging::request_log_path())
            .expect("read immutable request accounting log");
        let rows = request_log.lines().collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        let logged: serde_json::Value =
            serde_json::from_str(rows[0]).expect("request accounting row");
        assert_eq!(logged["timestamp_ms"].as_u64(), Some(recent[0].ended_at_ms));
        assert_eq!(logged["timestamp_ms"].as_u64(), Some(1_025));
        assert_eq!(
            logged.get("cost"),
            serde_json::to_value(&recent[0].cost).ok().as_ref()
        );
        assert_eq!(
            logged.get("accounting"),
            serde_json::to_value(&recent[0].accounting).ok().as_ref()
        );
    }

    #[tokio::test]
    async fn stream_publication_is_exactly_once() {
        let _env_guard = env_lock().await;
        let mut scoped = ScopedEnv::default();
        let temp_home = temp_proxy_home("stream_publication_is_exactly_once");
        unsafe {
            scoped.set_path("CODEX_HELPER_HOME", temp_home.as_path());
        }

        let state = ProxyState::new();
        let request_id = state
            .begin_request_for_test()
            .started_at_ms(1_000)
            .begin()
            .await;
        let observer = RequestObserver::from_parts(state.clone(), "codex", "POST", "/v1/responses");

        assert!(
            observer
                .publish_terminal_once(test_publication(request_id, true, 200))
                .await
        );
        assert!(
            !observer
                .publish_terminal_once(test_publication(request_id, true, 500))
                .await
        );

        let recent = state.list_recent_finished(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, request_id);
        assert_eq!(recent[0].status_code, 200);
        assert!(recent[0].streaming);
    }

    fn test_publication(request_id: u64, streaming: bool, status_code: u16) -> RequestPublication {
        let mut publication =
            RequestPublication::new_terminal(request_id, status_code, 25, 1_000, streaming);
        publication.ttfb_ms = Some(5);
        publication.station_name = Some("station-a".to_string());
        publication.provider_id = Some("provider-a".to_string());
        publication.endpoint_id = Some("endpoint-a".to_string());
        publication.provider_endpoint_key = Some("codex/provider-a/endpoint-a".to_string());
        publication.upstream_base_url = "http://upstream.test".to_string();
        publication.session_id = Some("session-a".to_string());
        publication.model = Some("gpt-5".to_string());
        publication.reasoning_effort = Some("medium".to_string());
        publication.usage = Some(UsageMetrics {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            ..UsageMetrics::default()
        });
        publication
    }

    fn temp_proxy_home(test_name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push("codex-helper-request-observer-tests");
        path.push(format!("{test_name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp home");
        path
    }
}
