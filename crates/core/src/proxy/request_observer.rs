use std::sync::Arc;

use axum::http::Method;

use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RetryInfo, ServiceTierLog, log_committed_request_with_debug,
};
use crate::runtime_store::{AttemptHandle, RequestAccountingScope};
use crate::state::{
    FinishRequestParams, ProxyState, RouteDecisionProvenance, SessionIdentitySource,
    SessionRouteAffinitySuccess,
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
    method: String,
    path: String,
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
        method: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            state,
            service_name: service_name.into(),
            method: method.into(),
            path: path.into(),
        }
    }

    pub(super) async fn publish_terminal_once(&self, publication: RequestPublication) -> bool {
        self.publish_terminal_with_accounting(publication, RequestAccountingScope::Economic)
            .await
    }

    pub(super) async fn publish_non_economic_terminal_once(
        &self,
        publication: RequestPublication,
    ) -> bool {
        self.publish_terminal_with_accounting(publication, RequestAccountingScope::NonEconomic)
            .await
    }

    pub(super) async fn publish_terminal_with_accounting(
        &self,
        mut publication: RequestPublication,
        accounting: RequestAccountingScope,
    ) -> bool {
        let include_in_economics = accounting == RequestAccountingScope::Economic;
        if !include_in_economics {
            publication.usage = None;
        }
        self.publish_terminal(publication, include_in_economics)
            .await
    }

    async fn publish_terminal(
        &self,
        publication: RequestPublication,
        include_in_economics: bool,
    ) -> bool {
        let RequestPublication {
            request_id,
            winning_attempt,
            status_code,
            duration_ms,
            ended_at_ms,
            ttfb_ms,
            provider_id,
            endpoint_id,
            provider_endpoint_key,
            upstream_origin,
            session_id,
            session_identity_source,
            cwd,
            model,
            reasoning_effort,
            service_tier,
            reported_model,
            codex_bridge,
            usage,
            route_decision,
            retry,
            http_debug,
            streaming,
            route_affinity_success,
        } = publication;

        let mut retry_for_runtime = retry.clone();
        if let Some(retry) = retry_for_runtime.as_mut() {
            for attempt in &mut retry.route_attempts {
                attempt.http_debug = None;
            }
        }
        let finish = FinishRequestParams {
            id: request_id,
            winning_attempt,
            status_code,
            duration_ms,
            ended_at_ms,
            observed_service_tier: service_tier.actual.clone(),
            reported_model,
            usage: usage.clone(),
            retry: retry_for_runtime,
            ttfb_ms,
            streaming,
        };
        let published = match (include_in_economics, route_affinity_success) {
            (true, Some(success)) => {
                self.state
                    .finish_request_with_session_route_affinity(finish, success)
                    .await
            }
            (false, Some(success)) => {
                self.state
                    .finish_non_economic_request_with_session_route_affinity(finish, success)
                    .await
            }
            (true, None) => self.state.finish_request(finish).await,
            (false, None) => self.state.finish_non_economic_request(finish).await,
        };
        if !published {
            return false;
        }

        log_committed_request_with_debug(
            Some(request_id),
            self.service_name.as_str(),
            self.method.as_str(),
            self.path.as_str(),
            status_code,
            duration_ms,
            ttfb_ms,
            provider_id,
            endpoint_id,
            provider_endpoint_key,
            upstream_origin,
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
        );
        true
    }
}

pub(super) struct RequestPublication {
    pub(super) request_id: u64,
    pub(super) winning_attempt: Option<AttemptHandle>,
    pub(super) status_code: u16,
    pub(super) duration_ms: u64,
    pub(super) ended_at_ms: u64,
    pub(super) ttfb_ms: Option<u64>,
    pub(super) provider_id: Option<String>,
    pub(super) endpoint_id: Option<String>,
    pub(super) provider_endpoint_key: Option<String>,
    pub(super) upstream_origin: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) service_tier: ServiceTierLog,
    pub(super) reported_model: Option<String>,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) usage: Option<UsageMetrics>,
    pub(super) route_decision: Option<RouteDecisionProvenance>,
    pub(super) retry: Option<RetryInfo>,
    pub(super) http_debug: Option<HttpDebugLog>,
    pub(super) streaming: bool,
    pub(super) route_affinity_success: Option<SessionRouteAffinitySuccess>,
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
            winning_attempt: None,
            status_code,
            duration_ms,
            ended_at_ms: started_at_ms + duration_ms,
            ttfb_ms: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_origin: None,
            session_id: None,
            session_identity_source: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: ServiceTierLog::default(),
            reported_model: None,
            codex_bridge: None,
            usage: None,
            route_decision: None,
            retry: None,
            http_debug: None,
            streaming,
            route_affinity_success: None,
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

    #[tokio::test]
    async fn request_debug_log_is_written_only_after_terminal_commit() {
        let _env_guard = env_lock().await;
        let mut scoped = ScopedEnv::default();
        let temp_home = temp_proxy_home("request_debug_log_is_written_only_after_terminal_commit");
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
        state
            .runtime_store_handle()
            .fail_next_logical_terminal_commit_for_test();

        assert!(
            !observer
                .publish_terminal_once(test_publication(request_id, false, 500))
                .await
        );
        assert!(state.list_recent_finished(10).await.is_empty());
        assert!(!crate::logging::request_log_path().exists());

        assert!(
            observer
                .publish_terminal_once(test_publication(request_id, false, 500))
                .await
        );
        let request_log = std::fs::read_to_string(crate::logging::request_log_path())
            .expect("committed request debug log");
        assert!(request_log.contains(&format!(r#""request_id":{request_id}"#)));
    }

    #[tokio::test]
    async fn poisoned_upstream_url_is_redacted_from_terminal_request_logs() {
        let _env_guard = env_lock().await;
        let mut scoped = ScopedEnv::default();
        let temp_home = temp_proxy_home("poisoned_upstream_url_is_redacted");
        let control_trace_path = temp_home.join("logs").join("control_trace.jsonl");
        unsafe {
            scoped.set_path("CODEX_HELPER_HOME", temp_home.as_path());
            scoped.set_path(
                "CODEX_HELPER_CONTROL_TRACE_PATH",
                control_trace_path.as_path(),
            );
            scoped.set("CODEX_HELPER_CONTROL_TRACE", "1");
            scoped.set("CODEX_HELPER_HTTP_DEBUG_SPLIT", "1");
        }

        let state = ProxyState::new();
        let request_id = state
            .begin_request_for_test()
            .started_at_ms(1_000)
            .begin()
            .await;
        let observer = RequestObserver::from_parts(state, "codex", "POST", "/v1/responses");
        let poisoned =
            "https://user:secret@relay.example.test:8443/private/secret-path?token=hidden#fragment";
        let mut publication = test_publication(request_id, false, 500);
        publication.upstream_origin = Some(poisoned.to_string());
        publication.route_decision = Some(RouteDecisionProvenance {
            effective_upstream_base_url: Some(crate::state::ResolvedRouteValue::new(
                poisoned,
                crate::state::RouteValueSource::RuntimeFallback,
            )),
            provider_id: Some("relay".to_string()),
            endpoint_id: Some("default".to_string()),
            ..RouteDecisionProvenance::default()
        });
        publication.http_debug = Some(HttpDebugLog {
            route_attempt_index: Some(0),
            request_body_len: Some(2),
            upstream_request_body_len: Some(2),
            upstream_headers_ms: Some(5),
            upstream_first_chunk_ms: None,
            upstream_body_read_ms: Some(1),
            upstream_error_class: Some("http_500".to_string()),
            upstream_error_hint: None,
            upstream_cf_ray: None,
            client_uri: "/v1/responses".to_string(),
            upstream_origin: Some(poisoned.to_string()),
            upstream_uri: Some(poisoned.to_string()),
            client_headers: Vec::new(),
            upstream_request_headers: Vec::new(),
            auth_resolution: None,
            client_body: None,
            upstream_request_body: None,
            upstream_response_headers: None,
            upstream_response_body: None,
            upstream_error: None,
        });

        assert!(observer.publish_terminal_once(publication).await);

        let paths = [
            crate::logging::request_log_path(),
            temp_home.join("logs").join("requests_debug.jsonl"),
            control_trace_path,
        ];
        for path in paths {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            assert!(text.contains("codex/provider-a/endpoint-a"));
            assert!(text.contains("https://relay.example.test:8443"));
            assert!(!text.contains("upstream_base_url"));
            assert!(!text.contains("station_name"));
            for secret in ["user:secret", "token=hidden", "fragment"] {
                assert!(!text.contains(secret), "{} leaked {secret}", path.display());
            }
        }
        let debug_text =
            std::fs::read_to_string(temp_home.join("logs").join("requests_debug.jsonl"))
                .expect("read sanitized HTTP debug log");
        assert!(debug_text.contains(r#""upstream_uri":"/private/secret-path""#));
    }

    fn test_publication(request_id: u64, streaming: bool, status_code: u16) -> RequestPublication {
        let mut publication =
            RequestPublication::new_terminal(request_id, status_code, 25, 1_000, streaming);
        publication.ttfb_ms = Some(5);
        publication.provider_id = Some("provider-a".to_string());
        publication.endpoint_id = Some("endpoint-a".to_string());
        publication.provider_endpoint_key = Some("codex/provider-a/endpoint-a".to_string());
        publication.upstream_origin = Some("http://upstream.test".to_string());
        publication.session_id = Some("session-a".to_string());
        publication.model = Some("gpt-5".to_string());
        publication.reasoning_effort = Some("medium".to_string());
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
