use ratatui::widgets::TableState;

use crate::codex_integration::CodexStartupReadiness;
use crate::dashboard_core::{
    ControlProfileOption, OperatorActionCapabilities, OperatorLocalSessionMetadata,
    OperatorReadModel, OperatorRetrySummary,
};
use crate::proxy::{OperatorRoutingMutationRequest, OperatorSessionAffinityMutationRequest};
use crate::sessions::{
    SessionMeta, SessionSummary, SessionSummarySource, SessionTranscriptMessage,
};
use crate::usage_providers::UsageProviderRefreshSummary;
use codex_helper_core::fleet::FleetSnapshot;
use std::collections::HashMap;

use super::Language;
use super::i18n::{self, msg};
use super::model::{
    Snapshot, codex_recent_window_threshold_ms, dashboard_request_filtered_indices, now_ms,
    request_matches_page_filters, request_page_focus_session_id,
};
use super::operator_actions::PendingOperatorAction;
use super::types::{Focus, Overlay, Page, StatsFocus};

#[derive(Debug, Clone)]
pub(in crate::tui) struct RecentCodexRow {
    pub(in crate::tui) root: String,
    pub(in crate::tui) branch: Option<String>,
    pub(in crate::tui) session_id: String,
    pub(in crate::tui) cwd: Option<String>,
    pub(in crate::tui) mtime_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum FleetViewMode {
    #[default]
    Tree,
    Flat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum CodexHistoryExternalFocusOrigin {
    Sessions,
    Requests,
    Recent,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexHistoryExternalFocus {
    pub(in crate::tui) summary: SessionSummary,
    pub(in crate::tui) origin: CodexHistoryExternalFocusOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum RuntimeConnectionKind {
    Integrated,
    LocalAttached,
    RemoteObserver,
}

impl RuntimeConnectionKind {
    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        match (self, lang) {
            (RuntimeConnectionKind::Integrated, Language::Zh) => "内置",
            (RuntimeConnectionKind::Integrated, Language::En) => "integrated",
            (RuntimeConnectionKind::LocalAttached, Language::Zh) => "本机附着",
            (RuntimeConnectionKind::LocalAttached, Language::En) => "local attached",
            (RuntimeConnectionKind::RemoteObserver, Language::Zh) => "远程观察",
            (RuntimeConnectionKind::RemoteObserver, Language::En) => "remote observer",
        }
    }

    pub(in crate::tui) fn is_attached(self) -> bool {
        matches!(self, Self::LocalAttached | Self::RemoteObserver)
    }

    pub(in crate::tui) fn allows_local_codex_switch(self) -> bool {
        !matches!(self, Self::RemoteObserver)
    }

    pub(in crate::tui) fn is_remote_observer(self) -> bool {
        matches!(self, Self::RemoteObserver)
    }
}

const STATS_PROJECT_OMITTED_KEY: &str = "\0quota-project:omitted";
const STATS_PROJECT_UNKNOWN_KEY: &str = "\0quota-project:unknown";
const STATS_PROJECT_EXTERNAL_KEY: &str = "\0quota-project:external";
const STATS_PROJECT_GAP_KEY: &str = "\0quota-project:gap";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum StatsProjectRowKind {
    Project(usize),
    Omitted,
    LocalUnknown,
    ExternalUnattributed,
    SignedGap,
}

fn stats_project_synthetic_keys(
    reconciliation: &crate::quota_analytics::QuotaReconciliationView,
) -> impl Iterator<Item = &'static str> {
    [
        (
            reconciliation.omitted_projects > 0,
            STATS_PROJECT_OMITTED_KEY,
        ),
        (
            reconciliation.local_unknown.is_some(),
            STATS_PROJECT_UNKNOWN_KEY,
        ),
        (
            reconciliation.external_unattributed.is_some(),
            STATS_PROJECT_EXTERNAL_KEY,
        ),
        (reconciliation.signed_delta.is_some(), STATS_PROJECT_GAP_KEY),
    ]
    .into_iter()
    .filter_map(|(present, key)| present.then_some(key))
}

fn stats_project_rows_len(pool: &crate::quota_analytics::PoolQuotaAnalytics) -> usize {
    pool.reconciliation.projects.len() + stats_project_synthetic_keys(&pool.reconciliation).count()
}

fn stats_project_row_key(
    pool: &crate::quota_analytics::PoolQuotaAnalytics,
    index: usize,
) -> Option<String> {
    pool.reconciliation
        .projects
        .get(index)
        .map(|row| row.project.display_key().to_string())
        .or_else(|| {
            stats_project_synthetic_keys(&pool.reconciliation)
                .nth(index.saturating_sub(pool.reconciliation.projects.len()))
                .map(str::to_string)
        })
}

fn stats_project_row_index(
    pool: &crate::quota_analytics::PoolQuotaAnalytics,
    key: &str,
) -> Option<usize> {
    pool.reconciliation
        .projects
        .iter()
        .position(|row| row.project.display_key() == key)
        .or_else(|| {
            stats_project_synthetic_keys(&pool.reconciliation)
                .position(|candidate| candidate == key)
                .map(|index| pool.reconciliation.projects.len() + index)
        })
}

fn stats_project_row_kind_from_key(
    pool: &crate::quota_analytics::PoolQuotaAnalytics,
    key: &str,
) -> Option<StatsProjectRowKind> {
    if let Some(index) = pool
        .reconciliation
        .projects
        .iter()
        .position(|row| row.project.display_key() == key)
    {
        return Some(StatsProjectRowKind::Project(index));
    }

    match key {
        STATS_PROJECT_OMITTED_KEY if pool.reconciliation.omitted_projects > 0 => {
            Some(StatsProjectRowKind::Omitted)
        }
        STATS_PROJECT_UNKNOWN_KEY if pool.reconciliation.local_unknown.is_some() => {
            Some(StatsProjectRowKind::LocalUnknown)
        }
        STATS_PROJECT_EXTERNAL_KEY if pool.reconciliation.external_unattributed.is_some() => {
            Some(StatsProjectRowKind::ExternalUnattributed)
        }
        STATS_PROJECT_GAP_KEY if pool.reconciliation.signed_delta.is_some() => {
            Some(StatsProjectRowKind::SignedGap)
        }
        _ => None,
    }
}

fn stats_project_row_kind(
    pool: &crate::quota_analytics::PoolQuotaAnalytics,
    index: usize,
) -> Option<StatsProjectRowKind> {
    if pool.reconciliation.projects.get(index).is_some() {
        return Some(StatsProjectRowKind::Project(index));
    }

    stats_project_synthetic_keys(&pool.reconciliation)
        .nth(index.saturating_sub(pool.reconciliation.projects.len()))
        .and_then(|key| stats_project_row_kind_from_key(pool, key))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum RequestControlFilter {
    #[default]
    All,
    AnyEvidence,
    Signals,
    Actions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct SessionBindingEditContext {
    pub(in crate::tui) session_key: String,
    pub(in crate::tui) expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct ProfileMenuSnapshot {
    pub(in crate::tui) options: Vec<ControlProfileOption>,
    pub(in crate::tui) configured_default_profile: Option<String>,
    pub(in crate::tui) effective_default_profile: Option<String>,
    pub(in crate::tui) runtime_default_profile_override: Option<String>,
    pub(in crate::tui) control_revision: u64,
    pub(in crate::tui) catalog_key: String,
}

impl RequestControlFilter {
    pub(in crate::tui) fn next(self) -> Self {
        match self {
            Self::All => Self::AnyEvidence,
            Self::AnyEvidence => Self::Signals,
            Self::Signals => Self::Actions,
            Self::Actions => Self::All,
        }
    }

    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        match (lang, self) {
            (_, Self::All) => "all",
            (Language::Zh, Self::AnyEvidence) => "证据",
            (Language::En, Self::AnyEvidence) => "evidence",
            (Language::Zh, Self::Signals) => "信号",
            (Language::En, Self::Signals) => "signals",
            (Language::Zh, Self::Actions) => "动作",
            (Language::En, Self::Actions) => "actions",
        }
    }
}

#[derive(Debug)]
pub(in crate::tui) struct UiState {
    pub(in crate::tui) service_name: &'static str,
    pub(in crate::tui) proxy_port: u16,
    pub(in crate::tui) language: Language,
    pub(in crate::tui) runtime_connection: RuntimeConnectionKind,
    pub(in crate::tui) operator_read_model: Option<OperatorReadModel>,
    pub(in crate::tui) host_local_sessions: HashMap<String, OperatorLocalSessionMetadata>,
    pub(in crate::tui) operator_action_capabilities: OperatorActionCapabilities,
    pub(in crate::tui) local_operator_transport_available: bool,
    pub(in crate::tui) runtime_status_error: Option<String>,
    pub(in crate::tui) page: Page,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) overlay: Overlay,
    pub(in crate::tui) routing_action_selected_idx: usize,
    pub(in crate::tui) routing_confirmation: Option<OperatorRoutingMutationRequest>,
    pub(in crate::tui) session_affinity_action_selected_idx: usize,
    pub(in crate::tui) session_affinity_confirmation:
        Option<OperatorSessionAffinityMutationRequest>,
    pub(in crate::tui) session_binding_edit: Option<SessionBindingEditContext>,
    pub(in crate::tui) session_profile_menu_idx: usize,
    pub(in crate::tui) session_model_menu_idx: usize,
    pub(in crate::tui) session_model_options: Vec<String>,
    pub(in crate::tui) session_effort_menu_idx: usize,
    pub(in crate::tui) session_service_tier_menu_idx: usize,
    pub(in crate::tui) session_binding_input_kind: super::types::SessionBindingInputKind,
    pub(in crate::tui) session_binding_input: String,
    pub(in crate::tui) session_binding_input_hint: Option<String>,
    pub(in crate::tui) startup_readiness: Option<CodexStartupReadiness>,
    pub(in crate::tui) selected_provider_idx: usize,
    pub(in crate::tui) selected_routing_candidate_idx: usize,
    pub(in crate::tui) selected_routing_candidate_key: Option<(String, String)>,
    pub(in crate::tui) routing_candidates_visible_rows: usize,
    pub(in crate::tui) routing_detail_available: bool,
    pub(in crate::tui) routing_detail_focused: bool,
    pub(in crate::tui) routing_detail_scroll: u16,
    pub(in crate::tui) dashboard_details_scroll: u16,
    pub(in crate::tui) selected_session_idx: usize,
    pub(in crate::tui) selected_session_id: Option<String>,
    pub(in crate::tui) selected_request_idx: usize,
    pub(in crate::tui) selected_request_id: Option<u64>,
    pub(in crate::tui) selected_request_page_idx: usize,
    pub(in crate::tui) selected_request_page_id: Option<u64>,
    pub(in crate::tui) focused_request_session_id: Option<String>,
    pub(in crate::tui) request_page_errors_only: bool,
    pub(in crate::tui) request_page_scope_session: bool,
    pub(in crate::tui) request_page_control_filter: RequestControlFilter,
    pub(in crate::tui) selected_sessions_page_idx: usize,
    pub(in crate::tui) sessions_page_active_only: bool,
    pub(in crate::tui) sessions_page_errors_only: bool,
    pub(in crate::tui) sessions_page_overrides_only: bool,
    pub(in crate::tui) fleet_snapshot: Option<FleetSnapshot>,
    pub(in crate::tui) fleet_loading: bool,
    pub(in crate::tui) fleet_refresh_generation: u64,
    pub(in crate::tui) fleet_last_refresh_at: Option<std::time::Instant>,
    pub(in crate::tui) fleet_last_loaded_at_ms: Option<u64>,
    pub(in crate::tui) fleet_last_error: Option<String>,
    pub(in crate::tui) needs_fleet_refresh: bool,
    pub(in crate::tui) selected_fleet_node_idx: usize,
    pub(in crate::tui) selected_fleet_unit_idx: usize,
    pub(in crate::tui) selected_fleet_node_id: Option<String>,
    pub(in crate::tui) selected_fleet_unit_id: Option<String>,
    pub(in crate::tui) fleet_view_mode: FleetViewMode,
    pub(in crate::tui) profile_options: Vec<ControlProfileOption>,
    pub(in crate::tui) configured_default_profile: Option<String>,
    pub(in crate::tui) effective_default_profile: Option<String>,
    pub(in crate::tui) runtime_default_profile_override: Option<String>,
    pub(in crate::tui) default_profile_control_revision: u64,
    pub(in crate::tui) profile_catalog_key: String,
    pub(in crate::tui) profile_menu_snapshot: Option<ProfileMenuSnapshot>,
    pub(in crate::tui) settings_profile_menu_idx: usize,
    pub(in crate::tui) settings_scroll: u16,
    pub(in crate::tui) help_scroll: u16,
    pub(in crate::tui) sessions_details_scroll: u16,
    pub(in crate::tui) requests_details_scroll: u16,
    pub(in crate::tui) codex_relay_diagnostics: super::settings_relay::CodexRelayDiagnosticsState,
    pub(in crate::tui) codex_relay_live_smoke: super::settings_relay::CodexRelayLiveSmokeState,
    pub(in crate::tui) stats_focus: StatsFocus,
    pub(in crate::tui) stats_errors_only: bool,
    pub(in crate::tui) stats_attention_only: bool,
    pub(in crate::tui) selected_stats_provider_endpoint_idx: usize,
    pub(in crate::tui) selected_stats_provider_endpoint_key: Option<String>,
    pub(in crate::tui) selected_stats_pool_idx: usize,
    pub(in crate::tui) selected_stats_pool_key: Option<String>,
    pub(in crate::tui) selected_stats_project_idx: usize,
    pub(in crate::tui) selected_stats_project_key: Option<String>,
    pub(in crate::tui) selected_stats_provider_idx: usize,
    pub(in crate::tui) selected_stats_provider_key: Option<String>,
    pub(in crate::tui) stats_provider_detail_scroll: u16,
    pub(in crate::tui) selected_service_status_idx: usize,
    pub(in crate::tui) selected_service_status_key: Option<(String, Option<String>)>,
    pub(in crate::tui) service_status_visible_rows: usize,
    pub(in crate::tui) service_status_detail_available: bool,
    pub(in crate::tui) service_status_detail_focused: bool,
    pub(in crate::tui) service_status_detail_scroll: u16,
    pub(in crate::tui) needs_snapshot_refresh: bool,
    pub(in crate::tui) pending_operator_action: Option<PendingOperatorAction>,
    pub(in crate::tui) deferred_auto_balance_refresh: bool,
    pub(in crate::tui) operator_action_in_flight: bool,
    pub(in crate::tui) balance_refresh_in_flight: bool,
    pub(in crate::tui) last_balance_refresh_requested_at: Option<std::time::Instant>,
    pub(in crate::tui) last_balance_refresh_finished_at: Option<std::time::Instant>,
    pub(in crate::tui) last_balance_refresh_summary: Option<UsageProviderRefreshSummary>,
    pub(in crate::tui) last_balance_refresh_message: Option<String>,
    pub(in crate::tui) last_balance_refresh_error: Option<String>,
    pub(in crate::tui) toast: Option<(String, std::time::Instant)>,
    pub(in crate::tui) codex_history_sessions: Vec<SessionSummary>,
    pub(in crate::tui) codex_history_error: Option<String>,
    pub(in crate::tui) codex_history_loaded_at_ms: Option<u64>,
    pub(in crate::tui) codex_history_loading: bool,
    pub(in crate::tui) codex_history_refresh_generation: u64,
    pub(in crate::tui) needs_codex_history_refresh: bool,
    pub(in crate::tui) selected_codex_history_idx: usize,
    pub(in crate::tui) selected_codex_history_id: Option<String>,
    pub(in crate::tui) codex_history_external_focus: Option<CodexHistoryExternalFocus>,
    pub(in crate::tui) codex_history_details_scroll: u16,
    pub(in crate::tui) codex_recent_rows: Vec<RecentCodexRow>,
    pub(in crate::tui) codex_recent_error: Option<String>,
    pub(in crate::tui) codex_recent_loaded_at_ms: Option<u64>,
    pub(in crate::tui) codex_recent_loading: bool,
    pub(in crate::tui) codex_recent_refresh_generation: u64,
    pub(in crate::tui) needs_codex_recent_refresh: bool,
    pub(in crate::tui) codex_recent_window_idx: usize,
    pub(in crate::tui) codex_recent_selected_idx: usize,
    pub(in crate::tui) codex_recent_selected_id: Option<String>,
    pub(in crate::tui) codex_recent_raw_cwd: bool,
    pub(in crate::tui) codex_recent_branch_cache: CodexRecentBranchCache,
    pub(in crate::tui) codex_recent_details_scroll: u16,
    pub(in crate::tui) session_transcript_meta: Option<SessionMeta>,
    pub(in crate::tui) session_transcript_sid: Option<String>,
    pub(in crate::tui) session_transcript_file: Option<String>,
    pub(in crate::tui) session_transcript_tail: Option<usize>,
    pub(in crate::tui) session_transcript_messages: Vec<SessionTranscriptMessage>,
    pub(in crate::tui) session_transcript_scroll: u16,
    pub(in crate::tui) session_transcript_error: Option<String>,
    pub(in crate::tui) last_runtime_config_loaded_at_ms: Option<u64>,
    pub(in crate::tui) last_runtime_config_source_mtime_ms: Option<u64>,
    pub(in crate::tui) last_retry_summary: Option<OperatorRetrySummary>,
    pub(in crate::tui) last_runtime_config_refresh_at: Option<std::time::Instant>,
    pub(in crate::tui) should_exit: bool,
    pub(in crate::tui) providers_table: TableState,
    pub(in crate::tui) routing_candidates_table: TableState,
    pub(in crate::tui) sessions_table: TableState,
    pub(in crate::tui) requests_table: TableState,
    pub(in crate::tui) request_page_table: TableState,
    pub(in crate::tui) sessions_page_table: TableState,
    pub(in crate::tui) codex_history_table: TableState,
    pub(in crate::tui) codex_recent_table: TableState,
    pub(in crate::tui) fleet_nodes_table: TableState,
    pub(in crate::tui) fleet_units_table: TableState,
    pub(in crate::tui) stats_provider_endpoints_table: TableState,
    pub(in crate::tui) stats_providers_table: TableState,
    pub(in crate::tui) stats_pools_table: TableState,
    pub(in crate::tui) stats_projects_table: TableState,
    pub(in crate::tui) service_status_table: TableState,
    pub(in crate::tui) provider_info_scroll: u16,
    pub(in crate::tui) provider_info_endpoint_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct CodexRecentBranchCache {
    entries: HashMap<String, Option<String>>,
    loaded_at_ms: Option<u64>,
}

impl CodexRecentBranchCache {
    const MAX_ENTRIES: usize = 1_000;

    pub(in crate::tui) fn new() -> Self {
        Self::default()
    }

    pub(in crate::tui) fn clone_entries(&self) -> HashMap<String, Option<String>> {
        self.entries.clone()
    }

    pub(in crate::tui) fn replace(&mut self, entries: HashMap<String, Option<String>>) {
        let mut entries = entries;
        if entries.len() > Self::MAX_ENTRIES {
            let mut keys = entries.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let remove = keys.len().saturating_sub(Self::MAX_ENTRIES);
            for key in keys.into_iter().take(remove) {
                entries.remove(&key);
            }
        }
        self.entries = entries;
        self.loaded_at_ms = Some(crate::tui::model::now_ms());
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            service_name: "codex",
            proxy_port: 3211,
            language: Language::En,
            runtime_connection: RuntimeConnectionKind::Integrated,
            operator_read_model: None,
            host_local_sessions: HashMap::new(),
            operator_action_capabilities: OperatorActionCapabilities::default(),
            local_operator_transport_available: true,
            runtime_status_error: None,
            page: Page::Dashboard,
            focus: Focus::Sessions,
            overlay: Overlay::None,
            routing_action_selected_idx: 0,
            routing_confirmation: None,
            session_affinity_action_selected_idx: 0,
            session_affinity_confirmation: None,
            session_binding_edit: None,
            session_profile_menu_idx: 0,
            session_model_menu_idx: 0,
            session_model_options: Vec::new(),
            session_effort_menu_idx: 0,
            session_service_tier_menu_idx: 0,
            session_binding_input_kind: super::types::SessionBindingInputKind::Model,
            session_binding_input: String::new(),
            session_binding_input_hint: None,
            startup_readiness: None,
            selected_provider_idx: 0,
            selected_routing_candidate_idx: 0,
            selected_routing_candidate_key: None,
            routing_candidates_visible_rows: 10,
            routing_detail_available: false,
            routing_detail_focused: false,
            routing_detail_scroll: 0,
            dashboard_details_scroll: 0,
            selected_session_idx: 0,
            selected_session_id: None,
            selected_request_idx: 0,
            selected_request_id: None,
            selected_request_page_idx: 0,
            selected_request_page_id: None,
            focused_request_session_id: None,
            request_page_errors_only: false,
            request_page_scope_session: false,
            request_page_control_filter: RequestControlFilter::All,
            selected_sessions_page_idx: 0,
            sessions_page_active_only: false,
            sessions_page_errors_only: false,
            sessions_page_overrides_only: false,
            fleet_snapshot: None,
            fleet_loading: false,
            fleet_refresh_generation: 0,
            fleet_last_refresh_at: None,
            fleet_last_loaded_at_ms: None,
            fleet_last_error: None,
            needs_fleet_refresh: false,
            selected_fleet_node_idx: 0,
            selected_fleet_unit_idx: 0,
            selected_fleet_node_id: None,
            selected_fleet_unit_id: None,
            fleet_view_mode: FleetViewMode::default(),
            profile_options: Vec::new(),
            configured_default_profile: None,
            effective_default_profile: None,
            runtime_default_profile_override: None,
            default_profile_control_revision: 0,
            profile_catalog_key: String::new(),
            profile_menu_snapshot: None,
            settings_profile_menu_idx: 0,
            settings_scroll: 0,
            help_scroll: 0,
            sessions_details_scroll: 0,
            requests_details_scroll: 0,
            codex_relay_diagnostics: Default::default(),
            codex_relay_live_smoke: Default::default(),
            stats_focus: StatsFocus::Pools,
            stats_errors_only: false,
            stats_attention_only: false,
            selected_stats_provider_endpoint_idx: 0,
            selected_stats_provider_endpoint_key: None,
            selected_stats_pool_idx: 0,
            selected_stats_pool_key: None,
            selected_stats_project_idx: 0,
            selected_stats_project_key: None,
            selected_stats_provider_idx: 0,
            selected_stats_provider_key: None,
            stats_provider_detail_scroll: 0,
            selected_service_status_idx: 0,
            selected_service_status_key: None,
            service_status_visible_rows: 10,
            service_status_detail_available: false,
            service_status_detail_focused: false,
            service_status_detail_scroll: 0,
            needs_snapshot_refresh: false,
            pending_operator_action: None,
            deferred_auto_balance_refresh: false,
            operator_action_in_flight: false,
            balance_refresh_in_flight: false,
            last_balance_refresh_requested_at: None,
            last_balance_refresh_finished_at: None,
            last_balance_refresh_summary: None,
            last_balance_refresh_message: None,
            last_balance_refresh_error: None,
            toast: None,
            codex_history_sessions: Vec::new(),
            codex_history_error: None,
            codex_history_loaded_at_ms: None,
            codex_history_loading: false,
            codex_history_refresh_generation: 0,
            needs_codex_history_refresh: false,
            selected_codex_history_idx: 0,
            selected_codex_history_id: None,
            codex_history_external_focus: None,
            codex_history_details_scroll: 0,
            codex_recent_rows: Vec::new(),
            codex_recent_error: None,
            codex_recent_loaded_at_ms: None,
            codex_recent_loading: false,
            codex_recent_refresh_generation: 0,
            needs_codex_recent_refresh: false,
            codex_recent_window_idx: 1,
            codex_recent_selected_idx: 0,
            codex_recent_selected_id: None,
            codex_recent_raw_cwd: false,
            codex_recent_branch_cache: CodexRecentBranchCache::new(),
            codex_recent_details_scroll: 0,
            session_transcript_meta: None,
            session_transcript_sid: None,
            session_transcript_file: None,
            session_transcript_tail: Some(80),
            session_transcript_messages: Vec::new(),
            session_transcript_scroll: 0,
            session_transcript_error: None,
            last_runtime_config_loaded_at_ms: None,
            last_runtime_config_source_mtime_ms: None,
            last_retry_summary: None,
            last_runtime_config_refresh_at: None,
            should_exit: false,
            providers_table: TableState::default(),
            routing_candidates_table: TableState::default(),
            sessions_table: TableState::default(),
            requests_table: TableState::default(),
            request_page_table: TableState::default(),
            sessions_page_table: TableState::default(),
            codex_history_table: TableState::default(),
            codex_recent_table: TableState::default(),
            fleet_nodes_table: TableState::default(),
            fleet_units_table: TableState::default(),
            stats_provider_endpoints_table: TableState::default(),
            stats_providers_table: TableState::default(),
            stats_pools_table: TableState::default(),
            stats_projects_table: TableState::default(),
            service_status_table: TableState::default(),
            provider_info_scroll: 0,
            provider_info_endpoint_id: None,
        }
    }
}

impl UiState {
    pub(in crate::tui) fn capture_profile_menu_snapshot(&mut self) {
        self.profile_menu_snapshot = Some(ProfileMenuSnapshot {
            options: self.profile_options.clone(),
            configured_default_profile: self.configured_default_profile.clone(),
            effective_default_profile: self.effective_default_profile.clone(),
            runtime_default_profile_override: self.runtime_default_profile_override.clone(),
            control_revision: self.default_profile_control_revision,
            catalog_key: self.profile_catalog_key.clone(),
        });
    }

    pub(in crate::tui) fn profile_menu_options(&self) -> &[ControlProfileOption] {
        self.profile_menu_snapshot
            .as_ref()
            .map_or(self.profile_options.as_slice(), |snapshot| {
                snapshot.options.as_slice()
            })
    }

    pub(in crate::tui) fn clear_profile_menu_snapshot(&mut self) {
        self.profile_menu_snapshot = None;
    }

    pub(in crate::tui) fn allows_local_codex_switch(&self) -> bool {
        self.service_name == "codex" && self.runtime_connection.allows_local_codex_switch()
    }

    pub(in crate::tui) fn can_bridge_runtime_sessions_to_local_codex(&self) -> bool {
        self.service_name == "codex"
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => self.local_operator_transport_available,
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    fn operator_read_model_allows_actions(&self) -> bool {
        self.operator_read_model
            .as_ref()
            .is_none_or(OperatorReadModel::can_use_runtime_actions)
    }

    pub(in crate::tui) fn can_refresh_provider_balances(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.refresh_provider_balances
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_mutate_routing(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.mutate_routing
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_mutate_session_affinity(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.mutate_session_affinity
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_mutate_session_binding(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.mutate_session_binding
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_reload_runtime(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.reload_runtime
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_mutate_default_profile(&self) -> bool {
        self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.mutate_default_profile
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_inspect_relay_capabilities(&self) -> bool {
        self.service_name == "codex"
            && self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.inspect_relay_capabilities
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn can_run_relay_live_smoke(&self) -> bool {
        self.service_name == "codex"
            && self.operator_read_model_allows_actions()
            && match self.runtime_connection {
                RuntimeConnectionKind::Integrated => true,
                RuntimeConnectionKind::LocalAttached => {
                    self.local_operator_transport_available
                        && self.operator_action_capabilities.run_relay_live_smoke
                }
                RuntimeConnectionKind::RemoteObserver => false,
            }
    }

    pub(in crate::tui) fn selected_quota_pool<'a>(
        &self,
        snapshot: &'a Snapshot,
    ) -> Option<&'a crate::quota_analytics::PoolQuotaAnalytics> {
        snapshot
            .quota_analytics
            .pools
            .get(self.selected_stats_pool_idx)
    }

    pub(in crate::tui) fn selected_stats_project_row<'a>(
        &self,
        snapshot: &'a Snapshot,
    ) -> Option<(
        &'a crate::quota_analytics::PoolQuotaAnalytics,
        StatsProjectRowKind,
    )> {
        let pool = self.selected_quota_pool(snapshot)?;
        let row = self
            .selected_stats_project_key
            .as_deref()
            .and_then(|key| stats_project_row_kind_from_key(pool, key))
            .or_else(|| stats_project_row_kind(pool, self.selected_stats_project_idx))?;
        Some((pool, row))
    }

    pub(in crate::tui) fn cycle_stats_focus(&mut self) -> StatsFocus {
        self.stats_focus = match self.stats_focus {
            StatsFocus::Pools => StatsFocus::Projects,
            StatsFocus::Projects => StatsFocus::Providers,
            StatsFocus::Providers => StatsFocus::ProviderEndpoints,
            StatsFocus::ProviderEndpoints => StatsFocus::Pools,
        };
        self.stats_provider_detail_scroll = 0;
        self.stats_focus
    }

    pub(in crate::tui) fn stats_focus_label(&self) -> &'static str {
        i18n::text(
            self.language,
            match self.stats_focus {
                StatsFocus::Pools => msg::STATS_FOCUS_POOLS,
                StatsFocus::Projects => msg::STATS_FOCUS_PROJECTS,
                StatsFocus::Providers => msg::STATS_FOCUS_PROVIDERS,
                StatsFocus::ProviderEndpoints => msg::STATS_FOCUS_ENDPOINTS,
            },
        )
    }

    pub(in crate::tui) fn move_stats_selection(&mut self, snapshot: &Snapshot, delta: i32) -> bool {
        match self.stats_focus {
            StatsFocus::Pools => {
                let len = snapshot.quota_analytics.pools.len();
                let Some(next) = adjust_table_selection(&mut self.stats_pools_table, delta, len)
                else {
                    return false;
                };
                self.selected_stats_pool_idx = next;
                self.selected_stats_pool_key = snapshot
                    .quota_analytics
                    .pools
                    .get(next)
                    .map(|pool| pool.identity.key.clone());
                self.selected_stats_project_idx = 0;
                self.selected_stats_project_key = None;
                self.stats_projects_table.select(None);
                *self.stats_projects_table.offset_mut() = 0;
                true
            }
            StatsFocus::Projects => {
                let pool = snapshot
                    .quota_analytics
                    .pools
                    .get(self.selected_stats_pool_idx);
                let len = pool.map(stats_project_rows_len).unwrap_or(0);
                let Some(next) = adjust_table_selection(&mut self.stats_projects_table, delta, len)
                else {
                    return false;
                };
                self.selected_stats_project_idx = next;
                self.selected_stats_project_key =
                    pool.and_then(|pool| stats_project_row_key(pool, next));
                true
            }
            StatsFocus::Providers => {
                let len = snapshot.usage_day.provider_rows.len();
                let Some(next) =
                    adjust_table_selection(&mut self.stats_providers_table, delta, len)
                else {
                    return false;
                };
                self.selected_stats_provider_idx = next;
                self.selected_stats_provider_key = snapshot
                    .usage_day
                    .provider_rows
                    .get(next)
                    .map(|row| row.name.clone());
                self.stats_provider_detail_scroll = 0;
                true
            }
            StatsFocus::ProviderEndpoints => {
                let len = snapshot.usage_day.provider_endpoint_rows.len();
                let Some(next) =
                    adjust_table_selection(&mut self.stats_provider_endpoints_table, delta, len)
                else {
                    return false;
                };
                self.selected_stats_provider_endpoint_idx = next;
                self.selected_stats_provider_endpoint_key = snapshot
                    .usage_day
                    .provider_endpoint_rows
                    .get(next)
                    .map(|row| row.name.clone());
                true
            }
        }
    }

    pub(in crate::tui) fn clamp_selection(&mut self, snapshot: &Snapshot, providers_len: usize) {
        self.selected_provider_idx = clamp_table_selection(
            &mut self.providers_table,
            Some(self.selected_provider_idx),
            providers_len,
        )
        .unwrap_or(0);

        let previous_routing_candidate_key = self.selected_routing_candidate_key.clone();
        if let Some(routing) = snapshot.routing.as_ref() {
            if let Some((provider_id, endpoint_id)) = self.selected_routing_candidate_key.as_ref()
                && let Some(index) = routing.candidates.iter().position(|candidate| {
                    candidate.provider_id == *provider_id && candidate.endpoint_id == *endpoint_id
                })
            {
                self.selected_routing_candidate_idx = index;
            } else if self.selected_routing_candidate_key.is_none()
                && let Some(target) = routing.new_session_preference.as_ref()
                && let Some(index) = routing.candidates.iter().position(|candidate| {
                    candidate.provider_id == target.provider_id
                        && candidate.endpoint_id == target.endpoint_id
                })
            {
                self.selected_routing_candidate_idx = index;
            }
            self.selected_routing_candidate_idx = clamp_table_selection(
                &mut self.routing_candidates_table,
                Some(self.selected_routing_candidate_idx),
                routing.candidates.len(),
            )
            .unwrap_or(0);
            self.selected_routing_candidate_key = routing
                .candidates
                .get(self.selected_routing_candidate_idx)
                .map(|candidate| (candidate.provider_id.clone(), candidate.endpoint_id.clone()));
            if self.selected_routing_candidate_key != previous_routing_candidate_key {
                self.routing_detail_scroll = 0;
            }
        } else {
            self.selected_routing_candidate_idx = 0;
            self.selected_routing_candidate_key = None;
            self.routing_detail_available = false;
            self.routing_detail_focused = false;
            self.routing_detail_scroll = 0;
            clamp_table_selection(&mut self.routing_candidates_table, None, 0);
        }

        if snapshot.rows.is_empty() {
            self.selected_session_idx = 0;
            self.selected_session_id = None;
            clamp_table_selection(&mut self.sessions_table, None, 0);
        } else {
            if let Some(sid) = self.selected_session_id.clone()
                && let Some(idx) = snapshot
                    .rows
                    .iter()
                    .position(|r| r.session_id.as_deref() == Some(sid.as_str()))
            {
                self.selected_session_idx = idx;
            } else {
                self.selected_session_idx = self.selected_session_idx.min(snapshot.rows.len() - 1);
                self.selected_session_id =
                    snapshot.rows[self.selected_session_idx].session_id.clone();
            }
            self.selected_session_idx = clamp_table_selection(
                &mut self.sessions_table,
                Some(self.selected_session_idx),
                snapshot.rows.len(),
            )
            .unwrap_or(0);
        }
        self.sync_dashboard_request_selection(snapshot);

        if let Some(pool_key) = self.selected_stats_pool_key.as_deref()
            && let Some(index) = snapshot
                .quota_analytics
                .pools
                .iter()
                .position(|pool| pool.identity.key == pool_key)
        {
            self.selected_stats_pool_idx = index;
        }
        self.selected_stats_pool_idx = clamp_table_selection(
            &mut self.stats_pools_table,
            Some(self.selected_stats_pool_idx),
            snapshot.quota_analytics.pools.len(),
        )
        .unwrap_or(0);
        self.selected_stats_pool_key = snapshot
            .quota_analytics
            .pools
            .get(self.selected_stats_pool_idx)
            .map(|pool| pool.identity.key.clone());

        let selected_pool = snapshot
            .quota_analytics
            .pools
            .get(self.selected_stats_pool_idx);
        if let Some(project_key) = self.selected_stats_project_key.as_deref()
            && let Some(index) =
                selected_pool.and_then(|pool| stats_project_row_index(pool, project_key))
        {
            self.selected_stats_project_idx = index;
        }
        let project_rows_len = selected_pool.map(stats_project_rows_len).unwrap_or(0);
        self.selected_stats_project_idx = clamp_table_selection(
            &mut self.stats_projects_table,
            Some(self.selected_stats_project_idx),
            project_rows_len,
        )
        .unwrap_or(0);
        self.selected_stats_project_key = selected_pool
            .and_then(|pool| stats_project_row_key(pool, self.selected_stats_project_idx));

        if let Some(endpoint_key) = self.selected_stats_provider_endpoint_key.as_deref()
            && let Some(index) = snapshot
                .usage_day
                .provider_endpoint_rows
                .iter()
                .position(|row| row.name == endpoint_key)
        {
            self.selected_stats_provider_endpoint_idx = index;
        }
        let stats_provider_endpoints_len = snapshot.usage_day.provider_endpoint_rows.len();
        self.selected_stats_provider_endpoint_idx = clamp_table_selection(
            &mut self.stats_provider_endpoints_table,
            Some(self.selected_stats_provider_endpoint_idx),
            stats_provider_endpoints_len,
        )
        .unwrap_or(0);
        self.selected_stats_provider_endpoint_key = snapshot
            .usage_day
            .provider_endpoint_rows
            .get(self.selected_stats_provider_endpoint_idx)
            .map(|row| row.name.clone());

        let previous_stats_provider_key = self.selected_stats_provider_key.clone();
        if let Some(provider_key) = self.selected_stats_provider_key.as_deref()
            && let Some(index) = snapshot
                .usage_day
                .provider_rows
                .iter()
                .position(|row| row.name == provider_key)
        {
            self.selected_stats_provider_idx = index;
        }
        let stats_providers_len = snapshot.usage_day.provider_rows.len();
        self.selected_stats_provider_idx = clamp_table_selection(
            &mut self.stats_providers_table,
            Some(self.selected_stats_provider_idx),
            stats_providers_len,
        )
        .unwrap_or(0);
        self.selected_stats_provider_key = snapshot
            .usage_day
            .provider_rows
            .get(self.selected_stats_provider_idx)
            .map(|row| row.name.clone());
        if stats_providers_len == 0
            || self.selected_stats_provider_key != previous_stats_provider_key
        {
            self.stats_provider_detail_scroll = 0;
        }

        self.sync_service_status_selection(snapshot);
    }

    pub(in crate::tui) fn reset_table_viewports(&mut self) {
        for table in [
            &mut self.providers_table,
            &mut self.routing_candidates_table,
            &mut self.sessions_table,
            &mut self.requests_table,
            &mut self.request_page_table,
            &mut self.sessions_page_table,
            &mut self.codex_history_table,
            &mut self.codex_recent_table,
            &mut self.fleet_nodes_table,
            &mut self.fleet_units_table,
            &mut self.stats_provider_endpoints_table,
            &mut self.stats_providers_table,
            &mut self.stats_pools_table,
            &mut self.stats_projects_table,
            &mut self.service_status_table,
        ] {
            *table.offset_mut() = 0;
        }
        self.routing_detail_scroll = 0;
        self.dashboard_details_scroll = 0;
        self.stats_provider_detail_scroll = 0;
        self.codex_history_details_scroll = 0;
        self.codex_recent_details_scroll = 0;
        self.service_status_detail_scroll = 0;
    }

    pub(in crate::tui) fn sync_providers_table_viewport(
        &mut self,
        providers_len: usize,
        visible_rows: usize,
    ) {
        self.selected_provider_idx = clamp_table_viewport(
            &mut self.providers_table,
            Some(self.selected_provider_idx),
            providers_len,
            visible_rows,
        )
        .unwrap_or(0);
    }

    pub(in crate::tui) fn sync_routing_candidates_table_viewport(
        &mut self,
        snapshot: &Snapshot,
        visible_rows: usize,
    ) {
        self.routing_candidates_visible_rows = visible_rows.max(1);
        let candidates = snapshot
            .routing
            .as_ref()
            .map(|routing| routing.candidates.as_slice())
            .unwrap_or_default();
        self.selected_routing_candidate_idx = clamp_table_viewport(
            &mut self.routing_candidates_table,
            Some(self.selected_routing_candidate_idx),
            candidates.len(),
            visible_rows,
        )
        .unwrap_or(0);
        self.selected_routing_candidate_key = candidates
            .get(self.selected_routing_candidate_idx)
            .map(|candidate| (candidate.provider_id.clone(), candidate.endpoint_id.clone()));
    }

    pub(in crate::tui) fn move_routing_selection(
        &mut self,
        snapshot: &Snapshot,
        delta: i32,
    ) -> bool {
        let Some(routing) = snapshot.routing.as_ref() else {
            return false;
        };
        let Some(next) = adjust_table_selection(
            &mut self.routing_candidates_table,
            delta,
            routing.candidates.len(),
        ) else {
            return false;
        };
        self.selected_routing_candidate_idx = next;
        self.selected_routing_candidate_key = routing
            .candidates
            .get(next)
            .map(|candidate| (candidate.provider_id.clone(), candidate.endpoint_id.clone()));
        self.routing_detail_scroll = 0;
        true
    }

    pub(in crate::tui) fn select_routing_candidate_index(
        &mut self,
        snapshot: &Snapshot,
        index: usize,
    ) -> bool {
        let Some(routing) = snapshot.routing.as_ref() else {
            return false;
        };
        let Some(candidate) = routing.candidates.get(index) else {
            return false;
        };
        self.selected_routing_candidate_idx = index;
        self.selected_routing_candidate_key =
            Some((candidate.provider_id.clone(), candidate.endpoint_id.clone()));
        self.routing_candidates_table.select(Some(index));
        self.routing_detail_scroll = 0;
        true
    }

    pub(in crate::tui) fn select_preferred_routing_candidate(
        &mut self,
        snapshot: &Snapshot,
    ) -> bool {
        let Some(routing) = snapshot.routing.as_ref() else {
            return false;
        };
        let Some(target) = routing.new_session_preference.as_ref() else {
            return false;
        };
        let Some(index) = routing.candidates.iter().position(|candidate| {
            candidate.provider_id == target.provider_id
                && candidate.endpoint_id == target.endpoint_id
        }) else {
            return false;
        };
        self.select_routing_candidate_index(snapshot, index)
    }

    pub(in crate::tui) fn selected_routing_candidate<'a>(
        &self,
        snapshot: &'a Snapshot,
    ) -> Option<&'a crate::dashboard_core::OperatorRouteCandidateSummary> {
        snapshot
            .routing
            .as_ref()?
            .candidates
            .get(self.selected_routing_candidate_idx)
    }

    pub(in crate::tui) fn sync_service_status_selection(&mut self, snapshot: &Snapshot) {
        let keys = service_status_row_keys(snapshot);
        let previous_key = self.selected_service_status_key.clone();
        if let Some(key) = previous_key.as_ref()
            && let Some(index) = keys.iter().position(|candidate| candidate == key)
        {
            self.selected_service_status_idx = index;
        }
        self.selected_service_status_idx = clamp_table_selection(
            &mut self.service_status_table,
            Some(self.selected_service_status_idx),
            keys.len(),
        )
        .unwrap_or(0);
        self.selected_service_status_key = keys.get(self.selected_service_status_idx).cloned();
        if self.selected_service_status_key != previous_key {
            self.service_status_detail_scroll = 0;
        }
        if keys.is_empty() {
            self.service_status_detail_available = false;
            self.service_status_detail_focused = false;
            self.service_status_detail_scroll = 0;
        }
    }

    pub(in crate::tui) fn sync_service_status_table_viewport(
        &mut self,
        snapshot: &Snapshot,
        visible_rows: usize,
    ) {
        self.service_status_visible_rows = visible_rows.max(1);
        let keys = service_status_row_keys(snapshot);
        self.selected_service_status_idx = clamp_table_viewport(
            &mut self.service_status_table,
            Some(self.selected_service_status_idx),
            keys.len(),
            visible_rows,
        )
        .unwrap_or(0);
        self.selected_service_status_key = keys.get(self.selected_service_status_idx).cloned();
    }

    pub(in crate::tui) fn select_service_status_index(
        &mut self,
        snapshot: &Snapshot,
        index: usize,
    ) -> bool {
        let keys = service_status_row_keys(snapshot);
        let Some(key) = keys.get(index) else {
            return false;
        };
        self.selected_service_status_idx = index;
        self.selected_service_status_key = Some(key.clone());
        self.service_status_table.select(Some(index));
        self.service_status_detail_scroll = 0;
        true
    }

    pub(in crate::tui) fn move_service_status_selection(
        &mut self,
        snapshot: &Snapshot,
        delta: i32,
    ) -> bool {
        let keys = service_status_row_keys(snapshot);
        let Some(index) = adjust_table_selection(&mut self.service_status_table, delta, keys.len())
        else {
            return false;
        };
        self.selected_service_status_idx = index;
        self.selected_service_status_key = keys.get(index).cloned();
        self.service_status_detail_scroll = 0;
        true
    }

    pub(in crate::tui) fn select_service_status_edge(
        &mut self,
        snapshot: &Snapshot,
        last: bool,
    ) -> bool {
        let len = service_status_row_keys(snapshot).len();
        if len == 0 {
            return false;
        }
        self.select_service_status_index(snapshot, if last { len - 1 } else { 0 })
    }

    pub(in crate::tui) fn sync_selected_provider_from_routing(
        &mut self,
        snapshot: &Snapshot,
        providers: &[crate::tui::ProviderOption],
    ) {
        self.provider_info_endpoint_id = None;
        let Some((provider_id, endpoint_id)) =
            self.selected_routing_candidate(snapshot).map(|candidate| {
                (
                    candidate.provider_id.as_str(),
                    candidate.endpoint_id.as_str(),
                )
            })
        else {
            return;
        };
        self.provider_info_endpoint_id = Some(endpoint_id.to_string());
        if let Some(index) = providers
            .iter()
            .position(|provider| provider.name == provider_id)
        {
            self.selected_provider_idx = index;
            self.providers_table.select(Some(index));
        }
    }

    pub(in crate::tui) fn sync_rendered_page_state(&mut self, snapshot: &Snapshot) {
        match self.page {
            Page::Sessions => self.sync_sessions_page_selection(snapshot),
            Page::Requests => self.sync_request_page_selection(snapshot),
            Page::History => self.sync_codex_history_selection(),
            Page::Recent => self.sync_codex_recent_selection(now_ms()),
            Page::Fleet => self.sync_fleet_selection(),
            _ => {}
        }
    }

    pub(in crate::tui) fn sync_fleet_selection(&mut self) {
        let Some(snapshot) = self.fleet_snapshot.as_ref() else {
            self.selected_fleet_node_idx = 0;
            self.selected_fleet_unit_idx = 0;
            self.selected_fleet_node_id = None;
            self.selected_fleet_unit_id = None;
            clamp_table_selection(&mut self.fleet_nodes_table, None, 0);
            clamp_table_selection(&mut self.fleet_units_table, None, 0);
            return;
        };

        let node_len = snapshot.nodes.len();
        let selected_node_idx = self
            .selected_fleet_node_id
            .as_deref()
            .and_then(|node_id| {
                snapshot
                    .nodes
                    .iter()
                    .position(|node| node.node_id == node_id)
            })
            .unwrap_or(self.selected_fleet_node_idx.min(node_len.saturating_sub(1)));

        self.selected_fleet_node_idx = clamp_table_selection(
            &mut self.fleet_nodes_table,
            Some(selected_node_idx),
            node_len,
        )
        .unwrap_or(0);
        self.selected_fleet_node_id = snapshot
            .nodes
            .get(self.selected_fleet_node_idx)
            .map(|node| node.node_id.clone());

        let unit_len = snapshot
            .nodes
            .get(self.selected_fleet_node_idx)
            .map(|node| node.work_units.len())
            .unwrap_or(0);
        let selected_unit_idx = self
            .selected_fleet_unit_id
            .as_deref()
            .and_then(|unit_id| {
                snapshot
                    .nodes
                    .get(self.selected_fleet_node_idx)?
                    .work_units
                    .iter()
                    .position(|unit| unit.id == unit_id)
            })
            .unwrap_or(self.selected_fleet_unit_idx.min(unit_len.saturating_sub(1)));

        self.selected_fleet_unit_idx = clamp_table_selection(
            &mut self.fleet_units_table,
            Some(selected_unit_idx),
            unit_len,
        )
        .unwrap_or(0);
        self.selected_fleet_unit_id = snapshot
            .nodes
            .get(self.selected_fleet_node_idx)
            .and_then(|node| node.work_units.get(self.selected_fleet_unit_idx))
            .map(|unit| unit.id.clone());
    }

    pub(in crate::tui) fn filtered_sessions_page_indices(&self, snapshot: &Snapshot) -> Vec<usize> {
        snapshot
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                if self.sessions_page_active_only && row.active_count == 0 {
                    return false;
                }
                if self.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                    return false;
                }
                if self.sessions_page_overrides_only && !row.binding.has_manual_values() {
                    return false;
                }
                true
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    pub(in crate::tui) fn sync_sessions_page_selection(&mut self, snapshot: &Snapshot) {
        let visible = self.filtered_sessions_page_indices(snapshot);
        let selected_idx = self
            .selected_session_id
            .as_deref()
            .and_then(|sid| {
                visible.iter().position(|row_idx| {
                    snapshot
                        .rows
                        .get(*row_idx)
                        .and_then(|row| row.session_id.as_deref())
                        == Some(sid)
                })
            })
            .unwrap_or(
                self.selected_sessions_page_idx
                    .min(visible.len().saturating_sub(1)),
            );

        self.selected_sessions_page_idx = clamp_table_selection(
            &mut self.sessions_page_table,
            Some(selected_idx),
            visible.len(),
        )
        .unwrap_or(0);

        if let Some(row_idx) = visible.get(self.selected_sessions_page_idx).copied() {
            self.selected_session_idx = row_idx;
            self.selected_session_id = snapshot
                .rows
                .get(row_idx)
                .and_then(|row| row.session_id.clone());
        }
    }

    pub(in crate::tui) fn request_page_filtered_indices(&self, snapshot: &Snapshot) -> Vec<usize> {
        let focused_sid = request_page_focus_session_id(
            snapshot,
            self.focused_request_session_id.as_deref(),
            self.selected_session_idx,
        );
        snapshot
            .recent
            .iter()
            .enumerate()
            .filter(|(_, request)| {
                request_matches_page_filters(
                    request,
                    snapshot.request_control_evidence.get(&request.id),
                    self.request_page_errors_only,
                    self.request_page_scope_session,
                    focused_sid.as_deref(),
                    self.request_page_control_filter,
                )
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    pub(in crate::tui) fn sync_dashboard_request_selection(&mut self, snapshot: &Snapshot) {
        let visible = dashboard_request_filtered_indices(snapshot, self.selected_session_idx);
        let previous_id = self.selected_request_id;
        let selected_idx = previous_id
            .and_then(|request_id| {
                visible.iter().position(|request_idx| {
                    snapshot.recent.get(*request_idx).map(|request| request.id) == Some(request_id)
                })
            })
            .unwrap_or(
                self.selected_request_idx
                    .min(visible.len().saturating_sub(1)),
            );
        self.selected_request_idx =
            clamp_table_selection(&mut self.requests_table, Some(selected_idx), visible.len())
                .unwrap_or(0);
        self.selected_request_id = visible
            .get(self.selected_request_idx)
            .and_then(|request_idx| snapshot.recent.get(*request_idx))
            .map(|request| request.id);
    }

    pub(in crate::tui) fn select_dashboard_request_index(
        &mut self,
        snapshot: &Snapshot,
        index: usize,
    ) -> bool {
        let visible = dashboard_request_filtered_indices(snapshot, self.selected_session_idx);
        let Some(request) = visible
            .get(index)
            .and_then(|request_idx| snapshot.recent.get(*request_idx))
        else {
            return false;
        };
        self.selected_request_idx = index;
        self.selected_request_id = Some(request.id);
        self.requests_table.select(Some(index));
        true
    }

    pub(in crate::tui) fn sync_request_page_selection(&mut self, snapshot: &Snapshot) {
        let visible = self.request_page_filtered_indices(snapshot);
        let previous_id = self.selected_request_page_id;
        let selected_idx = previous_id
            .and_then(|request_id| {
                visible.iter().position(|request_idx| {
                    snapshot.recent.get(*request_idx).map(|request| request.id) == Some(request_id)
                })
            })
            .unwrap_or(
                self.selected_request_page_idx
                    .min(visible.len().saturating_sub(1)),
            );
        self.selected_request_page_idx = clamp_table_selection(
            &mut self.request_page_table,
            Some(selected_idx),
            visible.len(),
        )
        .unwrap_or(0);
        self.selected_request_page_id = visible
            .get(self.selected_request_page_idx)
            .and_then(|request_idx| snapshot.recent.get(*request_idx))
            .map(|request| request.id);
        if self.selected_request_page_id != previous_id {
            self.requests_details_scroll = 0;
        }
    }

    pub(in crate::tui) fn select_request_page_index(
        &mut self,
        snapshot: &Snapshot,
        index: usize,
    ) -> bool {
        let visible = self.request_page_filtered_indices(snapshot);
        let Some(request) = visible
            .get(index)
            .and_then(|request_idx| snapshot.recent.get(*request_idx))
        else {
            return false;
        };
        self.selected_request_page_idx = index;
        self.selected_request_page_id = Some(request.id);
        self.request_page_table.select(Some(index));
        self.requests_details_scroll = 0;
        true
    }

    pub(in crate::tui) fn sync_codex_history_selection(&mut self) {
        let len = self.codex_history_visible_len();
        let selected_idx = self
            .selected_codex_history_id
            .as_deref()
            .and_then(|sid| {
                self.codex_history_sessions
                    .iter()
                    .position(|summary| summary.id == sid)
            })
            .unwrap_or(self.selected_codex_history_idx.min(len.saturating_sub(1)));

        self.selected_codex_history_idx = selected_idx;
        self.selected_codex_history_id = self
            .codex_history_sessions
            .get(self.selected_codex_history_idx)
            .map(|summary| summary.id.clone());
        self.selected_codex_history_idx = clamp_table_selection(
            &mut self.codex_history_table,
            Some(self.selected_codex_history_idx),
            len,
        )
        .unwrap_or(0);
    }

    pub(in crate::tui) fn codex_history_visible_len(&self) -> usize {
        self.codex_history_sessions.len()
    }

    pub(in crate::tui) fn codex_recent_visible_indices(&self, now_ms: u64) -> Vec<usize> {
        let threshold_ms = codex_recent_window_threshold_ms(now_ms, self.codex_recent_window_idx);
        self.codex_recent_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.mtime_ms >= threshold_ms)
            .map(|(idx, _)| idx)
            .collect()
    }

    pub(in crate::tui) fn sync_codex_recent_selection(&mut self, now_ms: u64) {
        let visible = self.codex_recent_visible_indices(now_ms);
        let selected_idx = self
            .codex_recent_selected_id
            .as_deref()
            .and_then(|sid| {
                visible.iter().position(|row_idx| {
                    self.codex_recent_rows
                        .get(*row_idx)
                        .map(|row| row.session_id.as_str())
                        == Some(sid)
                })
            })
            .unwrap_or(
                self.codex_recent_selected_idx
                    .min(visible.len().saturating_sub(1)),
            );

        self.codex_recent_selected_idx = clamp_table_selection(
            &mut self.codex_recent_table,
            Some(selected_idx),
            visible.len(),
        )
        .unwrap_or(0);
        self.codex_recent_selected_id = visible
            .get(self.codex_recent_selected_idx)
            .and_then(|idx| self.codex_recent_rows.get(*idx))
            .map(|row| row.session_id.clone());
    }

    pub(in crate::tui) fn prepare_codex_history_external_focus(
        &mut self,
        summary: SessionSummary,
        origin: CodexHistoryExternalFocusOrigin,
    ) {
        let sid = summary.id.clone();
        self.codex_history_external_focus = Some(CodexHistoryExternalFocus { summary, origin });
        if let Some(focus) = self.codex_history_external_focus.as_ref() {
            merge_codex_history_external_focus(&mut self.codex_history_sessions, focus);
        }
        self.selected_codex_history_idx = 0;
        self.selected_codex_history_id = Some(sid);
        self.codex_history_details_scroll = 0;
        self.sync_codex_history_selection();
    }
}

fn service_status_row_keys(snapshot: &Snapshot) -> Vec<(String, Option<String>)> {
    snapshot
        .service_status
        .iter()
        .flat_map(|status| &status.probes)
        .flat_map(|probe| {
            if probe.services.is_empty() {
                vec![(probe.id.clone(), None)]
            } else {
                probe
                    .services
                    .iter()
                    .map(|service| (probe.id.clone(), Some(service.model.clone())))
                    .collect()
            }
        })
        .collect()
}

fn clamp_table_selection(
    table: &mut TableState,
    selected: Option<usize>,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        table.select(None);
        *table.offset_mut() = 0;
        return None;
    }

    let selected = selected.unwrap_or(0).min(len - 1);
    table.select(Some(selected));
    *table.offset_mut() = table.offset().min(len - 1).min(selected);
    Some(selected)
}

fn clamp_table_viewport(
    table: &mut TableState,
    selected: Option<usize>,
    len: usize,
    visible_rows: usize,
) -> Option<usize> {
    let selected = clamp_table_selection(table, selected, len)?;
    if visible_rows == 0 {
        *table.offset_mut() = selected;
        return Some(selected);
    }

    let visible_rows = visible_rows.min(len);
    let max_offset = len.saturating_sub(visible_rows);
    let mut offset = table.offset().min(max_offset);

    if selected < offset {
        offset = selected;
    } else {
        let end_exclusive = offset.saturating_add(visible_rows);
        if selected >= end_exclusive {
            offset = selected.saturating_add(1).saturating_sub(visible_rows);
        }
    }

    *table.offset_mut() = offset.min(max_offset);
    Some(selected)
}

pub(in crate::tui) fn merge_codex_history_external_focus(
    list: &mut Vec<SessionSummary>,
    focus: &CodexHistoryExternalFocus,
) {
    let merged = list
        .iter()
        .find(|summary| {
            summary.id == focus.summary.id
                && (!summary.path.as_os_str().is_empty()
                    || summary.source == SessionSummarySource::LocalFile)
        })
        .cloned()
        .unwrap_or_else(|| focus.summary.clone());

    list.retain(|summary| summary.id != merged.id);
    list.insert(0, merged);
}

pub(in crate::tui) fn adjust_table_selection(
    table: &mut TableState,
    delta: i32,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        return clamp_table_selection(table, None, len);
    }
    let cur = table.selected().unwrap_or(0);
    let next = if delta.is_negative() {
        cur.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (cur + delta as usize).min(len - 1)
    };
    clamp_table_selection(table, Some(next), len)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn local_codex_switch_is_available_for_integrated_and_local_attached_modes_only() {
        for runtime_connection in [
            RuntimeConnectionKind::Integrated,
            RuntimeConnectionKind::LocalAttached,
        ] {
            let ui = UiState {
                service_name: "codex",
                runtime_connection,
                ..UiState::default()
            };
            assert!(ui.allows_local_codex_switch());
        }

        let remote = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };
        assert!(!remote.allows_local_codex_switch());
    }

    #[test]
    fn runtime_to_local_codex_bridge_requires_codex_and_local_session_metadata() {
        let integrated = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::Integrated,
            ..UiState::default()
        };
        assert!(integrated.can_bridge_runtime_sessions_to_local_codex());

        let local_attached = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::LocalAttached,
            local_operator_transport_available: true,
            ..UiState::default()
        };
        assert!(local_attached.can_bridge_runtime_sessions_to_local_codex());

        for ui in [
            UiState {
                service_name: "codex",
                runtime_connection: RuntimeConnectionKind::LocalAttached,
                local_operator_transport_available: false,
                ..UiState::default()
            },
            UiState {
                service_name: "codex",
                runtime_connection: RuntimeConnectionKind::RemoteObserver,
                ..UiState::default()
            },
            UiState {
                service_name: "claude",
                runtime_connection: RuntimeConnectionKind::Integrated,
                ..UiState::default()
            },
        ] {
            assert!(!ui.can_bridge_runtime_sessions_to_local_codex());
        }
    }
    use crate::dashboard_core::{
        OperatorRequestObservability, OperatorRequestSummary, OperatorRouteCandidateSummary,
        OperatorRoutingSummary,
    };
    use crate::quota_analytics::{
        PoolQuotaAnalytics, QuotaAnalyticsSupport, QuotaProjectRow, QuotaReconciliationView,
    };
    use crate::quota_pool::{PoolIdentity, QuotaQuantity, QuotaUnit};
    use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
    use crate::state::{
        BalanceSnapshotStatus, ProviderBalanceSnapshot, SessionObservationScope, UsageBucket,
        UsageRollupView,
    };
    use crate::tui::model::SessionRow;

    fn sample_summary(id: &str, path: &str, source: SessionSummarySource) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            path: PathBuf::from(path),
            cwd: None,
            created_at: None,
            updated_at: None,
            last_response_at: None,
            user_turns: 0,
            assistant_turns: 0,
            rounds: 0,
            first_user_message: None,
            source,
            sort_hint_ms: None,
        }
    }

    fn sample_usage_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            quota_analytics: crate::quota_analytics::QuotaAnalyticsView::default(),
            usage_rollup: UsageRollupView {
                by_provider: vec![(
                    "stale-provider".to_string(),
                    UsageBucket {
                        requests_total: 2,
                        ..UsageBucket::default()
                    },
                )],
                ..UsageRollupView::default()
            },
            provider_balances: HashMap::from([(
                "stale-provider".to_string(),
                vec![ProviderBalanceSnapshot {
                    observation_provider_id: "stale-observer".to_string(),
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex",
                        "stale-provider",
                        "endpoint-7",
                    ),
                    status: BalanceSnapshotStatus::Stale,
                    error: Some("refresh failed".to_string()),
                    ..ProviderBalanceSnapshot::default()
                }],
            )]),
            routing: None,
            pricing_catalog: Default::default(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            refreshed_at: std::time::Instant::now(),
        }
    }

    fn quota_pool(key: &str, project_path: &str) -> PoolQuotaAnalytics {
        PoolQuotaAnalytics {
            identity: PoolIdentity {
                key: key.to_string(),
                ..PoolIdentity::default()
            },
            reconciliation: QuotaReconciliationView {
                projects: vec![QuotaProjectRow {
                    project: ProjectIdentity {
                        kind: ProjectIdentityKind::GitRoot,
                        path: Some(project_path.to_string()),
                    },
                    local_cost: QuotaQuantity::from_integer(1, QuotaUnit::Usd),
                    requests: 1,
                }],
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        }
    }

    #[test]
    fn stats_selection_restores_stable_pool_and_project_keys_without_sessions() {
        let mut snapshot = sample_usage_snapshot();
        snapshot.quota_analytics.support = QuotaAnalyticsSupport::Supported;
        snapshot.quota_analytics.pools = vec![
            quota_pool("pool-a", "C:/src/a"),
            quota_pool("pool-b", "C:/src/b"),
        ];
        let mut ui = UiState {
            selected_stats_pool_key: Some("pool-b".to_string()),
            selected_stats_project_key: Some("C:/src/b".to_string()),
            ..UiState::default()
        };

        ui.clamp_selection(&snapshot, 0);

        assert_eq!(ui.selected_stats_pool_idx, 1);
        assert_eq!(ui.selected_stats_project_idx, 0);
        assert_eq!(ui.stats_pools_table.selected(), Some(1));
        assert_eq!(ui.stats_projects_table.selected(), Some(0));

        snapshot.quota_analytics.pools.swap(0, 1);
        ui.clamp_selection(&snapshot, 0);
        assert_eq!(ui.selected_stats_pool_idx, 0);
        assert_eq!(ui.selected_stats_pool_key.as_deref(), Some("pool-b"));
    }

    #[test]
    fn stats_focus_cycles_and_pool_move_resets_project_selection() {
        let mut snapshot = sample_usage_snapshot();
        snapshot.quota_analytics.support = QuotaAnalyticsSupport::Supported;
        snapshot.quota_analytics.pools = vec![
            quota_pool("pool-a", "C:/src/a"),
            quota_pool("pool-b", "C:/src/b"),
        ];
        let mut ui = UiState::default();
        ui.clamp_selection(&snapshot, 0);

        assert_eq!(ui.stats_focus, StatsFocus::Pools);
        assert_eq!(ui.cycle_stats_focus(), StatsFocus::Projects);
        assert_eq!(ui.cycle_stats_focus(), StatsFocus::Providers);
        assert_eq!(ui.cycle_stats_focus(), StatsFocus::ProviderEndpoints);
        assert_eq!(ui.cycle_stats_focus(), StatsFocus::Pools);

        ui.selected_stats_project_idx = 4;
        ui.selected_stats_project_key = Some("old".to_string());
        assert!(ui.move_stats_selection(&snapshot, 1));
        assert_eq!(ui.selected_stats_pool_idx, 1);
        assert_eq!(ui.selected_stats_project_idx, 0);
        assert_eq!(ui.selected_stats_project_key, None);
    }

    #[test]
    fn stats_project_selection_reaches_omitted_and_reconciliation_rows() {
        let mut snapshot = sample_usage_snapshot();
        let mut pool = quota_pool("pool-a", "C:/src/a");
        pool.reconciliation.omitted_projects = 3;
        pool.reconciliation.omitted_local_known =
            Some(QuotaQuantity::from_integer(4, QuotaUnit::Usd));
        pool.reconciliation.local_unknown = Some(QuotaQuantity::from_integer(1, QuotaUnit::Usd));
        pool.reconciliation.external_unattributed =
            Some(QuotaQuantity::from_integer(2, QuotaUnit::Usd));
        pool.reconciliation.signed_delta = Some(
            crate::quota_analytics::SignedUsdDelta::from_femto_usd(2 * 10_i128.pow(15)),
        );
        snapshot.quota_analytics.support = QuotaAnalyticsSupport::Supported;
        snapshot.quota_analytics.pools = vec![pool];
        let mut ui = UiState {
            stats_focus: StatsFocus::Projects,
            ..UiState::default()
        };
        ui.clamp_selection(&snapshot, 0);

        for _ in 0..4 {
            assert!(ui.move_stats_selection(&snapshot, 1));
        }

        assert_eq!(ui.selected_stats_project_idx, 4);
        assert_eq!(
            ui.selected_stats_project_key.as_deref(),
            Some(STATS_PROJECT_GAP_KEY)
        );
        assert_eq!(ui.stats_projects_table.selected(), Some(4));
    }

    fn empty_session_row(id: &str) -> SessionRow {
        SessionRow {
            session_id: Some(id.to_string()),
            local_session_id: None,
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
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: None,
            avg_output_tokens_per_second: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            binding: crate::state::SessionBindingProjection::default(),
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
        }
    }

    fn operator_request(
        id: u64,
        session_id: Option<&str>,
        status_code: u16,
    ) -> OperatorRequestSummary {
        OperatorRequestSummary {
            id,
            trace_key: None,
            session_key: session_id.map(ToOwned::to_owned),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            route_path: Vec::new(),
            upstream_origin: None,
            usage: None,
            cache_accounting_convention: Default::default(),
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: OperatorRequestObservability {
                duration_ms: Some(10),
                ttfb_ms: None,
                generation_ms: None,
                output_tokens_per_second: None,
                attempt_count: 1,
                route_attempt_count: 0,
                retried: false,
                cross_provider_failover: false,
                same_provider_retry: false,
                fast_mode: false,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
        }
    }

    fn service_status_snapshot(
        rows: &[(&str, &str)],
    ) -> codex_helper_core::service_status::ServiceStatusSnapshot {
        use codex_helper_core::service_status::{
            ServiceStatusKind, ServiceStatusProbeSnapshot, ServiceStatusServiceSnapshot,
            ServiceStatusSnapshot,
        };

        ServiceStatusSnapshot {
            generated_at_ms: 1,
            configured: true,
            enabled: true,
            refresh_interval_secs: 60,
            history_cells: 60,
            probes: rows
                .iter()
                .map(|(probe_id, model)| ServiceStatusProbeSnapshot {
                    id: (*probe_id).to_string(),
                    url: format!("https://{probe_id}.example"),
                    fetched_at_ms: 1,
                    generated_at_ms: None,
                    all_ok: Some(true),
                    services: vec![ServiceStatusServiceSnapshot {
                        model: (*model).to_string(),
                        uptime_pct: Some("100%".to_string()),
                        latest_kind: ServiceStatusKind::Ok,
                        latest: None,
                        history: Vec::new(),
                    }],
                    credential_readiness: None,
                    credential_details: Vec::new(),
                    error: None,
                })
                .collect(),
            error: None,
        }
    }

    #[test]
    fn merge_codex_history_external_focus_keeps_local_file_summary() {
        let mut list = vec![sample_summary(
            "sid-1",
            "local.jsonl",
            SessionSummarySource::LocalFile,
        )];
        let focus = CodexHistoryExternalFocus {
            summary: sample_summary("sid-1", "", SessionSummarySource::ObservedOnly),
            origin: CodexHistoryExternalFocusOrigin::Requests,
        };

        merge_codex_history_external_focus(&mut list, &focus);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "sid-1");
        assert_eq!(list[0].path, PathBuf::from("local.jsonl"));
        assert_eq!(list[0].source, SessionSummarySource::LocalFile);
    }

    #[test]
    fn sync_codex_history_selection_prefers_selected_id() {
        let mut ui = UiState {
            codex_history_sessions: vec![
                sample_summary("sid-a", "a.jsonl", SessionSummarySource::LocalFile),
                sample_summary("sid-b", "b.jsonl", SessionSummarySource::LocalFile),
            ],
            selected_codex_history_idx: 0,
            selected_codex_history_id: Some("sid-b".to_string()),
            ..Default::default()
        };

        ui.sync_codex_history_selection();

        assert_eq!(ui.selected_codex_history_idx, 1);
        assert_eq!(ui.selected_codex_history_id.as_deref(), Some("sid-b"));
        assert_eq!(ui.codex_history_table.selected(), Some(1));
    }

    #[test]
    fn sync_codex_recent_selection_uses_visible_window_and_clears_hidden_id() {
        let now = 3_600_001;
        let mut ui = UiState {
            codex_recent_rows: vec![
                RecentCodexRow {
                    root: "old-root".to_string(),
                    branch: None,
                    session_id: "sid-old".to_string(),
                    cwd: None,
                    mtime_ms: 0,
                },
                RecentCodexRow {
                    root: "new-root".to_string(),
                    branch: None,
                    session_id: "sid-new".to_string(),
                    cwd: None,
                    mtime_ms: now,
                },
            ],
            codex_recent_window_idx: 0,
            codex_recent_selected_idx: 9,
            codex_recent_selected_id: Some("sid-old".to_string()),
            ..Default::default()
        };

        ui.sync_codex_recent_selection(now);

        assert_eq!(ui.codex_recent_selected_idx, 0);
        assert_eq!(ui.codex_recent_selected_id.as_deref(), Some("sid-new"));
        assert_eq!(ui.codex_recent_table.selected(), Some(0));
    }

    #[test]
    fn codex_recent_visible_indices_do_not_hide_rows_after_three_hundred() {
        let now = now_ms();
        let ui = UiState {
            codex_recent_rows: (0..350)
                .map(|index| RecentCodexRow {
                    root: format!("root-{index}"),
                    branch: None,
                    session_id: format!("session-{index}"),
                    cwd: None,
                    mtime_ms: now,
                })
                .collect(),
            ..UiState::default()
        };

        let visible = ui.codex_recent_visible_indices(now);

        assert_eq!(visible.len(), 350);
        assert_eq!(visible.last(), Some(&349));
    }

    #[test]
    fn sync_sessions_page_selection_updates_global_selection_after_filter() {
        let mut inactive = empty_session_row("sid-inactive");
        inactive.active_count = 0;
        let mut active = empty_session_row("sid-active");
        active.active_count = 1;
        let snapshot = Snapshot {
            rows: vec![inactive, active],
            ..sample_usage_snapshot()
        };
        let mut ui = UiState {
            sessions_page_active_only: true,
            selected_session_idx: 0,
            selected_session_id: Some("sid-inactive".to_string()),
            selected_sessions_page_idx: 8,
            ..Default::default()
        };

        ui.sync_sessions_page_selection(&snapshot);

        assert_eq!(ui.selected_sessions_page_idx, 0);
        assert_eq!(ui.selected_session_idx, 1);
        assert_eq!(ui.selected_session_id.as_deref(), Some("sid-active"));
        assert_eq!(ui.sessions_page_table.selected(), Some(0));
    }

    #[test]
    fn sessions_page_keeps_all_operator_rows_reachable() {
        let snapshot = Snapshot {
            rows: (0..250)
                .map(|index| empty_session_row(&format!("sid-{index:03}")))
                .collect(),
            ..sample_usage_snapshot()
        };
        let mut ui = UiState {
            selected_session_id: Some("sid-249".to_string()),
            ..UiState::default()
        };

        assert_eq!(ui.filtered_sessions_page_indices(&snapshot).len(), 250);
        ui.sync_sessions_page_selection(&snapshot);

        assert_eq!(ui.selected_sessions_page_idx, 249);
        assert_eq!(ui.selected_session_idx, 249);
        assert_eq!(ui.selected_session_id.as_deref(), Some("sid-249"));
        assert_eq!(ui.sessions_page_table.selected(), Some(249));
    }

    #[test]
    fn session_manual_control_filter_uses_binding_projection() {
        let automatic = empty_session_row("sid-automatic");
        let mut manual = empty_session_row("sid-manual");
        manual.binding = crate::state::SessionBindingProjection {
            revision: "binding:v1:manual".to_string(),
            reasoning_effort: Some("high".to_string()),
            continuity_mode: Some(crate::state::SessionContinuityMode::ManualProfile),
            ..crate::state::SessionBindingProjection::default()
        };
        let snapshot = Snapshot {
            rows: vec![automatic, manual],
            ..sample_usage_snapshot()
        };
        let ui = UiState {
            sessions_page_overrides_only: true,
            ..UiState::default()
        };

        assert_eq!(ui.filtered_sessions_page_indices(&snapshot), vec![1]);
    }

    #[test]
    fn sync_request_page_selection_clamps_filtered_selection() {
        let snapshot = Snapshot {
            recent: vec![
                operator_request(1, Some("sid"), 200),
                operator_request(2, Some("sid"), 500),
            ],
            ..sample_usage_snapshot()
        };
        let mut ui = UiState {
            request_page_errors_only: true,
            selected_request_page_idx: 7,
            ..Default::default()
        };

        ui.sync_request_page_selection(&snapshot);

        assert_eq!(ui.selected_request_page_idx, 0);
        assert_eq!(ui.request_page_table.selected(), Some(0));
    }

    #[test]
    fn request_selections_follow_request_identity_across_prepend_refresh() {
        let mut snapshot = Snapshot {
            rows: vec![empty_session_row("sid")],
            recent: vec![
                operator_request(3, Some("sid"), 200),
                operator_request(2, Some("sid"), 200),
                operator_request(1, Some("sid"), 200),
            ],
            ..sample_usage_snapshot()
        };
        let mut ui = UiState {
            selected_session_id: Some("sid".to_string()),
            selected_request_idx: 1,
            selected_request_id: Some(2),
            selected_request_page_idx: 1,
            selected_request_page_id: Some(2),
            ..UiState::default()
        };

        ui.clamp_selection(&snapshot, 0);
        ui.sync_request_page_selection(&snapshot);
        snapshot
            .recent
            .insert(0, operator_request(4, Some("sid"), 200));
        ui.clamp_selection(&snapshot, 0);
        ui.sync_request_page_selection(&snapshot);

        assert_eq!(ui.selected_request_idx, 2);
        assert_eq!(ui.selected_request_id, Some(2));
        assert_eq!(ui.requests_table.selected(), Some(2));
        assert_eq!(ui.selected_request_page_idx, 2);
        assert_eq!(ui.selected_request_page_id, Some(2));
        assert_eq!(ui.request_page_table.selected(), Some(2));
    }

    #[test]
    fn dashboard_requests_keep_the_bounded_operator_tail_reachable() {
        let snapshot = Snapshot {
            rows: vec![empty_session_row("sid")],
            recent: (1..=75)
                .map(|id| operator_request(id, Some("sid"), 200))
                .collect(),
            ..sample_usage_snapshot()
        };
        let mut ui = UiState {
            selected_session_id: Some("sid".to_string()),
            selected_request_idx: 59,
            selected_request_id: Some(75),
            ..UiState::default()
        };

        ui.clamp_selection(&snapshot, 0);

        assert_eq!(dashboard_request_filtered_indices(&snapshot, 0).len(), 75);
        assert_eq!(ui.selected_request_idx, 74);
        assert_eq!(ui.selected_request_id, Some(75));
        assert_eq!(ui.requests_table.selected(), Some(74));
    }

    #[test]
    fn service_status_selection_follows_probe_identity_across_reordering() {
        let before = Snapshot {
            service_status: Some(service_status_snapshot(&[
                ("provider-a", "model-a"),
                ("provider-b", "model-b"),
            ])),
            ..sample_usage_snapshot()
        };
        let after = Snapshot {
            service_status: Some(service_status_snapshot(&[
                ("provider-b", "model-b"),
                ("provider-a", "model-a"),
            ])),
            ..sample_usage_snapshot()
        };
        let mut ui = UiState::default();

        assert!(ui.move_service_status_selection(&before, 1));
        assert_eq!(
            ui.selected_service_status_key,
            Some(("provider-b".to_string(), Some("model-b".to_string())))
        );

        ui.clamp_selection(&after, 0);

        assert_eq!(ui.selected_service_status_idx, 0);
        assert_eq!(ui.service_status_table.selected(), Some(0));
        assert_eq!(
            ui.selected_service_status_key,
            Some(("provider-b".to_string(), Some("model-b".to_string())))
        );
    }

    #[test]
    fn table_selection_clamp_resets_stale_offset() {
        let mut table = TableState::default()
            .with_offset(25)
            .with_selected(Some(25));

        let selected = clamp_table_selection(&mut table, Some(25), 3);

        assert_eq!(selected, Some(2));
        assert_eq!(table.selected(), Some(2));
        assert_eq!(table.offset(), 2);

        let selected = clamp_table_selection(&mut table, Some(2), 0);

        assert_eq!(selected, None);
        assert_eq!(table.selected(), None);
        assert_eq!(table.offset(), 0);
    }

    #[test]
    fn table_viewport_scrolls_down_to_keep_selection_visible() {
        let mut table = TableState::default().with_offset(0).with_selected(Some(0));

        let selected = clamp_table_viewport(&mut table, Some(8), 20, 5);

        assert_eq!(selected, Some(8));
        assert_eq!(table.selected(), Some(8));
        assert_eq!(table.offset(), 4);
    }

    #[test]
    fn table_viewport_scrolls_up_to_keep_selection_visible() {
        let mut table = TableState::default()
            .with_offset(10)
            .with_selected(Some(10));

        let selected = clamp_table_viewport(&mut table, Some(7), 20, 5);

        assert_eq!(selected, Some(7));
        assert_eq!(table.selected(), Some(7));
        assert_eq!(table.offset(), 7);
    }

    #[test]
    fn table_viewport_clamps_offset_when_list_shrinks() {
        let mut table = TableState::default()
            .with_offset(12)
            .with_selected(Some(12));

        let selected = clamp_table_viewport(&mut table, Some(12), 8, 5);

        assert_eq!(selected, Some(7));
        assert_eq!(table.selected(), Some(7));
        assert_eq!(table.offset(), 3);
    }

    #[test]
    fn reset_table_viewports_keeps_selection_but_clears_offsets() {
        let mut ui = UiState {
            providers_table: TableState::default().with_offset(8).with_selected(Some(9)),
            sessions_table: TableState::default().with_offset(3).with_selected(Some(4)),
            ..UiState::default()
        };

        ui.reset_table_viewports();

        assert_eq!(ui.providers_table.selected(), Some(9));
        assert_eq!(ui.providers_table.offset(), 0);
        assert_eq!(ui.sessions_table.selected(), Some(4));
        assert_eq!(ui.sessions_table.offset(), 0);
    }

    fn routing_snapshot_with_candidates(candidates: &[(&str, &str)]) -> Snapshot {
        Snapshot {
            routing: Some(OperatorRoutingSummary {
                route_graph_key: "routing:sha256:test".to_string(),
                control_revision: 0,
                provider_policy_revision: 0,
                entry: "main".to_string(),
                entry_strategy: crate::config::RouteStrategy::RoundRobin,
                entry_target: None,
                new_session_preference: None,
                affinity_policy: crate::config::RouteAffinityPolicy::FallbackSticky,
                scheduling_preset: crate::config::SchedulingPreset::Balanced,
                fallback_ttl_ms: None,
                reprobe_preferred_after_ms: None,
                candidates: candidates
                    .iter()
                    .enumerate()
                    .map(|(route_order, (provider_id, endpoint_id))| {
                        OperatorRouteCandidateSummary {
                            route_order,
                            provider_id: (*provider_id).to_string(),
                            endpoint_id: (*endpoint_id).to_string(),
                            preference_group: 0,
                            route_path: Vec::new(),
                        }
                    })
                    .collect(),
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn routing_selection_follows_endpoint_identity_across_reordering() {
        let before = routing_snapshot_with_candidates(&[("input", "a"), ("ciii", "b")]);
        let after = routing_snapshot_with_candidates(&[("ciii", "b"), ("input", "a")]);
        let mut ui = UiState::default();

        assert!(ui.move_routing_selection(&before, 1));
        assert_eq!(
            ui.selected_routing_candidate_key,
            Some(("ciii".to_string(), "b".to_string()))
        );

        ui.clamp_selection(&after, 0);

        assert_eq!(ui.selected_routing_candidate_idx, 0);
        assert_eq!(
            ui.selected_routing_candidate_key,
            Some(("ciii".to_string(), "b".to_string()))
        );
    }

    #[test]
    fn routing_selection_clamps_when_selected_endpoint_disappears() {
        let before = routing_snapshot_with_candidates(&[("input", "a"), ("ciii", "b")]);
        let after = routing_snapshot_with_candidates(&[("input", "a")]);
        let mut ui = UiState::default();

        assert!(ui.move_routing_selection(&before, 1));
        ui.clamp_selection(&after, 0);

        assert_eq!(ui.selected_routing_candidate_idx, 0);
        assert_eq!(
            ui.selected_routing_candidate_key,
            Some(("input".to_string(), "a".to_string()))
        );
    }
}
