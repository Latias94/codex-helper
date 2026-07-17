use sha2::{Digest, Sha256};

use super::{
    ProxyState, SessionIdentitySource, SessionRouteAffinity, SessionRouteAffinityTarget,
    SessionRouteControlGuard,
};
use crate::runtime_store::RuntimeStoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRouteAffinityControlCommand {
    Clear,
    Rebind(SessionRouteAffinityTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRouteAffinityControlStatus {
    Applied,
    Unchanged,
    Conflict,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRouteAffinityControlCommit {
    pub status: SessionRouteAffinityControlStatus,
    pub affinity: Option<SessionRouteAffinity>,
}

pub fn session_route_affinity_revision(affinity: &SessionRouteAffinity) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codex-helper:session-route-affinity-control:v1\0");
    hash_text(&mut digest, affinity.route_graph_key.as_str());
    digest.update([session_identity_source_tag(
        affinity.session_identity_source,
    )]);
    hash_text(
        &mut digest,
        affinity.provider_endpoint.service_name.as_str(),
    );
    hash_text(&mut digest, affinity.provider_endpoint.provider_id.as_str());
    hash_text(&mut digest, affinity.provider_endpoint.endpoint_id.as_str());
    hash_text(&mut digest, affinity.upstream_base_url.as_str());
    digest.update((affinity.route_path.len() as u64).to_be_bytes());
    for segment in &affinity.route_path {
        hash_text(&mut digest, segment);
    }
    digest.update(affinity.last_selected_at_ms.to_be_bytes());
    digest.update(affinity.last_changed_at_ms.to_be_bytes());
    hash_text(&mut digest, affinity.change_reason.as_str());
    format!("affinity:v1:{:x}", digest.finalize())
}

fn hash_text(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

fn session_identity_source_tag(source: Option<SessionIdentitySource>) -> u8 {
    match source {
        None => 0,
        Some(SessionIdentitySource::Header) => 1,
        Some(SessionIdentitySource::BodySessionId) => 2,
        Some(SessionIdentitySource::PromptCacheKey) => 3,
        Some(SessionIdentitySource::MetadataSessionId) => 4,
        Some(SessionIdentitySource::PreviousResponseId) => 5,
    }
}

impl ProxyState {
    pub async fn compare_and_mutate_session_route_affinity(
        &self,
        session_id: &str,
        expected_revision: Option<&str>,
        command: SessionRouteAffinityControlCommand,
        now_ms: u64,
    ) -> Result<SessionRouteAffinityControlCommit, RuntimeStoreError> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err(RuntimeStoreError::InvariantViolation {
                entity: "session route affinity control",
                id: session_id.to_string(),
                detail: "session_id is empty".to_string(),
            });
        }

        let route_control_guard = self.lock_session_route_control(session_id).await;
        self.compare_and_mutate_session_route_affinity_with_control(
            &route_control_guard,
            expected_revision,
            command,
            now_ms,
        )
        .await
    }

    pub(crate) async fn compare_and_mutate_session_route_affinity_with_control(
        &self,
        route_control_guard: &SessionRouteControlGuard,
        expected_revision: Option<&str>,
        command: SessionRouteAffinityControlCommand,
        now_ms: u64,
    ) -> Result<SessionRouteAffinityControlCommit, RuntimeStoreError> {
        self.validate_session_route_control_guard(route_control_guard)?;
        let session_id = route_control_guard.session_id();
        let is_active = {
            let projection = self.request_lifecycle_projection.read().await;
            projection.active_requests.values().any(|request| {
                request
                    .session_id
                    .as_deref()
                    .is_some_and(|active_session_id| active_session_id == session_id)
            })
        };
        if is_active {
            let _affinity_update_guard = self.session_route_affinity_updates.lock().await;
            let current = self.read_session_route_affinity(session_id, now_ms)?;
            return Ok(SessionRouteAffinityControlCommit {
                status: SessionRouteAffinityControlStatus::Busy,
                affinity: current,
            });
        }

        let _affinity_update_guard = self.session_route_affinity_updates.lock().await;
        let current = self.read_session_route_affinity(session_id, now_ms)?;
        let current_revision = current.as_ref().map(session_route_affinity_revision);
        if current_revision.as_deref() != expected_revision {
            return Ok(SessionRouteAffinityControlCommit {
                status: SessionRouteAffinityControlStatus::Conflict,
                affinity: current,
            });
        }

        let (status, affinity) = match command {
            SessionRouteAffinityControlCommand::Clear => match current {
                None => (SessionRouteAffinityControlStatus::Unchanged, None),
                Some(_) => {
                    let removed = self.with_runtime_store_blocking(|runtime_store| {
                        runtime_store.delete_session_affinity(session_id)
                    })?;
                    debug_assert!(removed, "CAS-protected affinity row must exist");
                    (SessionRouteAffinityControlStatus::Applied, None)
                }
            },
            SessionRouteAffinityControlCommand::Rebind(target) => {
                let Some(current) = current else {
                    return Ok(SessionRouteAffinityControlCommit {
                        status: SessionRouteAffinityControlStatus::Conflict,
                        affinity: None,
                    });
                };
                if target.same_target(&current) {
                    (SessionRouteAffinityControlStatus::Unchanged, Some(current))
                } else {
                    let affinity = SessionRouteAffinity {
                        route_graph_key: target.route_graph_key,
                        session_identity_source: target
                            .session_identity_source
                            .or(current.session_identity_source),
                        provider_endpoint: target.provider_endpoint,
                        upstream_base_url: target.upstream_base_url,
                        route_path: target.route_path,
                        last_selected_at_ms: now_ms,
                        last_changed_at_ms: now_ms,
                        change_reason: "operator_rebind".to_string(),
                    };
                    self.with_runtime_store_blocking(|runtime_store| {
                        runtime_store.upsert_session_affinity(
                            super::session_affinity_record(session_id, &affinity),
                            super::session_affinity_limit(self.session_route_affinity_max_entries),
                        )
                    })?;
                    (SessionRouteAffinityControlStatus::Applied, Some(affinity))
                }
            }
        };

        if status == SessionRouteAffinityControlStatus::Applied {
            self.session_route_reservations
                .lock()
                .await
                .remove(session_id);
            self.notify_state_changed();
        }
        Ok(SessionRouteAffinityControlCommit { status, affinity })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::pricing::capture_operator_model_price_catalog;
    use crate::provider_catalog::ProviderCatalogSnapshot;
    use crate::runtime_identity::ProviderEndpointKey;

    fn target(provider_id: &str) -> SessionRouteAffinityTarget {
        SessionRouteAffinityTarget {
            route_graph_key: "route:v1:test".to_string(),
            session_identity_source: Some(SessionIdentitySource::Header),
            provider_endpoint: ProviderEndpointKey::new("codex", provider_id, "default"),
            upstream_base_url: format!("https://{provider_id}.example.test/v1"),
            route_path: vec!["main".to_string(), provider_id.to_string()],
        }
    }

    async fn begin_request_with_guard(
        state: &ProxyState,
        guard: &SessionRouteControlGuard,
        started_at_ms: u64,
    ) -> Result<u64, RuntimeStoreError> {
        let policy_revision = state
            .capture_provider_policy_snapshot()
            .await
            .policy_revision;
        state
            .try_begin_request_with_session_route_control(
                Some(guard),
                "codex",
                "POST",
                "/v1/responses",
                Some(SessionIdentitySource::Header),
                None,
                None,
                None,
                Some("gpt-5".to_string()),
                Some("gpt-5".to_string()),
                None,
                None,
                None,
                Arc::new(ProviderCatalogSnapshot::bundled()),
                Arc::new(capture_operator_model_price_catalog()),
                1,
                "test-runtime".to_string(),
                policy_revision,
                started_at_ms,
            )
            .await
    }

    #[tokio::test]
    async fn affinity_control_rebind_and_clear_are_revision_guarded() {
        let state = ProxyState::new();
        let initial = state
            .record_session_route_affinity_success(None, "session-a", target("input"), None, 100)
            .await
            .expect("record initial affinity");
        let initial_revision = session_route_affinity_revision(&initial);

        let rebound = state
            .compare_and_mutate_session_route_affinity(
                "session-a",
                Some(initial_revision.as_str()),
                SessionRouteAffinityControlCommand::Rebind(target("ciii")),
                200,
            )
            .await
            .expect("rebind affinity");
        assert_eq!(rebound.status, SessionRouteAffinityControlStatus::Applied);
        let rebound_affinity = rebound.affinity.expect("rebound affinity");
        assert_eq!(rebound_affinity.provider_endpoint.provider_id, "ciii");
        assert_eq!(rebound_affinity.change_reason, "operator_rebind");

        let stale = state
            .compare_and_mutate_session_route_affinity(
                "session-a",
                Some(initial_revision.as_str()),
                SessionRouteAffinityControlCommand::Clear,
                201,
            )
            .await
            .expect("reject stale clear");
        assert_eq!(stale.status, SessionRouteAffinityControlStatus::Conflict);

        let rebound_revision = session_route_affinity_revision(&rebound_affinity);
        let cleared = state
            .compare_and_mutate_session_route_affinity(
                "session-a",
                Some(rebound_revision.as_str()),
                SessionRouteAffinityControlCommand::Clear,
                202,
            )
            .await
            .expect("clear affinity");
        assert_eq!(cleared.status, SessionRouteAffinityControlStatus::Applied);
        assert!(
            state
                .peek_session_route_affinity("session-a")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn affinity_control_rejects_an_active_session() {
        let state = ProxyState::new();
        let initial = state
            .record_session_route_affinity_success(None, "session-busy", target("input"), None, 100)
            .await
            .expect("record affinity");
        let revision = session_route_affinity_revision(&initial);
        let _request_id = state
            .begin_request(
                "codex",
                "POST",
                "/v1/responses",
                Some("  session-busy  ".to_string()),
                Some(SessionIdentitySource::Header),
                None,
                None,
                None,
                None,
                None,
                None,
                150,
            )
            .await;

        let commit = state
            .compare_and_mutate_session_route_affinity(
                "session-busy",
                Some(revision.as_str()),
                SessionRouteAffinityControlCommand::Clear,
                200,
            )
            .await
            .expect("reject busy mutation");
        assert_eq!(commit.status, SessionRouteAffinityControlStatus::Busy);
        assert_eq!(commit.affinity.as_ref(), Some(&initial));
        let active = state.list_active_requests().await;
        assert_eq!(active[0].session_id.as_deref(), Some("session-busy"));
        assert!(
            state
                .peek_session_route_affinity("session-busy")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn mutation_holding_session_guard_precedes_waiting_request_affinity_read() {
        let state = ProxyState::new();
        let initial = state
            .record_session_route_affinity_success(
                None,
                "session-mutation-first",
                target("input"),
                None,
                100,
            )
            .await
            .expect("record initial affinity");
        let initial_revision = session_route_affinity_revision(&initial);
        let mutation_guard = tokio::time::timeout(
            Duration::from_secs(1),
            state.lock_session_route_control("session-mutation-first"),
        )
        .await
        .expect("mutation should acquire the session guard");
        let request_waiting = state
            .signal_next_session_route_control_lock_wait_for_test()
            .await;
        let request_state = Arc::clone(&state);
        let request = tokio::spawn(async move {
            let request_id = request_state
                .begin_request(
                    "codex",
                    "POST",
                    "/v1/responses",
                    Some("session-mutation-first".to_string()),
                    Some(SessionIdentitySource::Header),
                    None,
                    None,
                    None,
                    Some("gpt-5".to_string()),
                    None,
                    None,
                    250,
                )
                .await;
            let affinity = request_state
                .get_session_route_affinity("session-mutation-first")
                .await;
            (request_id, affinity)
        });
        tokio::time::timeout(Duration::from_secs(1), request_waiting)
            .await
            .expect("request should reach the held session guard")
            .expect("request wait signal should remain connected");

        let commit = tokio::time::timeout(
            Duration::from_secs(1),
            state.compare_and_mutate_session_route_affinity_with_control(
                &mutation_guard,
                Some(initial_revision.as_str()),
                SessionRouteAffinityControlCommand::Rebind(target("ciii")),
                200,
            ),
        )
        .await
        .expect("mutation should not deadlock")
        .expect("rebind affinity");
        assert_eq!(commit.status, SessionRouteAffinityControlStatus::Applied);
        drop(mutation_guard);

        let (_, observed) = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("waiting request should resume")
            .expect("waiting request task should join");
        assert_eq!(
            observed
                .expect("request should observe affinity")
                .provider_endpoint
                .provider_id,
            "ciii"
        );
    }

    #[tokio::test]
    async fn admitted_request_releases_guard_before_waiting_mutation_reports_busy() {
        let state = ProxyState::new();
        let initial = state
            .record_session_route_affinity_success(
                None,
                "session-request-first",
                target("input"),
                None,
                100,
            )
            .await
            .expect("record initial affinity");
        let revision = session_route_affinity_revision(&initial);
        let request_guard = tokio::time::timeout(
            Duration::from_secs(1),
            state.lock_session_route_control("session-request-first"),
        )
        .await
        .expect("request should acquire the session guard");
        tokio::time::timeout(
            Duration::from_secs(1),
            begin_request_with_guard(state.as_ref(), &request_guard, 150),
        )
        .await
        .expect("request admission should not deadlock")
        .expect("begin active request");

        let mutation_waiting = state
            .signal_next_session_route_control_lock_wait_for_test()
            .await;
        let mutation_state = Arc::clone(&state);
        let mutation = tokio::spawn(async move {
            mutation_state
                .compare_and_mutate_session_route_affinity(
                    "session-request-first",
                    Some(revision.as_str()),
                    SessionRouteAffinityControlCommand::Clear,
                    200,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), mutation_waiting)
            .await
            .expect("mutation should reach the held session guard")
            .expect("mutation wait signal should remain connected");
        assert!(!mutation.is_finished(), "mutation must wait for admission");
        drop(request_guard);

        let commit = tokio::time::timeout(Duration::from_secs(1), mutation)
            .await
            .expect("mutation should resume after admission")
            .expect("mutation task should join")
            .expect("mutation should return a control result");
        assert_eq!(commit.status, SessionRouteAffinityControlStatus::Busy);
        assert_eq!(commit.affinity.as_ref(), Some(&initial));
    }

    #[tokio::test]
    async fn session_route_control_guards_do_not_block_different_sessions() {
        let state = ProxyState::new();
        let first = tokio::time::timeout(
            Duration::from_secs(1),
            state.lock_session_route_control("session-one"),
        )
        .await
        .expect("first session guard should be available");
        let second = tokio::time::timeout(
            Duration::from_secs(1),
            state.lock_session_route_control("session-two"),
        )
        .await
        .expect("different session must not wait for the first guard");
        assert_eq!(first.session_id(), "session-one");
        assert_eq!(second.session_id(), "session-two");
    }

    #[tokio::test]
    async fn session_route_control_guard_cannot_cross_proxy_state_boundaries() {
        let first = ProxyState::new();
        let second = ProxyState::new();
        let guard = tokio::time::timeout(
            Duration::from_secs(1),
            first.lock_session_route_control("session-cross-state"),
        )
        .await
        .expect("first state should acquire its guard");

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            begin_request_with_guard(second.as_ref(), &guard, 100),
        )
        .await
        .expect("cross-state validation should not block")
        .expect_err("a guard from another ProxyState must be rejected");
        assert!(
            matches!(
                error,
                RuntimeStoreError::InvariantViolation {
                    entity: "session route control guard",
                    ..
                }
            ),
            "unexpected error: {error}"
        );
        assert!(second.list_active_requests().await.is_empty());
    }

    #[tokio::test]
    async fn blank_session_id_is_recorded_as_no_session_identity() {
        let state = ProxyState::new();
        let _request_id = state
            .begin_request(
                "codex",
                "POST",
                "/v1/responses",
                Some("   ".to_string()),
                Some(SessionIdentitySource::Header),
                None,
                None,
                None,
                None,
                None,
                None,
                150,
            )
            .await;

        let active = state.list_active_requests().await;
        assert_eq!(active[0].session_id, None);
        assert_eq!(active[0].session_identity_source, None);
    }
}
