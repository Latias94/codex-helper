use std::time::{Duration, Instant};

use crate::config::{RetryConfig, ServiceKind};
use crate::dashboard_core::{
    OperatorHealthSummary, OperatorProfileSummary, OperatorRetrySummary, OperatorRuntimeSummary,
    OperatorSummaryCounts, WindowStats, build_operator_health_summary,
    summarize_recent_retry_observations,
};
use crate::logging::{ControlTraceLogEntry, control_trace_path, read_recent_control_trace_entries};
use crate::proxy::local_proxy_base_url;
use anyhow::bail;
use reqwest::Client;

mod attached_discovery;
mod attached_refresh;
mod control_mutations;
mod running_refresh;
mod runtime_lifecycle;
mod runtime_operations;
mod types;

pub use self::attached_discovery::DiscoveredProxy;
use self::attached_discovery::{
    local_host_local_control_plane_capabilities, local_remote_admin_access_capabilities,
    local_shared_control_plane_capabilities,
};
use self::types::PortInUseModal;
pub use self::types::{
    AttachedStatus, ControlTraceDataSource, ControlTraceReadResult, GuiRuntimeSnapshot,
    PortInUseAction, ProxyController, ProxyMode, ProxyModeKind, RunningProxy,
};

fn admin_auth_token() -> Option<String> {
    std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn with_admin_auth(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(token) = admin_auth_token() {
        builder.header(crate::proxy::ADMIN_TOKEN_HEADER, token)
    } else {
        builder
    }
}

async fn send_admin_request(builder: reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
    let response = with_admin_auth(builder).send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::FORBIDDEN
        && (body.contains(crate::proxy::ADMIN_TOKEN_HEADER)
            || body.contains(crate::proxy::ADMIN_TOKEN_ENV_VAR))
    {
        bail!("admin access denied: {body}");
    }

    if body.trim().is_empty() {
        bail!("admin request failed: {status}");
    }
    bail!("admin request failed: {status}: {body}");
}

impl ProxyController {
    pub fn new(default_port: u16, default_service: ServiceKind) -> Self {
        Self {
            mode: ProxyMode::Stopped,
            desired_port: default_port,
            desired_service: default_service,
            last_start_error: None,
            port_in_use_modal: None,
            http_client: Client::new(),
            discovered: Vec::new(),
            last_discovery_scan: None,
        }
    }

    pub fn set_defaults(&mut self, port: u16, service: ServiceKind) {
        self.desired_port = port;
        self.desired_service = service;
    }

    pub fn desired_port(&self) -> u16 {
        self.desired_port
    }

    pub fn desired_service(&self) -> ServiceKind {
        self.desired_service
    }

    pub fn set_desired_port(&mut self, port: u16) {
        self.desired_port = port;
    }

    pub fn set_desired_service(&mut self, service: ServiceKind) {
        self.desired_service = service;
    }

    pub fn kind(&self) -> ProxyModeKind {
        match self.mode {
            ProxyMode::Stopped => ProxyModeKind::Stopped,
            ProxyMode::Starting => ProxyModeKind::Starting,
            ProxyMode::Running(_) => ProxyModeKind::Running,
            ProxyMode::Attached(_) => ProxyModeKind::Attached,
        }
    }

    pub fn last_start_error(&self) -> Option<&str> {
        self.last_start_error.as_deref()
    }

    pub fn discovered_proxies(&self) -> &[DiscoveredProxy] {
        &self.discovered
    }

    pub fn last_discovery_scan(&self) -> Option<Instant> {
        self.last_discovery_scan
    }

    pub fn running(&self) -> Option<&RunningProxy> {
        match &self.mode {
            ProxyMode::Running(r) => Some(r),
            _ => None,
        }
    }

    pub fn attached(&self) -> Option<&AttachedStatus> {
        match &self.mode {
            ProxyMode::Attached(s) => Some(s),
            _ => None,
        }
    }

    pub fn control_trace_source(&self) -> Option<ControlTraceDataSource> {
        match &self.mode {
            ProxyMode::Running(_) => Some(ControlTraceDataSource::LocalFile {
                path: control_trace_path(),
            }),
            ProxyMode::Attached(att) if att.supports_control_trace_api => {
                Some(ControlTraceDataSource::AttachedApi {
                    admin_base_url: att.admin_base_url.clone(),
                })
            }
            ProxyMode::Attached(att) => Some(ControlTraceDataSource::AttachedFallbackLocal {
                admin_base_url: att.admin_base_url.clone(),
                path: control_trace_path(),
            }),
            _ => None,
        }
    }

    pub fn control_trace_source_signature(&self) -> Option<String> {
        self.control_trace_source().map(|source| source.signature())
    }

    pub fn read_control_trace_entries(
        &self,
        rt: &tokio::runtime::Runtime,
        limit: usize,
    ) -> anyhow::Result<ControlTraceReadResult> {
        let limit = limit.clamp(20, 400);
        let source = self
            .control_trace_source()
            .ok_or_else(|| anyhow::anyhow!("proxy is not running/attached"))?;

        let entries = match &source {
            ControlTraceDataSource::LocalFile { .. }
            | ControlTraceDataSource::AttachedFallbackLocal { .. } => {
                read_recent_control_trace_entries(limit)?
            }
            ControlTraceDataSource::AttachedApi { admin_base_url } => {
                let control_trace_path = self
                    .attached()
                    .and_then(|att| att.operator_summary_links.as_ref())
                    .map(|links| links.control_trace.as_str())
                    .unwrap_or("/__codex_helper/api/v1/control-trace")
                    .to_string();
                let client = self.http_client.clone();
                let admin_base_url = admin_base_url.clone();
                rt.block_on(async move {
                    let response = send_admin_request(
                        client
                            .get(format!(
                                "{admin_base_url}{control_trace_path}?limit={limit}"
                            ))
                            .timeout(Duration::from_millis(800)),
                    )
                    .await?;
                    let entries = response.json::<Vec<ControlTraceLogEntry>>().await?;
                    Ok::<Vec<ControlTraceLogEntry>, anyhow::Error>(entries)
                })?
            }
        };

        Ok(ControlTraceReadResult { source, entries })
    }

    pub fn snapshot(&self) -> Option<GuiRuntimeSnapshot> {
        match &self.mode {
            ProxyMode::Running(r) => Some(GuiRuntimeSnapshot {
                kind: ProxyModeKind::Running,
                base_url: Some(local_proxy_base_url(r.port)),
                port: Some(r.port),
                service_name: Some(r.service_name.to_string()),
                last_error: r.last_error.clone(),
                active: r.active.clone(),
                recent: r.recent.clone(),
                session_cards: r.session_cards.clone(),
                global_station_override: r.global_station_override.clone(),
                configured_active_station: r.configured_active_station.clone(),
                effective_active_station: r.effective_active_station.clone(),
                configured_default_profile: r.configured_default_profile.clone(),
                default_profile: r.default_profile.clone(),
                profiles: r.profiles.clone(),
                providers: r.providers.clone(),
                session_model_overrides: r.session_model_overrides.clone(),
                session_station_overrides: r.session_station_overrides.clone(),
                session_effort_overrides: r.session_effort_overrides.clone(),
                session_service_tier_overrides: r.session_service_tier_overrides.clone(),
                session_stats: r.session_stats.clone(),
                stations: r.stations.clone(),
                usage_rollup: r.usage_rollup.clone(),
                stats_5m: r.stats_5m.clone(),
                stats_1h: r.stats_1h.clone(),
                operator_runtime_summary: Some(local_operator_runtime_summary(r)),
                operator_retry_summary: Some(local_operator_retry_summary(r)),
                operator_health_summary: Some(local_operator_health_summary(r)),
                operator_counts: Some(local_operator_counts(r)),
                supports_operator_summary_api: true,
                configured_retry: r.configured_retry.clone(),
                resolved_retry: r.resolved_retry.clone(),
                supports_v1: true,
                supports_retry_config_api: true,
                supports_persisted_station_settings: true,
                supports_default_profile_override: true,
                supports_station_runtime_override: true,
                supports_session_override_reset: true,
                shared_capabilities: local_shared_control_plane_capabilities(),
                host_local_capabilities: local_host_local_control_plane_capabilities(),
                remote_admin_access: local_remote_admin_access_capabilities(),
            }),
            ProxyMode::Attached(a) => Some(GuiRuntimeSnapshot {
                kind: ProxyModeKind::Attached,
                base_url: Some(a.base_url.clone()),
                port: Some(a.port),
                service_name: a.service_name.clone(),
                last_error: a.last_error.clone(),
                active: a.active.clone(),
                recent: a.recent.clone(),
                session_cards: a.session_cards.clone(),
                global_station_override: a.global_station_override.clone(),
                configured_active_station: a.configured_active_station.clone(),
                effective_active_station: a.effective_active_station.clone(),
                configured_default_profile: a.configured_default_profile.clone(),
                default_profile: a.default_profile.clone(),
                profiles: a.profiles.clone(),
                providers: a.providers.clone(),
                session_model_overrides: a.session_model_overrides.clone(),
                session_station_overrides: a.session_station_overrides.clone(),
                session_effort_overrides: a.session_effort_overrides.clone(),
                session_service_tier_overrides: a.session_service_tier_overrides.clone(),
                session_stats: a.session_stats.clone(),
                stations: a.stations.clone(),
                usage_rollup: a.usage_rollup.clone(),
                stats_5m: a.stats_5m.clone(),
                stats_1h: a.stats_1h.clone(),
                operator_runtime_summary: a.operator_runtime_summary.clone(),
                operator_retry_summary: a.operator_retry_summary.clone(),
                operator_health_summary: a.operator_health_summary.clone(),
                operator_counts: a.operator_counts.clone(),
                supports_operator_summary_api: a.supports_operator_summary_api,
                configured_retry: a.configured_retry.clone(),
                resolved_retry: a.resolved_retry.clone(),
                supports_v1: a.api_version == Some(1),
                supports_retry_config_api: a.supports_retry_config_api,
                supports_persisted_station_settings: a.supports_persisted_station_settings,
                supports_default_profile_override: a.supports_default_profile_override,
                supports_station_runtime_override: a.supports_station_runtime_override,
                supports_session_override_reset: a.supports_session_override_reset,
                shared_capabilities: a.shared_capabilities.clone(),
                host_local_capabilities: a.host_local_capabilities.clone(),
                remote_admin_access: a.remote_admin_access.clone(),
            }),
            _ => None,
        }
    }

    pub fn refresh_current_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        match self.kind() {
            ProxyModeKind::Running => self.refresh_running_if_due(rt, refresh_every),
            ProxyModeKind::Attached => self.refresh_attached_if_due(rt, refresh_every),
            _ => {}
        }
    }

    pub fn show_port_in_use_modal(&self) -> bool {
        self.port_in_use_modal.is_some()
    }

    pub fn clear_port_in_use_modal(&mut self) {
        self.port_in_use_modal = None;
    }

    pub fn stop(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let ProxyMode::Running(mut running) = std::mem::replace(&mut self.mode, ProxyMode::Stopped)
        else {
            self.mode = ProxyMode::Stopped;
            return Ok(());
        };

        let _ = running.shutdown_tx.send(true);
        if let Some(mut handle) = running.server_handle.take() {
            let joined = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), &mut handle).await
            });
            match joined {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    return Err(e);
                }
                Ok(Err(join_err)) => {
                    return Err(anyhow::anyhow!("server task join error: {join_err}"));
                }
                Err(_) => {
                    handle.abort();
                }
            }
        }
        Ok(())
    }

    pub fn request_attach(&mut self, port: u16) {
        self.request_attach_with_admin_base(port, None);
    }

    pub fn detach(&mut self) {
        self.mode = ProxyMode::Stopped;
        self.last_start_error = None;
        self.port_in_use_modal = None;
    }
}

fn local_operator_runtime_summary(r: &RunningProxy) -> OperatorRuntimeSummary {
    let mgr = match r.service_name {
        "claude" => &r.cfg.claude,
        _ => &r.cfg.codex,
    };
    let default_profile_summary = r.default_profile.as_deref().and_then(|profile_name| {
        crate::config::resolve_service_profile(mgr, profile_name)
            .ok()
            .map(|profile| OperatorProfileSummary {
                name: profile_name.to_string(),
                station: profile.station,
                model: profile.model,
                reasoning_effort: profile.reasoning_effort,
                service_tier: profile.service_tier.clone(),
                fast_mode: profile.service_tier.as_deref() == Some("priority"),
            })
    });

    OperatorRuntimeSummary {
        runtime_loaded_at_ms: None,
        runtime_source_mtime_ms: None,
        configured_active_station: r.configured_active_station.clone(),
        effective_active_station: r.effective_active_station.clone(),
        global_station_override: r.global_station_override.clone(),
        configured_default_profile: r.configured_default_profile.clone(),
        default_profile: r.default_profile.clone(),
        default_profile_summary,
    }
}

fn local_operator_retry_summary(r: &RunningProxy) -> OperatorRetrySummary {
    let resolved_retry = r
        .resolved_retry
        .clone()
        .or_else(|| r.configured_retry.clone().map(|retry| retry.resolve()))
        .unwrap_or_else(|| RetryConfig::default().resolve());
    let retry_observations = summarize_recent_retry_observations(&r.recent);
    OperatorRetrySummary {
        configured_profile: r.configured_retry.as_ref().and_then(|retry| retry.profile),
        supports_write: true,
        upstream_max_attempts: resolved_retry.upstream.max_attempts,
        provider_max_attempts: resolved_retry.provider.max_attempts,
        allow_cross_station_before_first_output: resolved_retry
            .allow_cross_station_before_first_output,
        recent_retried_requests: retry_observations.recent_retried_requests,
        recent_cross_station_failovers: retry_observations.recent_cross_station_failovers,
        recent_fast_mode_requests: retry_observations.recent_fast_mode_requests,
    }
}

fn local_operator_counts(r: &RunningProxy) -> OperatorSummaryCounts {
    let mgr = match r.service_name {
        "claude" => &r.cfg.claude,
        _ => &r.cfg.codex,
    };
    OperatorSummaryCounts {
        active_requests: r.active.len(),
        recent_requests: r.recent.len(),
        sessions: r.session_cards.len(),
        stations: r.stations.len(),
        profiles: mgr.profiles.len(),
        providers: r.providers.len(),
    }
}

fn local_operator_health_summary(r: &RunningProxy) -> OperatorHealthSummary {
    build_operator_health_summary(&r.stations, &r.station_health, &r.health_checks, &r.lb_view)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
