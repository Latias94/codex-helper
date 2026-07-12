use std::time::Instant;

use tokio::sync::mpsc;

use crate::proxy::{CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse, ProxyService};
use crate::tui::i18n;
use crate::tui::model::Snapshot;
use crate::tui::state::UiState;

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayDiagnosticsResult {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) result: Result<CodexRelayCapabilitiesResponse, String>,
}

pub(in crate::tui) type CodexRelayDiagnosticsSender =
    mpsc::UnboundedSender<CodexRelayDiagnosticsResult>;

pub(in crate::tui) fn infer_codex_relay_diagnostics_model(
    ui: &UiState,
    snapshot: &Snapshot,
) -> Option<String> {
    let selected = snapshot.rows.get(ui.selected_session_idx);
    selected
        .and_then(|row| {
            row.effective_model
                .as_ref()
                .map(|value| value.value.as_str())
        })
        .or_else(|| selected.and_then(|row| row.override_model.as_deref()))
        .or_else(|| selected.and_then(|row| row.last_model.as_deref()))
        .or_else(|| {
            snapshot
                .recent
                .iter()
                .find_map(|request| request.model.as_deref())
        })
        .or_else(|| {
            let default_profile = ui.effective_default_profile.as_deref()?;
            ui.profile_options
                .iter()
                .find(|profile| profile.name == default_profile)
                .and_then(|profile| profile.model.as_deref())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(in crate::tui) fn request_codex_relay_diagnostics(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    tx: &CodexRelayDiagnosticsSender,
) -> bool {
    if ui.service_name != "codex" {
        ui.toast = Some((
            i18n::label(
                ui.language,
                "Codex relay diagnostics are only available for Codex service",
            )
            .to_string(),
            Instant::now(),
        ));
        return true;
    }

    if ui.codex_relay_diagnostics.loading {
        ui.toast = Some((
            i18n::label(ui.language, "Codex relay diagnostics already running").to_string(),
            Instant::now(),
        ));
        return true;
    }

    let generation = ui.codex_relay_diagnostics.generation.saturating_add(1);
    let request = CodexRelayCapabilitiesRequest {
        model: infer_codex_relay_diagnostics_model(ui, snapshot),
        ..CodexRelayCapabilitiesRequest::default()
    };
    ui.codex_relay_diagnostics.generation = generation;
    ui.codex_relay_diagnostics.loading = true;
    ui.codex_relay_diagnostics.last_started_at = Some(Instant::now());
    ui.codex_relay_diagnostics.last_error = None;
    ui.toast = Some((
        i18n::label(ui.language, "Codex relay diagnostics started").to_string(),
        Instant::now(),
    ));

    let proxy = proxy.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = proxy
            .codex_relay_capabilities(request)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(CodexRelayDiagnosticsResult { generation, result });
    });

    true
}

pub(in crate::tui) fn apply_codex_relay_diagnostics_result(
    ui: &mut UiState,
    result: CodexRelayDiagnosticsResult,
) -> bool {
    if result.generation != ui.codex_relay_diagnostics.generation {
        return false;
    }

    ui.codex_relay_diagnostics.loading = false;
    ui.codex_relay_diagnostics.last_finished_at = Some(Instant::now());
    match result.result {
        Ok(response) => {
            let recommended = response.recommendation.recommended_patch_mode;
            let changed = response.recommendation.changes_current_mode;
            ui.codex_relay_diagnostics.last_error = None;
            ui.codex_relay_diagnostics.last_result = Some(response);
            ui.toast = Some((
                format!(
                    "{}: {}{}",
                    i18n::label(ui.language, "Codex relay recommendation"),
                    recommended.as_preset_str(),
                    if changed { " *" } else { "" }
                ),
                Instant::now(),
            ));
        }
        Err(error) => {
            ui.codex_relay_diagnostics.last_result = None;
            ui.codex_relay_diagnostics.last_error = Some(error.clone());
            ui.toast = Some((
                format!(
                    "{}: {error}",
                    i18n::label(ui.language, "Codex relay diagnostics failed")
                ),
                Instant::now(),
            ));
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Instant;

    use crate::dashboard_core::ControlProfileOption;
    use crate::state::{
        FinishedRequest, RequestObservability, ResolvedRouteValue, RouteValueSource,
        SessionObservationScope,
    };
    use crate::tui::model::{SessionRow, Snapshot};

    use super::*;

    fn finished_request_with_model(model: &str) -> FinishedRequest {
        FinishedRequest {
            id: 1,
            trace_id: None,
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some(model.to_string()),
            reasoning_effort: None,
            service_tier: None,
            station_name: None,
            provider_id: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            accounting: Default::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 1,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1,
        }
    }

    fn session_row() -> SessionRow {
        SessionRow {
            session_id: Some("sid".to_string()),
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_station_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: None,
            avg_output_tokens_per_second: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station_name: None,
            override_route_target: None,
            override_service_tier: None,
        }
    }

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            quota_analytics: crate::quota_analytics::QuotaAnalyticsView::default(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            provider_endpoint_policy_actions: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: Instant::now(),
        }
    }

    fn profile(name: &str, model: &str) -> ControlProfileOption {
        ControlProfileOption {
            name: name.to_string(),
            extends: None,
            station: None,
            model: Some(model.to_string()),
            reasoning_effort: None,
            service_tier: None,
            fast_mode: false,
            is_default: false,
        }
    }

    #[test]
    fn codex_relay_diagnostics_model_prefers_selected_effective_model() {
        let mut row = session_row();
        row.effective_model = Some(ResolvedRouteValue {
            value: "gpt-5.5".to_string(),
            source: RouteValueSource::RequestPayload,
        });
        row.override_model = Some("override-model".to_string());
        row.last_model = Some("last-model".to_string());
        let mut snapshot = empty_snapshot();
        snapshot.rows.push(row);
        let ui = UiState::default();

        let model = infer_codex_relay_diagnostics_model(&ui, &snapshot);

        assert_eq!(model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn codex_relay_diagnostics_model_uses_default_profile_when_runtime_is_empty() {
        let snapshot = empty_snapshot();
        let ui = UiState {
            effective_default_profile: Some("image".to_string()),
            profile_options: vec![profile("image", "gpt-image-capable")],
            ..UiState::default()
        };

        let model = infer_codex_relay_diagnostics_model(&ui, &snapshot);

        assert_eq!(model.as_deref(), Some("gpt-image-capable"));
    }

    #[test]
    fn codex_relay_diagnostics_model_uses_recent_request_before_profile() {
        let mut snapshot = empty_snapshot();
        snapshot
            .recent
            .push(finished_request_with_model("recent-model"));
        let ui = UiState {
            effective_default_profile: Some("image".to_string()),
            profile_options: vec![profile("image", "profile-model")],
            ..UiState::default()
        };

        let model = infer_codex_relay_diagnostics_model(&ui, &snapshot);

        assert_eq!(model.as_deref(), Some("recent-model"));
    }
}
