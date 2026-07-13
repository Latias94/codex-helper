use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::routing::post;
use tokio::sync::Notify;

use super::harness::{spawn_proxy_service, spawn_test_upstream, upstream_config};
use super::*;
use crate::pricing::{CostBreakdown, CostConfidence};
use crate::routing_ir::RoutePlanUpstreamRuntimeState;
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::{
    AttemptId, AttemptOutcome, AttemptPendingEvidence, AttemptRecord, AttemptRouteEvidence,
    AttemptTerminal, CommittedRequestProjection, CommittedRequestQuery, EconomicsState,
    LogicalRequestOutcome, LogicalRequestRecord, LogicalRequestTerminal,
    LogicalRequestTerminalPayload, NewAttempt, NewLogicalRequest, OperatorLedgerRevision,
    ProviderAutomaticEligibility, ProviderEffectiveEligibility, ProviderManualEligibility,
    ProviderObservation, ProviderObservationAuthority, ProviderObservationDisposition,
    ProviderObservationHistoryEntry, ProviderObservationScope, ProviderPolicyEffect,
    ProviderPolicySnapshot, RequestAccountingScope, RuntimeStore, RuntimeStoreError,
    TerminalOrigin,
};
use crate::state::{FinishedRequest, ProxyState};
use crate::usage::{CacheAccountingConvention, UsageMetrics};

const CRASH_CHILD_HOME_ENV: &str = "CODEX_HELPER_TEST_CRASH_CHILD_HOME";
const CRASH_CHILD_UPSTREAM_URL_ENV: &str = "CODEX_HELPER_TEST_CRASH_CHILD_UPSTREAM_URL";
const CRASH_CHILD_READY_PATH_ENV: &str = "CODEX_HELPER_TEST_CRASH_CHILD_READY_PATH";
const CRASH_CHILD_TEST: &str = "proxy::tests::crash_recovery::crash_gate_during_attempt_child";
const DIRECT_CRASH_CHILD_PHASE_ENV: &str = "CODEX_HELPER_TEST_DIRECT_CRASH_CHILD_PHASE";
const DIRECT_CRASH_CHILD_MARKER_ENV: &str = "CODEX_HELPER_TEST_DIRECT_CRASH_CHILD_MARKER";
const DIRECT_CRASH_CHILD_TEST: &str =
    "proxy::tests::crash_recovery::crash_gate_durable_boundary_child";
const DIRECT_CRASH_SESSION_ID: &str = "sid-direct-crash-boundary";
const POLICY_CRASH_CHILD_PHASE_ENV: &str = "CODEX_HELPER_TEST_POLICY_CRASH_CHILD_PHASE";
const POLICY_CRASH_CHILD_MARKER_ENV: &str = "CODEX_HELPER_TEST_POLICY_CRASH_CHILD_MARKER";
const POLICY_CRASH_CHILD_TEST: &str =
    "proxy::tests::crash_recovery::crash_gate_policy_boundary_child";
const POLICY_TEST_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const CRASH_CHILD_COORDINATION_TIMEOUT: Duration = Duration::from_secs(30);
const CRASH_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCrashPhase {
    Pending,
    AttemptResult,
    LogicalTerminal,
}

impl DirectCrashPhase {
    const ALL: [Self; 3] = [Self::Pending, Self::AttemptResult, Self::LogicalTerminal];

    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending_committed",
            Self::AttemptResult => "attempt_result_committed",
            Self::LogicalTerminal => "logical_terminal_committed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending_committed" => Some(Self::Pending),
            "attempt_result_committed" => Some(Self::AttemptResult),
            "logical_terminal_committed" => Some(Self::LogicalTerminal),
            _ => None,
        }
    }

    fn first_recovery_counts(self) -> (u64, u64) {
        match self {
            Self::Pending => (1, 1),
            Self::AttemptResult => (1, 0),
            Self::LogicalTerminal => (0, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyCrashPhase {
    Rollback,
    Committed,
}

impl PolicyCrashPhase {
    const ALL: [Self; 2] = [Self::Rollback, Self::Committed];

    fn as_str(self) -> &'static str {
        match self {
            Self::Rollback => "policy_rollback",
            Self::Committed => "policy_committed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "policy_rollback" => Some(Self::Rollback),
            "policy_committed" => Some(Self::Committed),
            _ => None,
        }
    }
}

struct CrashTestHome(PathBuf);

impl CrashTestHome {
    fn new() -> Self {
        Self(make_temp_test_dir())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CrashTestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for_proxy_address(path: &Path, child: &mut Child) -> SocketAddr {
    tokio::time::timeout(CRASH_CHILD_COORDINATION_TIMEOUT, async {
        loop {
            if let Ok(value) = std::fs::read_to_string(path)
                && let Ok(address) = value.trim().parse()
            {
                break address;
            }
            if let Some(status) = child.try_wait().expect("inspect crash child status") {
                panic!("crash child exited before publishing its proxy address: {status}");
            }
            tokio::time::sleep(CRASH_CHILD_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("crash child must publish its proxy address")
}

async fn wait_for_upstream_attempt(
    upstream_hits: &AtomicUsize,
    upstream_received: &Notify,
    child: &mut Child,
) {
    tokio::time::timeout(CRASH_CHILD_COORDINATION_TIMEOUT, async {
        loop {
            if upstream_hits.load(Ordering::SeqCst) > 0 {
                break;
            }
            if let Some(status) = child.try_wait().expect("inspect crash child status") {
                panic!("crash child exited before reaching the upstream attempt: {status}");
            }
            tokio::select! {
                _ = upstream_received.notified() => {}
                _ = tokio::time::sleep(CRASH_CHILD_POLL_INTERVAL) => {}
            }
        }
    })
    .await
    .expect("upstream must receive the request before the child is killed");
}

async fn wait_for_crash_marker(path: &Path, expected: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(path).is_ok_and(|value| value.trim() == expected) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("crash child must publish marker {expected:?}"));
}

fn direct_attempt_evidence() -> AttemptPendingEvidence {
    AttemptPendingEvidence::new(
        1,
        "test-runtime",
        AttemptRouteEvidence {
            provider_endpoint_key: Some("codex/test/default".to_string()),
            provider_id: Some("test".to_string()),
            endpoint_id: Some("default".to_string()),
            route_path: vec!["test".to_string(), "default".to_string()],
            upstream_base_url: Some("https://example.test/v1".to_string()),
            mapped_model: Some("gpt-5".to_string()),
        },
    )
}

fn policy_crash_endpoint() -> ProviderEndpointKey {
    ProviderEndpointKey::new("codex", "policy-crash", "default")
}

fn policy_crash_scope() -> ProviderObservationScope {
    let endpoint = policy_crash_endpoint();
    ProviderObservationScope::new(
        endpoint.clone(),
        "https://provider.test",
        endpoint.stable_key(),
        "test:crash-matrix",
        "https://provider.test/usage",
        POLICY_TEST_DIGEST,
        POLICY_TEST_DIGEST,
    )
    .expect("construct policy crash observation scope")
}

fn policy_crash_observation() -> ProviderObservation {
    ProviderObservation {
        observed_at_unix_ms: 101,
        completed_at_unix_ms: 102,
        authority: ProviderObservationAuthority::Authoritative,
        evidence: serde_json::json!({ "exhausted": true }),
        effect: ProviderPolicyEffect::Block {
            action_kind: "balance_exhausted".to_string(),
            code: Some("balance_exhausted".to_string()),
            reason: "policy crash fixture exhaustion".to_string(),
            expires_at_unix_ms: None,
        },
    }
}

fn direct_terminal_payload(
    winning_attempt_id: AttemptId,
    terminal_at_unix_ms: u64,
) -> LogicalRequestTerminalPayload {
    let usage = UsageMetrics {
        input_tokens: 1_000,
        output_tokens: 100,
        total_tokens: 1_100,
        ..UsageMetrics::default()
    };
    let billable_usage = usage.canonical_usage_buckets(CacheAccountingConvention::SEPARATE);
    let cost: CostBreakdown = serde_json::from_value(serde_json::json!({
        "input_cost_usd": "0.001",
        "output_cost_usd": "0.0002",
        "total_cost_usd": "0.0012",
        "confidence": "exact",
        "pricing_source": "test:crash-matrix"
    }))
    .expect("construct direct crash cost fixture");

    LogicalRequestTerminalPayload {
        finished_request: FinishedRequest {
            id: 41,
            trace_id: Some("trace-direct-crash-boundary".to_string()),
            session_id: Some(DIRECT_CRASH_SESSION_ID.to_string()),
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: None,
            provider_id: Some("test".to_string()),
            route_decision: None,
            usage: Some(usage),
            cost,
            accounting: Default::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: Default::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 30,
            ttfb_ms: Some(10),
            streaming: false,
            ended_at_ms: terminal_at_unix_ms,
        },
        winning_attempt_id: Some(winning_attempt_id),
        runtime_revision: 1,
        runtime_digest: "test-runtime".to_string(),
        policy_revision: None,
        provider_epoch: None,
        provider_price_key: None,
        requested_model: Some("gpt-5".to_string()),
        mapped_model: Some("gpt-5".to_string()),
        reported_model: None,
        pricing_model: Some("gpt-5".to_string()),
        requested_service_tier: None,
        effective_service_tier: None,
        actual_service_tier: None,
        pricing_service_tier: None,
        cache_accounting_convention: CacheAccountingConvention::SEPARATE,
        billable_usage: Some(billable_usage),
        accounting_scope: RequestAccountingScope::Economic,
    }
}

fn assert_direct_finished_request(request: &FinishedRequest) {
    assert_eq!(request.id, 41);
    assert_eq!(request.session_id.as_deref(), Some(DIRECT_CRASH_SESSION_ID));
    assert_eq!(request.model.as_deref(), Some("gpt-5"));
    assert_eq!(request.status_code, 200);
    let usage = request.usage.as_ref().expect("fixed usage is present");
    assert_eq!(usage.input_tokens, 1_000);
    assert_eq!(usage.output_tokens, 100);
    assert_eq!(usage.total_tokens, 1_100);
    assert_eq!(request.cost.input_cost_usd.as_deref(), Some("0.001"));
    assert_eq!(request.cost.output_cost_usd.as_deref(), Some("0.0002"));
    assert_eq!(request.cost.total_cost_usd.as_deref(), Some("0.0012"));
    assert_eq!(request.cost.confidence, CostConfidence::Exact);
    assert_eq!(
        request.cost.pricing_source.as_deref(),
        Some("test:crash-matrix")
    );
}

#[derive(Debug, PartialEq)]
struct DirectPhaseSnapshot {
    logical_request: LogicalRequestRecord,
    attempt: AttemptRecord,
    committed: Vec<CommittedRequestProjection>,
    ledger_revision: OperatorLedgerRevision,
    policy: ProviderPolicySnapshot,
}

#[derive(Debug, PartialEq)]
struct PolicyPhaseSnapshot {
    policy: ProviderPolicySnapshot,
    history: Vec<ProviderObservationHistoryEntry>,
    committed: Vec<CommittedRequestProjection>,
    ledger_revision: OperatorLedgerRevision,
}

async fn direct_phase_snapshot(
    store: Arc<RuntimeStore>,
    phase: DirectCrashPhase,
    expected_recovery_ordinal: u64,
    expected_interrupted_logical_count: u64,
    expected_interrupted_attempt_count: u64,
) -> DirectPhaseSnapshot {
    let report = store.startup_recovery_report();
    assert_eq!(
        report.recovery_ordinal, expected_recovery_ordinal,
        "phase={phase:?}"
    );
    assert_eq!(
        report.interrupted_logical_count, expected_interrupted_logical_count,
        "phase={phase:?}"
    );
    assert_eq!(
        report.interrupted_attempt_count, expected_interrupted_attempt_count,
        "phase={phase:?}"
    );

    let logical_requests = store
        .read_recent_logical_requests(10)
        .expect("read direct crash logical requests");
    assert_eq!(logical_requests.len(), 1, "phase={phase:?}");
    let logical_request = logical_requests[0].clone();
    let logical_terminal = logical_request
        .terminal
        .as_ref()
        .expect("direct crash logical request is terminal");

    let logical_handle = store.logical_request_handle(logical_request.request.id);
    let attempts = store
        .read_attempts_for_logical_request(logical_handle)
        .expect("read direct crash attempts");
    assert_eq!(attempts.len(), 1, "phase={phase:?}");
    let attempt = attempts[0].clone();
    assert_eq!(attempt.attempt_ordinal, 1, "phase={phase:?}");
    assert_eq!(
        attempt
            .attempt
            .evidence
            .route
            .provider_endpoint_key
            .as_deref(),
        Some("codex/test/default"),
        "phase={phase:?}"
    );
    let attempt_terminal = attempt
        .terminal
        .as_ref()
        .expect("direct crash attempt is terminal");

    match phase {
        DirectCrashPhase::Pending => {
            assert_eq!(
                logical_terminal.terminal.outcome,
                LogicalRequestOutcome::Interrupted
            );
            assert_eq!(
                logical_terminal.terminal.economics_state,
                EconomicsState::Unknown
            );
            assert_eq!(logical_terminal.origin, TerminalOrigin::StartupRecovery);
            assert!(logical_terminal.terminal.payload.is_none());
            assert_eq!(
                attempt_terminal.terminal.outcome,
                AttemptOutcome::Interrupted
            );
            assert_eq!(
                attempt_terminal.terminal.economics_state,
                EconomicsState::Unknown
            );
            assert_eq!(attempt_terminal.origin, TerminalOrigin::StartupRecovery);
            if expected_interrupted_logical_count == 1 {
                assert_eq!(logical_terminal.recovery_run_id, Some(report.run_id));
                assert_eq!(attempt_terminal.recovery_run_id, Some(report.run_id));
            }
        }
        DirectCrashPhase::AttemptResult => {
            assert_eq!(
                logical_terminal.terminal.outcome,
                LogicalRequestOutcome::Interrupted
            );
            assert_eq!(
                logical_terminal.terminal.economics_state,
                EconomicsState::Unknown
            );
            assert_eq!(logical_terminal.origin, TerminalOrigin::StartupRecovery);
            assert!(logical_terminal.terminal.payload.is_none());
            assert_eq!(attempt_terminal.terminal.outcome, AttemptOutcome::Succeeded);
            assert_eq!(
                attempt_terminal.terminal.economics_state,
                EconomicsState::Known
            );
            assert_eq!(attempt_terminal.origin, TerminalOrigin::Runtime);
            assert_eq!(attempt_terminal.recovery_run_id, None);
            if expected_interrupted_logical_count == 1 {
                assert_eq!(logical_terminal.recovery_run_id, Some(report.run_id));
            }
        }
        DirectCrashPhase::LogicalTerminal => {
            assert_eq!(
                logical_terminal.terminal.outcome,
                LogicalRequestOutcome::Succeeded
            );
            assert_eq!(
                logical_terminal.terminal.economics_state,
                EconomicsState::Known
            );
            assert_eq!(logical_terminal.origin, TerminalOrigin::Runtime);
            assert_eq!(logical_terminal.recovery_run_id, None);
            assert_eq!(attempt_terminal.terminal.outcome, AttemptOutcome::Succeeded);
            assert_eq!(
                attempt_terminal.terminal.economics_state,
                EconomicsState::Known
            );
            assert_eq!(attempt_terminal.origin, TerminalOrigin::Runtime);
            assert_eq!(attempt_terminal.recovery_run_id, None);
            let payload = logical_terminal
                .terminal
                .payload
                .as_ref()
                .expect("committed terminal payload is present");
            assert_eq!(payload.winning_attempt_id, Some(attempt.attempt.id));
            assert_direct_finished_request(&payload.finished_request);
        }
    }

    let committed = store
        .query_committed_requests(&CommittedRequestQuery::default())
        .expect("query direct crash projections")
        .items;
    if phase == DirectCrashPhase::LogicalTerminal {
        assert_eq!(committed.len(), 1, "phase={phase:?}");
        assert_eq!(committed[0].logical_request_id, logical_request.request.id);
        assert_eq!(committed[0].outcome, LogicalRequestOutcome::Succeeded);
        assert_direct_finished_request(&committed[0].payload.finished_request);
    } else {
        assert!(committed.is_empty(), "phase={phase:?}");
    }

    let ledger_revision = store
        .operator_ledger_revision()
        .expect("read direct crash operator ledger revision");
    let policy = store
        .provider_policy_snapshot()
        .expect("read direct crash provider policy");
    assert_eq!(policy.policy_revision, 0, "phase={phase:?}");
    assert!(policy.projections.is_empty(), "phase={phase:?}");

    let state = ProxyState::new_with_runtime_store(Arc::clone(&store))
        .expect("rehydrate direct crash state");
    assert!(state.list_active_requests().await.is_empty());
    let recent = state.list_recent_finished(10).await;
    let sessions = state.list_session_stats().await;
    let usage = state.get_usage_rollup_view("codex", 12, 0).await;
    if phase == DirectCrashPhase::LogicalTerminal {
        assert_eq!(recent.len(), 1, "phase={phase:?}");
        assert_direct_finished_request(&recent[0]);
        assert_eq!(usage.loaded.requests_total, 1, "phase={phase:?}");
        assert_eq!(usage.loaded.usage.input_tokens, 1_000, "phase={phase:?}");
        assert_eq!(usage.loaded.usage.output_tokens, 100, "phase={phase:?}");
        assert_eq!(usage.loaded.usage.total_tokens, 1_100, "phase={phase:?}");
        assert_eq!(
            usage.loaded.cost.total_cost_usd.as_deref(),
            Some("0.0012"),
            "phase={phase:?}"
        );
        assert_eq!(usage.loaded.cost.priced_requests, 1, "phase={phase:?}");
        assert_eq!(usage.loaded.cost.unpriced_requests, 0, "phase={phase:?}");
        assert_eq!(sessions.len(), 1, "phase={phase:?}");
        assert_eq!(sessions[DIRECT_CRASH_SESSION_ID].turns_total, 1);
    } else {
        assert!(recent.is_empty(), "phase={phase:?}");
        assert_eq!(usage.loaded.requests_total, 0, "phase={phase:?}");
        assert_eq!(usage.loaded.usage.total_tokens, 0, "phase={phase:?}");
        assert!(usage.loaded.cost.is_empty(), "phase={phase:?}");
        assert!(sessions.is_empty(), "phase={phase:?}");
    }
    let endpoint = ProviderEndpointKey::new("codex", "test", "default");
    assert_eq!(
        state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await
            .provider_endpoint(&endpoint),
        RoutePlanUpstreamRuntimeState::default(),
        "phase={phase:?}"
    );
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty(),
        "phase={phase:?}"
    );
    drop(state);

    DirectPhaseSnapshot {
        logical_request,
        attempt,
        committed,
        ledger_revision,
        policy,
    }
}

async fn policy_phase_snapshot(
    store: Arc<RuntimeStore>,
    phase: PolicyCrashPhase,
    expected_recovery_ordinal: u64,
) -> PolicyPhaseSnapshot {
    let report = store.startup_recovery_report();
    assert_eq!(
        report.recovery_ordinal, expected_recovery_ordinal,
        "phase={phase:?}"
    );
    assert_eq!(report.interrupted_logical_count, 0, "phase={phase:?}");
    assert_eq!(report.interrupted_attempt_count, 0, "phase={phase:?}");
    assert!(
        store
            .read_recent_logical_requests(10)
            .expect("read policy crash logical requests")
            .is_empty(),
        "phase={phase:?}"
    );

    let committed = store
        .query_committed_requests(&CommittedRequestQuery::default())
        .expect("query policy crash request projections")
        .items;
    assert!(committed.is_empty(), "phase={phase:?}");
    let ledger_revision = store
        .operator_ledger_revision()
        .expect("read policy crash operator ledger revision");

    let endpoint = policy_crash_endpoint();
    let history = store
        .read_provider_observation_history(&endpoint, 10)
        .expect("read policy crash observation history");
    let policy = store
        .provider_policy_snapshot()
        .expect("read policy crash snapshot");
    assert_eq!(policy.projections.len(), 1, "phase={phase:?}");
    let projection = &policy.projections[0];
    assert_eq!(projection.provider_endpoint, endpoint, "phase={phase:?}");
    assert_eq!(
        projection.manual,
        ProviderManualEligibility::Enabled,
        "phase={phase:?}"
    );
    assert!(projection.manual_reason.is_none(), "phase={phase:?}");

    match phase {
        PolicyCrashPhase::Rollback => {
            assert_eq!(policy.policy_revision, 1);
            assert_eq!(projection.policy_revision, 1);
            assert_eq!(projection.updated_at_unix_ms, 100);
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Eligible);
            assert_eq!(projection.effective, ProviderEffectiveEligibility::Eligible);
            assert!(projection.active_action.is_none());
            assert!(history.is_empty());
        }
        PolicyCrashPhase::Committed => {
            assert_eq!(policy.policy_revision, 2);
            assert_eq!(projection.policy_revision, 2);
            assert_eq!(projection.updated_at_unix_ms, 102);
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Blocked);
            assert_eq!(
                projection.effective,
                ProviderEffectiveEligibility::Ineligible
            );
            assert_eq!(history.len(), 1);
            let observation = &history[0];
            assert_eq!(observation.provider_endpoint, endpoint);
            assert_eq!(observation.generation, 1);
            assert_eq!(observation.observation, policy_crash_observation());
            assert_eq!(
                observation.disposition,
                ProviderObservationDisposition::Accepted
            );
            assert_eq!(observation.policy_revision, 2);

            let action = projection
                .active_action
                .as_ref()
                .expect("committed policy crash action is active");
            assert_eq!(action.provider_endpoint, endpoint);
            assert_eq!(action.incarnation_id, observation.incarnation_id);
            assert_eq!(action.generation, 1);
            assert_eq!(action.action_kind, "balance_exhausted");
            assert_eq!(action.code.as_deref(), Some("balance_exhausted"));
            assert_eq!(action.reason, "policy crash fixture exhaustion");
            assert_eq!(action.opened_at_unix_ms, 102);
            assert_eq!(action.expires_at_unix_ms, None);
            assert_eq!(action.closed_at_unix_ms, None);
            assert_eq!(action.close_reason, None);
        }
    }

    let state = ProxyState::new_with_runtime_store(Arc::clone(&store))
        .expect("rehydrate policy crash state");
    assert_eq!(*state.capture_provider_policy_snapshot().await, policy);
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(state.list_session_stats().await.is_empty());
    drop(state);

    PolicyPhaseSnapshot {
        policy,
        history,
        committed,
        ledger_revision,
    }
}

#[test]
fn crash_gate_during_attempt_child() {
    let Some(home) = std::env::var_os(CRASH_CHILD_HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let upstream_url = std::env::var(CRASH_CHILD_UPSTREAM_URL_ENV)
        .expect("crash child upstream URL is configured");
    let ready_path = PathBuf::from(
        std::env::var_os(CRASH_CHILD_READY_PATH_ENV).expect("crash child ready path is configured"),
    );

    let runtime = tokio::runtime::Runtime::new().expect("create crash child runtime");
    runtime.block_on(async move {
        let config =
            make_helper_config(vec![upstream_config(upstream_url)], RetryConfig::default());
        let runtime_store = Arc::new(
            RuntimeStore::open_in_home(&home).expect("open persistent crash child runtime store"),
        );
        let proxy = ProxyService::new_with_runtime_store(
            Client::new(),
            Arc::new(config),
            "codex",
            runtime_store,
        )
        .expect("create persistent crash child proxy");
        let server = spawn_proxy_service(proxy);
        std::fs::write(&ready_path, server.addr.to_string())
            .expect("publish crash child proxy address");
        std::future::pending::<()>().await;
    });
}

#[test]
fn crash_gate_durable_boundary_child() {
    let Some(home) = std::env::var_os(CRASH_CHILD_HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let phase_value = std::env::var(DIRECT_CRASH_CHILD_PHASE_ENV)
        .expect("direct crash child phase is configured");
    let phase = DirectCrashPhase::parse(&phase_value)
        .unwrap_or_else(|| panic!("unknown direct crash child phase {phase_value:?}"));
    let marker_path = PathBuf::from(
        std::env::var_os(DIRECT_CRASH_CHILD_MARKER_ENV)
            .expect("direct crash child marker path is configured"),
    );

    let store = RuntimeStore::open_in_home(&home).expect("open direct crash child runtime store");
    let initial_report = store.startup_recovery_report();
    assert_eq!(initial_report.recovery_ordinal, 1);
    assert_eq!(initial_report.interrupted_logical_count, 0);
    assert_eq!(initial_report.interrupted_attempt_count, 0);

    let begun_at_unix_ms = crate::logging::now_ms();
    let attempt_at_unix_ms = begun_at_unix_ms.saturating_add(10);
    let attempt_terminal_at_unix_ms = begun_at_unix_ms.saturating_add(20);
    let logical_terminal_at_unix_ms = begun_at_unix_ms.saturating_add(30);
    let request = NewLogicalRequest {
        id: crate::runtime_store::LogicalRequestId::new(),
        begun_at_unix_ms,
    };
    let logical_handle = store
        .transaction(|transaction| transaction.begin_logical_request(request))
        .expect("commit direct crash logical request begin")
        .handle;
    let attempt = NewAttempt {
        id: AttemptId::new(),
        logical_request_id: request.id,
        begun_at_unix_ms: attempt_at_unix_ms,
        evidence: direct_attempt_evidence(),
    };
    let attempt_handle = store
        .transaction(|transaction| transaction.begin_attempt(logical_handle, attempt))
        .expect("commit direct crash attempt begin")
        .handle;

    if phase != DirectCrashPhase::Pending {
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    attempt_handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: attempt_terminal_at_unix_ms,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("commit direct crash attempt result");
    }
    if phase == DirectCrashPhase::LogicalTerminal {
        store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    logical_handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: logical_terminal_at_unix_ms,
                        economics_state: EconomicsState::Known,
                        payload: Some(direct_terminal_payload(
                            attempt_handle.id(),
                            logical_terminal_at_unix_ms,
                        )),
                    },
                )
            })
            .expect("commit direct crash logical terminal");
    }

    std::fs::write(&marker_path, phase.as_str())
        .expect("publish direct crash durable boundary marker");
    loop {
        std::hint::black_box(&store);
        std::thread::park();
    }
}

#[test]
fn crash_gate_policy_boundary_child() {
    let Some(home) = std::env::var_os(CRASH_CHILD_HOME_ENV).map(PathBuf::from) else {
        return;
    };
    let phase_value = std::env::var(POLICY_CRASH_CHILD_PHASE_ENV)
        .expect("policy crash child phase is configured");
    let phase = PolicyCrashPhase::parse(&phase_value)
        .unwrap_or_else(|| panic!("unknown policy crash child phase {phase_value:?}"));
    let marker_path = PathBuf::from(
        std::env::var_os(POLICY_CRASH_CHILD_MARKER_ENV)
            .expect("policy crash child marker path is configured"),
    );

    let store = RuntimeStore::open_in_home(&home).expect("open policy crash child runtime store");
    let initial_report = store.startup_recovery_report();
    assert_eq!(initial_report.recovery_ordinal, 1);
    assert_eq!(initial_report.interrupted_logical_count, 0);
    assert_eq!(initial_report.interrupted_attempt_count, 0);

    let endpoint = policy_crash_endpoint();
    let reservation = store
        .reserve_provider_observation(policy_crash_scope(), 100)
        .expect("commit policy crash reservation");
    assert_eq!(reservation.policy_revision, 1);
    assert_eq!(reservation.projection.provider_endpoint, endpoint);
    assert_eq!(
        reservation.projection.automatic,
        ProviderAutomaticEligibility::Eligible
    );
    assert!(reservation.projection.active_action.is_none());
    let baseline = store
        .provider_policy_snapshot()
        .expect("read policy crash reservation baseline");

    match phase {
        PolicyCrashPhase::Rollback => {
            store.fail_next_policy_commit_for_test();
            let error = store
                .commit_provider_observation(reservation.ticket, policy_crash_observation())
                .expect_err("inject policy crash transaction rollback");
            assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
            assert_eq!(
                store
                    .provider_policy_snapshot()
                    .expect("read policy snapshot after rollback"),
                baseline
            );
            assert!(
                store
                    .read_provider_observation_history(&endpoint, 10)
                    .expect("read policy history after rollback")
                    .is_empty()
            );
        }
        PolicyCrashPhase::Committed => {
            let committed = store
                .commit_provider_observation(reservation.ticket, policy_crash_observation())
                .expect("commit policy crash observation");
            assert_eq!(
                committed.disposition,
                ProviderObservationDisposition::Accepted
            );
            assert_eq!(committed.policy_revision, 2);
            assert_eq!(
                committed.projection.automatic,
                ProviderAutomaticEligibility::Blocked
            );
            assert!(committed.projection.active_action.is_some());
            assert_eq!(
                store
                    .read_provider_observation_history(&endpoint, 10)
                    .expect("read committed policy history")
                    .len(),
                1
            );
        }
    }

    std::fs::write(&marker_path, phase.as_str())
        .expect("publish policy crash durable boundary marker");
    loop {
        std::hint::black_box(&store);
        std::thread::park();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_gate_durable_boundaries_recover_exactly_once() {
    for phase in DirectCrashPhase::ALL {
        let home = CrashTestHome::new();
        let marker_path = home.path().join(format!("{}-committed", phase.as_str()));
        let mut child = KillOnDrop(
            Command::new(std::env::current_exe().expect("current core test executable"))
                .arg(DIRECT_CRASH_CHILD_TEST)
                .arg("--exact")
                .env(CRASH_CHILD_HOME_ENV, home.path())
                .env(DIRECT_CRASH_CHILD_PHASE_ENV, phase.as_str())
                .env(DIRECT_CRASH_CHILD_MARKER_ENV, &marker_path)
                .env("CODEX_HELPER_HOME", home.path())
                .spawn()
                .unwrap_or_else(|error| {
                    panic!("spawn direct crash child for phase {phase:?}: {error}")
                }),
        );
        wait_for_crash_marker(&marker_path, phase.as_str()).await;
        child
            .0
            .kill()
            .unwrap_or_else(|error| panic!("kill direct crash child for phase {phase:?}: {error}"));
        let status = child.0.wait().unwrap_or_else(|error| {
            panic!("wait for direct crash child for phase {phase:?}: {error}")
        });
        assert!(
            !status.success(),
            "direct crash child must not exit cleanly; phase={phase:?}"
        );

        let (interrupted_logical_count, interrupted_attempt_count) = phase.first_recovery_counts();
        let first = direct_phase_snapshot(
            Arc::new(
                RuntimeStore::open_in_home(home.path()).unwrap_or_else(|error| {
                    panic!("first reopen after direct crash for phase {phase:?}: {error}")
                }),
            ),
            phase,
            2,
            interrupted_logical_count,
            interrupted_attempt_count,
        )
        .await;
        let second = direct_phase_snapshot(
            Arc::new(
                RuntimeStore::open_in_home(home.path()).unwrap_or_else(|error| {
                    panic!("second reopen after direct crash for phase {phase:?}: {error}")
                }),
            ),
            phase,
            3,
            0,
            0,
        )
        .await;
        assert_eq!(
            second, first,
            "second reopen must preserve exact-once state; phase={phase:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_gate_policy_boundaries_preserve_atomic_projection() {
    for phase in PolicyCrashPhase::ALL {
        let home = CrashTestHome::new();
        let marker_path = home.path().join(format!("{}-ready", phase.as_str()));
        let mut child = KillOnDrop(
            Command::new(std::env::current_exe().expect("current core test executable"))
                .arg(POLICY_CRASH_CHILD_TEST)
                .arg("--exact")
                .env(CRASH_CHILD_HOME_ENV, home.path())
                .env(POLICY_CRASH_CHILD_PHASE_ENV, phase.as_str())
                .env(POLICY_CRASH_CHILD_MARKER_ENV, &marker_path)
                .env("CODEX_HELPER_HOME", home.path())
                .spawn()
                .unwrap_or_else(|error| {
                    panic!("spawn policy crash child for phase {phase:?}: {error}")
                }),
        );
        wait_for_crash_marker(&marker_path, phase.as_str()).await;
        child
            .0
            .kill()
            .unwrap_or_else(|error| panic!("kill policy crash child for phase {phase:?}: {error}"));
        let status = child.0.wait().unwrap_or_else(|error| {
            panic!("wait for policy crash child for phase {phase:?}: {error}")
        });
        assert!(
            !status.success(),
            "policy crash child must not exit cleanly; phase={phase:?}"
        );

        let first = policy_phase_snapshot(
            Arc::new(
                RuntimeStore::open_in_home(home.path()).unwrap_or_else(|error| {
                    panic!("first reopen after policy crash for phase {phase:?}: {error}")
                }),
            ),
            phase,
            2,
        )
        .await;
        let second = policy_phase_snapshot(
            Arc::new(
                RuntimeStore::open_in_home(home.path()).unwrap_or_else(|error| {
                    panic!("second reopen after policy crash for phase {phase:?}: {error}")
                }),
            ),
            phase,
            3,
        )
        .await;
        assert_eq!(
            second, first,
            "second reopen must preserve policy transaction exactly once; phase={phase:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_gate_during_attempt_recovers_without_success_projection() {
    let home = CrashTestHome::new();
    let ready_path = home.path().join("proxy-ready");
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream_received = Arc::new(Notify::new());
    let upstream = spawn_test_upstream(Router::new().route(
        "/v1/responses",
        post({
            let upstream_hits = Arc::clone(&upstream_hits);
            let upstream_received = Arc::clone(&upstream_received);
            move |body: Bytes| {
                let upstream_hits = Arc::clone(&upstream_hits);
                let upstream_received = Arc::clone(&upstream_received);
                async move {
                    let payload: serde_json::Value =
                        serde_json::from_slice(&body).expect("decode upstream request body");
                    assert_eq!(payload["input"].as_str(), Some("crash-gate"));
                    assert_eq!(
                        payload["prompt_cache_key"].as_str(),
                        Some("sid-during-attempt-crash")
                    );
                    upstream_hits.fetch_add(1, Ordering::SeqCst);
                    upstream_received.notify_one();
                    std::future::pending::<StatusCode>().await
                }
            }
        }),
    ));

    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().expect("current core test executable"))
            .arg(CRASH_CHILD_TEST)
            .arg("--exact")
            .env(CRASH_CHILD_HOME_ENV, home.path())
            .env(CRASH_CHILD_UPSTREAM_URL_ENV, upstream.base_url())
            .env(CRASH_CHILD_READY_PATH_ENV, &ready_path)
            .env("CODEX_HELPER_HOME", home.path())
            .spawn()
            .expect("spawn persistent proxy crash child"),
    );
    let proxy_addr = wait_for_proxy_address(&ready_path, &mut child.0).await;
    let request = tokio::spawn(async move {
        Client::new()
            .post(format!("http://{proxy_addr}/v1/responses"))
            .header("content-type", "application/json")
            .header("session_id", "sid-during-attempt-crash")
            .body(r#"{"input":"crash-gate"}"#)
            .send()
            .await
    });

    wait_for_upstream_attempt(&upstream_hits, &upstream_received, &mut child.0).await;
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    child
        .0
        .kill()
        .expect("kill proxy child during upstream attempt");
    let status = child.0.wait().expect("wait for killed proxy child");
    assert!(!status.success(), "proxy child must not exit cleanly");
    let request_result = tokio::time::timeout(CRASH_CHILD_COORDINATION_TIMEOUT, request)
        .await
        .expect("downstream request must observe the child exit")
        .expect("join downstream request task");
    assert!(request_result.is_err());
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    let recovered_store = Arc::new(
        RuntimeStore::open_in_home(home.path()).expect("recover during-attempt runtime store"),
    );
    let first_report = recovered_store.startup_recovery_report();
    assert_eq!(first_report.recovery_ordinal, 2);
    assert_eq!(first_report.interrupted_logical_count, 1);
    assert_eq!(first_report.interrupted_attempt_count, 1);
    let recovered_requests = recovered_store
        .read_recent_logical_requests(10)
        .expect("read recovered logical requests");
    assert_eq!(recovered_requests.len(), 1);
    let recovered_request = recovered_requests[0].clone();
    let logical_terminal = recovered_request
        .terminal
        .as_ref()
        .expect("recovered logical request is terminal");
    assert_eq!(
        logical_terminal.terminal.outcome,
        LogicalRequestOutcome::Interrupted
    );
    assert_eq!(
        logical_terminal.terminal.economics_state,
        EconomicsState::Unknown
    );
    assert!(logical_terminal.terminal.payload.is_none());
    assert_eq!(logical_terminal.origin, TerminalOrigin::StartupRecovery);
    assert_eq!(logical_terminal.recovery_run_id, Some(first_report.run_id));

    let request_handle = recovered_store.logical_request_handle(recovered_request.request.id);
    let recovered_attempts = recovered_store
        .read_attempts_for_logical_request(request_handle)
        .expect("read recovered upstream attempts");
    assert_eq!(recovered_attempts.len(), 1);
    let recovered_attempt = recovered_attempts[0].clone();
    assert_eq!(recovered_attempt.attempt_ordinal, 1);
    assert_eq!(
        recovered_attempt
            .attempt
            .evidence
            .route
            .provider_endpoint_key
            .as_deref(),
        Some("codex/test/default")
    );
    let attempt_terminal = recovered_attempt
        .terminal
        .as_ref()
        .expect("recovered upstream attempt is terminal");
    assert_eq!(
        attempt_terminal.terminal.outcome,
        crate::runtime_store::AttemptOutcome::Interrupted
    );
    assert_eq!(
        attempt_terminal.terminal.economics_state,
        EconomicsState::Unknown
    );
    assert_eq!(attempt_terminal.origin, TerminalOrigin::StartupRecovery);
    assert_eq!(attempt_terminal.recovery_run_id, Some(first_report.run_id));
    assert!(
        recovered_store
            .query_committed_requests(&CommittedRequestQuery::default())
            .expect("query recovered runtime projections")
            .items
            .is_empty()
    );

    let endpoint = ProviderEndpointKey::new("codex", "test", "default");
    assert!(
        recovered_store
            .read_provider_observation_history(&endpoint, 10)
            .expect("read provider observation history")
            .is_empty()
    );
    let policy = recovered_store
        .provider_policy_snapshot()
        .expect("read recovered provider policy");
    assert_eq!(policy.policy_revision, 0);
    assert!(policy.projections.is_empty());

    let recovered_state = ProxyState::new_with_runtime_store(Arc::clone(&recovered_store))
        .expect("rehydrate state after during-attempt crash");
    assert!(recovered_state.list_recent_finished(10).await.is_empty());
    assert!(recovered_state.list_session_stats().await.is_empty());
    let usage = recovered_state.get_usage_rollup_view("codex", 12, 0).await;
    assert_eq!(usage.loaded.requests_total, 0);
    assert_eq!(usage.loaded.usage.total_tokens, 0);
    assert!(usage.loaded.cost.is_empty());
    assert_eq!(
        recovered_state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await
            .provider_endpoint(&endpoint),
        RoutePlanUpstreamRuntimeState::default()
    );
    assert!(
        recovered_state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );
    drop(recovered_state);
    drop(recovered_store);

    let clean_restart = RuntimeStore::open_in_home(home.path())
        .expect("restart recovered during-attempt runtime store");
    let second_report = clean_restart.startup_recovery_report();
    assert_eq!(second_report.recovery_ordinal, 3);
    assert_eq!(second_report.interrupted_logical_count, 0);
    assert_eq!(second_report.interrupted_attempt_count, 0);
    assert_eq!(
        clean_restart
            .read_recent_logical_requests(10)
            .expect("read logical request after clean restart"),
        vec![recovered_request]
    );
    assert_eq!(
        clean_restart
            .read_attempts_for_logical_request(request_handle)
            .expect("read attempt after clean restart"),
        vec![recovered_attempt]
    );
    assert!(
        clean_restart
            .query_committed_requests(&CommittedRequestQuery::default())
            .expect("query runtime projections after clean restart")
            .items
            .is_empty()
    );
}
