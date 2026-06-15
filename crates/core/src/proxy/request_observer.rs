use std::sync::Arc;

use axum::http::Method;

use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RetryInfo, ServiceTierLog, log_request_with_debug,
};
use crate::state::{
    FinishRequestParams, ProxyState, RouteDecisionProvenance, SessionIdentitySource,
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

        let published = self
            .state
            .finish_request(FinishRequestParams {
                id: request_id,
                status_code,
                duration_ms,
                ended_at_ms,
                observed_service_tier: service_tier.actual.clone(),
                usage: usage.clone(),
                retry: retry.clone(),
                ttfb_ms,
                streaming,
            })
            .await;
        if !published {
            return false;
        }

        log_request_with_debug(
            Some(request_id),
            self.service_name.as_str(),
            self.method.as_str(),
            self.path.as_str(),
            status_code,
            duration_ms,
            ttfb_ms,
            station_name.as_deref(),
            provider_id,
            endpoint_id,
            provider_endpoint_key,
            upstream_base_url.as_str(),
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

    fn test_publication(request_id: u64, streaming: bool, status_code: u16) -> RequestPublication {
        RequestPublication {
            request_id,
            status_code,
            duration_ms: 25,
            ended_at_ms: 1_025,
            ttfb_ms: Some(5),
            station_name: Some("station-a".to_string()),
            provider_id: Some("provider-a".to_string()),
            endpoint_id: Some("endpoint-a".to_string()),
            provider_endpoint_key: Some("codex:provider-a:endpoint-a".to_string()),
            upstream_base_url: "http://upstream.test".to_string(),
            session_id: Some("session-a".to_string()),
            session_identity_source: None,
            cwd: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: ServiceTierLog::default(),
            codex_bridge: None,
            usage: None,
            route_decision: None,
            retry: None,
            http_debug: None,
            streaming,
        }
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
