use ratatui::widgets::{ListState, TableState};

use crate::codex_integration::CodexStartupReadiness;
use crate::config::{
    FleetRegistryConfig, ResolvedRetryConfig, UsageForecastConfig,
    is_supported_route_graph_config_version,
};
use crate::dashboard_core::ControlProfileOption;
use crate::routing_explain::RoutingExplainResponse;
use crate::sessions::{
    SessionMeta, SessionSummary, SessionSummarySource, SessionTranscriptMessage,
};
#[cfg(test)]
use crate::usage_balance::{
    UsageBalanceBuildInput, UsageBalanceEndpointRow, UsageBalanceProviderRow,
    UsageBalanceRefreshInput, UsageBalanceView,
};
use crate::usage_providers::UsageProviderRefreshSummary;
use codex_helper_core::fleet::FleetSnapshot;
use std::collections::{BTreeMap, HashMap};

use super::Language;
use super::model::{
    RoutingSpecView, Snapshot, codex_recent_window_threshold_ms, filtered_requests_len, now_ms,
    request_matches_page_filters, request_page_focus_session_id, routing_provider_names,
    session_row_has_any_override,
};
use super::types::{Focus, Overlay, Page, StatsFocus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct RoutingProviderRow {
    pub(in crate::tui) name: String,
    pub(in crate::tui) alias: Option<String>,
    pub(in crate::tui) enabled: bool,
    pub(in crate::tui) tags: BTreeMap<String, String>,
    pub(in crate::tui) in_catalog: bool,
}

impl RoutingProviderRow {
    pub(in crate::tui) fn display_label(&self) -> String {
        self.alias
            .as_deref()
            .filter(|alias| !alias.trim().is_empty() && *alias != self.name)
            .map(|alias| format!("{} ({alias})", self.name))
            .unwrap_or_else(|| self.name.clone())
    }
}

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

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct CodexRelayDiagnosticsState {
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) last_started_at: Option<std::time::Instant>,
    pub(in crate::tui) last_finished_at: Option<std::time::Instant>,
    pub(in crate::tui) last_result: Option<crate::proxy::CodexRelayCapabilitiesResponse>,
    pub(in crate::tui) last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct CodexRelayLiveSmokeState {
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) mode: Option<crate::tui::codex_relay_live_smoke::CodexRelayLiveSmokeMode>,
    pub(in crate::tui) pending_confirm:
        Option<crate::tui::codex_relay_live_smoke::CodexRelayLiveSmokeMode>,
    pub(in crate::tui) pending_confirm_at: Option<std::time::Instant>,
    pub(in crate::tui) last_started_at: Option<std::time::Instant>,
    pub(in crate::tui) last_finished_at: Option<std::time::Instant>,
    pub(in crate::tui) last_result: Option<crate::proxy::CodexRelayLiveSmokeResponse>,
    pub(in crate::tui) last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum RuntimeConnectionKind {
    Integrated,
    Attached,
}

impl RuntimeConnectionKind {
    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        match (self, lang) {
            (RuntimeConnectionKind::Integrated, Language::Zh) => "内置",
            (RuntimeConnectionKind::Integrated, Language::En) => "integrated",
            (RuntimeConnectionKind::Attached, Language::Zh) => "已附着",
            (RuntimeConnectionKind::Attached, Language::En) => "attached",
        }
    }

    pub(in crate::tui) fn is_attached(self) -> bool {
        matches!(self, RuntimeConnectionKind::Attached)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::tui) enum RequestControlFilter {
    #[default]
    All,
    AnyEvidence,
    Signals,
    Actions,
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
    pub(in crate::tui) usage_forecast: UsageForecastConfig,
    pub(in crate::tui) refresh_ms: u64,
    pub(in crate::tui) config_version: Option<u32>,
    pub(in crate::tui) runtime_connection: RuntimeConnectionKind,
    pub(in crate::tui) runtime_shutdown_available: Option<bool>,
    pub(in crate::tui) runtime_status_error: Option<String>,
    pub(in crate::tui) page: Page,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) overlay: Overlay,
    pub(in crate::tui) startup_readiness: Option<CodexStartupReadiness>,
    pub(in crate::tui) selected_station_idx: usize,
    pub(in crate::tui) selected_session_idx: usize,
    pub(in crate::tui) selected_session_id: Option<String>,
    pub(in crate::tui) selected_request_idx: usize,
    pub(in crate::tui) selected_request_page_idx: usize,
    pub(in crate::tui) focused_request_session_id: Option<String>,
    pub(in crate::tui) request_page_errors_only: bool,
    pub(in crate::tui) request_page_scope_session: bool,
    pub(in crate::tui) request_page_control_filter: RequestControlFilter,
    pub(in crate::tui) selected_sessions_page_idx: usize,
    pub(in crate::tui) sessions_page_active_only: bool,
    pub(in crate::tui) sessions_page_errors_only: bool,
    pub(in crate::tui) sessions_page_overrides_only: bool,
    pub(in crate::tui) effort_menu_idx: usize,
    pub(in crate::tui) model_menu_idx: usize,
    pub(in crate::tui) service_tier_menu_idx: usize,
    pub(in crate::tui) profile_menu_idx: usize,
    pub(in crate::tui) provider_menu_idx: usize,
    pub(in crate::tui) routing_menu_idx: usize,
    pub(in crate::tui) routing_spec: Option<RoutingSpecView>,
    pub(in crate::tui) routing_explain: Option<RoutingExplainResponse>,
    pub(in crate::tui) last_routing_control_refresh_at: Option<std::time::Instant>,
    pub(in crate::tui) fleet_registry: FleetRegistryConfig,
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
    pub(in crate::tui) session_model_options: Vec<String>,
    pub(in crate::tui) session_model_input: String,
    pub(in crate::tui) session_model_input_hint: Option<String>,
    pub(in crate::tui) session_service_tier_input: String,
    pub(in crate::tui) session_service_tier_input_hint: Option<String>,
    pub(in crate::tui) profile_options: Vec<ControlProfileOption>,
    pub(in crate::tui) configured_default_profile: Option<String>,
    pub(in crate::tui) effective_default_profile: Option<String>,
    pub(in crate::tui) runtime_default_profile_override: Option<String>,
    pub(in crate::tui) stats_focus: StatsFocus,
    pub(in crate::tui) stats_days: usize,
    pub(in crate::tui) stats_errors_only: bool,
    pub(in crate::tui) stats_attention_only: bool,
    pub(in crate::tui) selected_stats_station_idx: usize,
    pub(in crate::tui) selected_stats_provider_idx: usize,
    pub(in crate::tui) stats_provider_detail_scroll: u16,
    pub(in crate::tui) needs_snapshot_refresh: bool,
    pub(in crate::tui) needs_config_refresh: bool,
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
    pub(in crate::tui) session_transcript_meta: Option<SessionMeta>,
    pub(in crate::tui) session_transcript_sid: Option<String>,
    pub(in crate::tui) session_transcript_file: Option<String>,
    pub(in crate::tui) session_transcript_tail: Option<usize>,
    pub(in crate::tui) session_transcript_messages: Vec<SessionTranscriptMessage>,
    pub(in crate::tui) session_transcript_scroll: u16,
    pub(in crate::tui) session_transcript_error: Option<String>,
    pub(in crate::tui) pending_overwrite_from_codex_confirm_at: Option<std::time::Instant>,
    pub(in crate::tui) last_runtime_config_loaded_at_ms: Option<u64>,
    pub(in crate::tui) last_runtime_config_source_mtime_ms: Option<u64>,
    pub(in crate::tui) last_runtime_retry: Option<ResolvedRetryConfig>,
    pub(in crate::tui) last_runtime_config_refresh_at: Option<std::time::Instant>,
    pub(in crate::tui) last_balance_refresh_requested_at: Option<std::time::Instant>,
    pub(in crate::tui) balance_refresh_in_flight: bool,
    pub(in crate::tui) last_balance_refresh_finished_at: Option<std::time::Instant>,
    pub(in crate::tui) last_balance_refresh_message: Option<String>,
    pub(in crate::tui) last_balance_refresh_error: Option<String>,
    pub(in crate::tui) last_balance_refresh_summary: Option<UsageProviderRefreshSummary>,
    pub(in crate::tui) codex_relay_diagnostics: CodexRelayDiagnosticsState,
    pub(in crate::tui) codex_relay_live_smoke: CodexRelayLiveSmokeState,
    pub(in crate::tui) should_exit: bool,
    pub(in crate::tui) stations_table: TableState,
    pub(in crate::tui) sessions_table: TableState,
    pub(in crate::tui) requests_table: TableState,
    pub(in crate::tui) request_page_table: TableState,
    pub(in crate::tui) sessions_page_table: TableState,
    pub(in crate::tui) codex_history_table: TableState,
    pub(in crate::tui) codex_recent_table: TableState,
    pub(in crate::tui) fleet_nodes_table: TableState,
    pub(in crate::tui) fleet_units_table: TableState,
    pub(in crate::tui) stats_stations_table: TableState,
    pub(in crate::tui) stats_providers_table: TableState,
    pub(in crate::tui) menu_list: ListState,
    pub(in crate::tui) station_info_scroll: u16,
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
            usage_forecast: UsageForecastConfig::default(),
            refresh_ms: 500,
            config_version: None,
            runtime_connection: RuntimeConnectionKind::Integrated,
            runtime_shutdown_available: None,
            runtime_status_error: None,
            page: Page::Dashboard,
            focus: Focus::Sessions,
            overlay: Overlay::None,
            startup_readiness: None,
            selected_station_idx: 0,
            selected_session_idx: 0,
            selected_session_id: None,
            selected_request_idx: 0,
            selected_request_page_idx: 0,
            focused_request_session_id: None,
            request_page_errors_only: false,
            request_page_scope_session: false,
            request_page_control_filter: RequestControlFilter::All,
            selected_sessions_page_idx: 0,
            sessions_page_active_only: false,
            sessions_page_errors_only: false,
            sessions_page_overrides_only: false,
            effort_menu_idx: 0,
            model_menu_idx: 0,
            service_tier_menu_idx: 0,
            profile_menu_idx: 0,
            provider_menu_idx: 0,
            routing_menu_idx: 0,
            routing_spec: None,
            routing_explain: None,
            last_routing_control_refresh_at: None,
            fleet_registry: FleetRegistryConfig::default(),
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
            session_model_options: Vec::new(),
            session_model_input: String::new(),
            session_model_input_hint: None,
            session_service_tier_input: String::new(),
            session_service_tier_input_hint: None,
            profile_options: Vec::new(),
            configured_default_profile: None,
            effective_default_profile: None,
            runtime_default_profile_override: None,
            stats_focus: StatsFocus::Providers,
            stats_days: 7,
            stats_errors_only: false,
            stats_attention_only: false,
            selected_stats_station_idx: 0,
            selected_stats_provider_idx: 0,
            stats_provider_detail_scroll: 0,
            needs_snapshot_refresh: false,
            needs_config_refresh: false,
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
            session_transcript_meta: None,
            session_transcript_sid: None,
            session_transcript_file: None,
            session_transcript_tail: Some(80),
            session_transcript_messages: Vec::new(),
            session_transcript_scroll: 0,
            session_transcript_error: None,
            pending_overwrite_from_codex_confirm_at: None,
            last_runtime_config_loaded_at_ms: None,
            last_runtime_config_source_mtime_ms: None,
            last_runtime_retry: None,
            last_runtime_config_refresh_at: None,
            last_balance_refresh_requested_at: None,
            balance_refresh_in_flight: false,
            last_balance_refresh_finished_at: None,
            last_balance_refresh_message: None,
            last_balance_refresh_error: None,
            last_balance_refresh_summary: None,
            codex_relay_diagnostics: CodexRelayDiagnosticsState::default(),
            codex_relay_live_smoke: CodexRelayLiveSmokeState::default(),
            should_exit: false,
            stations_table: TableState::default(),
            sessions_table: TableState::default(),
            requests_table: TableState::default(),
            request_page_table: TableState::default(),
            sessions_page_table: TableState::default(),
            codex_history_table: TableState::default(),
            codex_recent_table: TableState::default(),
            fleet_nodes_table: TableState::default(),
            fleet_units_table: TableState::default(),
            stats_stations_table: TableState::default(),
            stats_providers_table: TableState::default(),
            menu_list: ListState::default(),
            station_info_scroll: 0,
        }
    }
}

impl UiState {
    pub(in crate::tui) fn uses_route_graph_routing(&self) -> bool {
        self.config_version
            .is_some_and(|version| version == 3 || is_supported_route_graph_config_version(version))
    }

    pub(in crate::tui) fn station_page_rows_len(&self, legacy_len: usize) -> usize {
        if self.uses_route_graph_routing() {
            return self.routing_provider_count().unwrap_or(legacy_len);
        }
        legacy_len
    }

    pub(in crate::tui) fn clamp_selection(&mut self, snapshot: &Snapshot, providers_len: usize) {
        let station_page_rows_len = self.station_page_rows_len(providers_len);
        self.selected_station_idx = clamp_table_selection(
            &mut self.stations_table,
            Some(self.selected_station_idx),
            station_page_rows_len,
        )
        .unwrap_or(0);

        if snapshot.rows.is_empty() {
            self.selected_session_idx = 0;
            self.selected_session_id = None;
            clamp_table_selection(&mut self.sessions_table, None, 0);

            self.selected_request_idx = 0;
            clamp_table_selection(&mut self.requests_table, None, 0);
            return;
        }

        if let Some(sid) = self.selected_session_id.clone()
            && let Some(idx) = snapshot
                .rows
                .iter()
                .position(|r| r.session_id.as_deref() == Some(sid.as_str()))
        {
            self.selected_session_idx = idx;
        } else {
            self.selected_session_idx = self.selected_session_idx.min(snapshot.rows.len() - 1);
            self.selected_session_id = snapshot.rows[self.selected_session_idx].session_id.clone();
        }
        self.selected_session_idx = clamp_table_selection(
            &mut self.sessions_table,
            Some(self.selected_session_idx),
            snapshot.rows.len(),
        )
        .unwrap_or(0);

        let req_len = filtered_requests_len(snapshot, self.selected_session_idx);
        self.selected_request_idx = clamp_table_selection(
            &mut self.requests_table,
            Some(self.selected_request_idx),
            req_len,
        )
        .unwrap_or(0);

        let stats_stations_len = snapshot.usage_day.station_rows.len();
        self.selected_stats_station_idx = clamp_table_selection(
            &mut self.stats_stations_table,
            Some(self.selected_stats_station_idx),
            stats_stations_len,
        )
        .unwrap_or(0);

        let stats_providers_len = snapshot.usage_day.provider_rows.len();
        self.selected_stats_provider_idx = clamp_table_selection(
            &mut self.stats_providers_table,
            Some(self.selected_stats_provider_idx),
            stats_providers_len,
        )
        .unwrap_or(0);
        if stats_providers_len == 0 {
            self.stats_provider_detail_scroll = 0;
        }
    }

    pub(in crate::tui) fn reset_table_viewports(&mut self) {
        for table in [
            &mut self.stations_table,
            &mut self.sessions_table,
            &mut self.requests_table,
            &mut self.request_page_table,
            &mut self.sessions_page_table,
            &mut self.codex_history_table,
            &mut self.codex_recent_table,
            &mut self.fleet_nodes_table,
            &mut self.fleet_units_table,
            &mut self.stats_stations_table,
            &mut self.stats_providers_table,
        ] {
            *table.offset_mut() = 0;
        }
        self.stats_provider_detail_scroll = 0;
    }

    pub(in crate::tui) fn sync_stations_table_viewport(
        &mut self,
        providers_len: usize,
        visible_rows: usize,
    ) {
        self.selected_station_idx = clamp_table_viewport(
            &mut self.stations_table,
            Some(self.selected_station_idx),
            providers_len,
            visible_rows,
        )
        .unwrap_or(0);
    }

    pub(in crate::tui) fn routing_provider_order(&self) -> Option<Vec<String>> {
        self.routing_provider_rows()
            .map(|rows| rows.into_iter().map(|row| row.name).collect())
    }

    pub(in crate::tui) fn routing_provider_count(&self) -> Option<usize> {
        self.routing_provider_rows().map(|rows| rows.len())
    }

    pub(in crate::tui) fn routing_provider_rows(&self) -> Option<Vec<RoutingProviderRow>> {
        let spec = self.routing_spec.as_ref()?;
        let catalog = spec
            .providers
            .iter()
            .map(|provider| (provider.name.as_str(), provider))
            .collect::<HashMap<_, _>>();
        Some(
            routing_provider_names(spec)
                .into_iter()
                .map(|name| {
                    let provider = catalog.get(name.as_str()).copied();
                    RoutingProviderRow {
                        name,
                        alias: provider.and_then(|provider| provider.alias.clone()),
                        enabled: provider.map(|provider| provider.enabled).unwrap_or(false),
                        tags: provider
                            .map(|provider| provider.tags.clone())
                            .unwrap_or_default(),
                        in_catalog: provider.is_some(),
                    }
                })
                .collect(),
        )
    }

    pub(in crate::tui) fn selected_route_graph_provider_name(&self) -> Option<String> {
        self.selected_route_graph_provider_row()
            .map(|row| row.name.clone())
    }

    pub(in crate::tui) fn selected_route_graph_provider_row(&self) -> Option<RoutingProviderRow> {
        self.routing_provider_rows()?
            .get(self.selected_station_idx)
            .cloned()
    }

    pub(in crate::tui) fn selected_routing_menu_provider_row(&self) -> Option<RoutingProviderRow> {
        self.routing_provider_rows()?
            .get(self.routing_menu_idx)
            .cloned()
    }

    pub(in crate::tui) fn reordered_routing_provider_order(
        &self,
        direction: isize,
    ) -> Option<(Vec<String>, usize)> {
        let mut order = self.routing_provider_order()?;
        if order.is_empty() {
            return None;
        }
        let current_idx = self.routing_menu_idx.min(order.len().saturating_sub(1));
        let next_idx = current_idx.checked_add_signed(direction)?;
        if next_idx >= order.len() {
            return None;
        }
        order.swap(current_idx, next_idx);
        Some((order, next_idx))
    }

    pub(in crate::tui) fn clamp_routing_menu_selection(&mut self) {
        self.routing_menu_idx = self
            .routing_provider_count()
            .map(|len| self.routing_menu_idx.min(len.saturating_sub(1)))
            .unwrap_or(0);
    }

    pub(in crate::tui) fn sync_routing_menu_with_station_selection(&mut self) {
        self.routing_menu_idx = self.selected_station_idx;
        if self.routing_provider_count().is_some() {
            self.clamp_routing_menu_selection();
        }
    }

    pub(in crate::tui) fn sync_station_selection_with_routing_menu(&mut self) {
        self.selected_station_idx = self.routing_menu_idx;
        self.selected_station_idx = self
            .routing_provider_count()
            .map(|len| self.selected_station_idx.min(len.saturating_sub(1)))
            .unwrap_or(0);
    }

    pub(in crate::tui) fn sync_route_graph_table_viewport(&mut self, visible_rows: usize) {
        let len = self.routing_provider_count().unwrap_or(0);
        self.selected_station_idx = clamp_table_viewport(
            &mut self.stations_table,
            Some(self.selected_station_idx),
            len,
            visible_rows,
        )
        .unwrap_or(0);
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
                if self.sessions_page_overrides_only && !session_row_has_any_override(row) {
                    return false;
                }
                true
            })
            .take(200)
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
                    self.request_page_errors_only,
                    self.request_page_scope_session,
                    focused_sid.as_deref(),
                    self.request_page_control_filter,
                )
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    pub(in crate::tui) fn sync_request_page_selection(&mut self, snapshot: &Snapshot) {
        let len = self.request_page_filtered_indices(snapshot).len();
        self.selected_request_page_idx = clamp_table_selection(
            &mut self.request_page_table,
            Some(self.selected_request_page_idx),
            len,
        )
        .unwrap_or(0);
    }

    pub(in crate::tui) fn sync_codex_history_selection(&mut self) {
        let len = self.codex_history_sessions.len().min(300);
        let selected_idx = self
            .selected_codex_history_id
            .as_deref()
            .and_then(|sid| {
                self.codex_history_sessions
                    .iter()
                    .take(300)
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

    pub(in crate::tui) fn codex_recent_visible_indices(&self, now_ms: u64) -> Vec<usize> {
        let threshold_ms = codex_recent_window_threshold_ms(now_ms, self.codex_recent_window_idx);
        self.codex_recent_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.mtime_ms >= threshold_ms)
            .take(300)
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
        self.sync_codex_history_selection();
    }

    #[cfg(test)]
    pub(in crate::tui) fn usage_balance_provider_rows_len(&self, snapshot: &Snapshot) -> usize {
        let view = self.usage_balance_view_for_selection(snapshot);
        self.filtered_usage_balance_provider_rows(&view).len()
    }

    #[cfg(test)]
    pub(in crate::tui) fn usage_balance_view_for_selection(
        &self,
        snapshot: &Snapshot,
    ) -> UsageBalanceView {
        self.usage_balance_view_with_refresh(snapshot, crate::tui::model::now_ms(), {
            UsageBalanceRefreshInput {
                refreshing: self.balance_refresh_in_flight,
                last_message: self.last_balance_refresh_message.clone(),
                last_error: self.last_balance_refresh_error.clone(),
                last_provider_refresh: self.last_balance_refresh_summary.clone(),
            }
        })
    }

    #[cfg(test)]
    pub(in crate::tui) fn usage_balance_view_for_report(
        &self,
        snapshot: &Snapshot,
        generated_at_ms: u64,
    ) -> UsageBalanceView {
        self.usage_balance_view_with_refresh(
            snapshot,
            generated_at_ms,
            UsageBalanceRefreshInput::default(),
        )
    }

    #[cfg(test)]
    pub(in crate::tui) fn filtered_usage_balance_provider_rows<'a>(
        &self,
        view: &'a UsageBalanceView,
    ) -> Vec<&'a UsageBalanceProviderRow> {
        view.provider_rows
            .iter()
            .filter(|row| !self.stats_attention_only || row.needs_attention())
            .collect()
    }

    #[cfg(test)]
    pub(in crate::tui) fn selected_usage_balance_provider_row<'a>(
        &self,
        view: &'a UsageBalanceView,
    ) -> Option<&'a UsageBalanceProviderRow> {
        self.filtered_usage_balance_provider_rows(view)
            .into_iter()
            .nth(self.selected_stats_provider_idx)
    }

    #[cfg(test)]
    pub(in crate::tui) fn selected_usage_balance_provider_endpoints<'a>(
        &self,
        view: &'a UsageBalanceView,
    ) -> Vec<&'a UsageBalanceEndpointRow> {
        let Some(provider) = self.selected_usage_balance_provider_row(view) else {
            return Vec::new();
        };
        view.endpoint_rows
            .iter()
            .filter(|row| row.provider_id == provider.provider_id)
            .collect()
    }
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
impl UiState {
    fn usage_balance_view_with_refresh(
        &self,
        snapshot: &Snapshot,
        generated_at_ms: u64,
        refresh: UsageBalanceRefreshInput,
    ) -> UsageBalanceView {
        UsageBalanceView::build(UsageBalanceBuildInput {
            service_name: self.service_name,
            window_days: self.stats_days,
            generated_at_ms,
            usage_rollup: &snapshot.usage_rollup,
            provider_balances: &snapshot.provider_balances,
            recent: &snapshot.recent,
            routing_explain: self.routing_explain.as_ref(),
            refresh,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::state::{
        BalanceSnapshotStatus, FinishedRequest, ProviderBalanceSnapshot, SessionObservationScope,
        UsageBucket, UsageRollupView,
    };
    use crate::tui::model::RoutingProviderRef;
    use crate::tui::model::SessionRow;
    use crate::tui::types::StatsFocus;

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
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
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
                    provider_id: "stale-provider".to_string(),
                    upstream_index: Some(7),
                    status: BalanceSnapshotStatus::Stale,
                    error: Some("refresh failed".to_string()),
                    ..ProviderBalanceSnapshot::default()
                }],
            )]),
            provider_balance_history: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            provider_endpoint_policy_actions: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: std::time::Instant::now(),
        }
    }

    fn empty_session_row(id: &str) -> SessionRow {
        SessionRow {
            session_id: Some(id.to_string()),
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

    fn finished_request(id: u64, session_id: Option<&str>, status_code: u16) -> FinishedRequest {
        FinishedRequest {
            id,
            trace_id: None,
            session_id: session_id.map(ToOwned::to_owned),
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
            provider_id: None,
            station_name: None,
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
    fn sync_request_page_selection_clamps_filtered_selection() {
        let snapshot = Snapshot {
            recent: vec![
                finished_request(1, Some("sid"), 200),
                finished_request(2, Some("sid"), 500),
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
    fn route_graph_routing_detection_includes_current_v5_schema() {
        let mut ui = UiState {
            config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            ..UiState::default()
        };
        assert!(ui.uses_route_graph_routing());

        ui.config_version = Some(2);
        assert!(!ui.uses_route_graph_routing());
    }

    fn sample_route_graph_spec() -> RoutingSpecView {
        RoutingSpecView {
            entry: "main".to_string(),
            routes: BTreeMap::new(),
            policy: crate::config::RoutingPolicyV4::OrderedFailover,
            order: vec!["backup".to_string()],
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
            entry_strategy: crate::config::RoutingPolicyV4::OrderedFailover,
            expanded_order: vec!["backup".to_string(), "input".to_string()],
            entry_target: None,
            providers: vec![
                RoutingProviderRef {
                    name: "input".to_string(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::new(),
                },
                RoutingProviderRef {
                    name: "backup".to_string(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::new(),
                },
            ],
        }
    }

    #[test]
    fn route_graph_selection_tracks_provider_order_and_menu_sync() {
        let spec = sample_route_graph_spec();
        let mut ui = UiState {
            config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            routing_spec: Some(spec),
            selected_station_idx: 1,
            routing_menu_idx: 0,
            ..UiState::default()
        };

        assert_eq!(
            ui.routing_provider_order(),
            Some(vec!["backup".to_string(), "input".to_string()])
        );
        let rows = ui.routing_provider_rows().expect("routing rows");
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["backup", "input"]
        );
        assert_eq!(rows[1].display_label(), "input");
        assert_eq!(
            ui.selected_route_graph_provider_name().as_deref(),
            Some("input")
        );

        ui.sync_routing_menu_with_station_selection();
        assert_eq!(ui.routing_menu_idx, 1);
        assert_eq!(
            ui.selected_routing_menu_provider_row().map(|row| row.name),
            Some("input".to_string())
        );

        ui.routing_menu_idx = 0;
        ui.sync_station_selection_with_routing_menu();
        assert_eq!(ui.selected_station_idx, 0);
        assert_eq!(
            ui.selected_route_graph_provider_name().as_deref(),
            Some("backup")
        );
    }

    #[test]
    fn route_graph_selection_clamps_after_refresh_and_stays_on_row_model() {
        let mut spec = sample_route_graph_spec();
        spec.expanded_order = vec!["backup".to_string()];
        spec.providers
            .retain(|provider| provider.name.as_str() == "backup");
        let mut ui = UiState {
            config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            routing_spec: Some(spec),
            selected_station_idx: 9,
            routing_menu_idx: 9,
            ..UiState::default()
        };
        let snapshot = sample_usage_snapshot();

        ui.clamp_selection(&snapshot, 2);
        ui.clamp_routing_menu_selection();

        assert_eq!(ui.selected_station_idx, 0);
        assert_eq!(ui.routing_menu_idx, 0);
        assert_eq!(
            ui.selected_route_graph_provider_row()
                .map(|row| (row.name, row.in_catalog)),
            Some(("backup".to_string(), true))
        );
        assert_eq!(
            ui.selected_routing_menu_provider_row()
                .map(|row| (row.name, row.in_catalog)),
            Some(("backup".to_string(), true))
        );
    }

    #[test]
    fn route_graph_reorder_helper_returns_order_and_new_menu_selection() {
        let spec = sample_route_graph_spec();
        let ui = UiState {
            config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            routing_spec: Some(spec),
            routing_menu_idx: 1,
            ..UiState::default()
        };

        let (order, next_idx) = ui
            .reordered_routing_provider_order(-1)
            .expect("selected row can move up");

        assert_eq!(order, vec!["input".to_string(), "backup".to_string()]);
        assert_eq!(next_idx, 0);
        assert!(ui.reordered_routing_provider_order(1).is_none());
    }

    #[test]
    fn route_graph_viewport_clamp_keeps_selected_detail_row_aligned() {
        let spec = sample_route_graph_spec();
        let mut ui = UiState {
            config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            routing_spec: Some(spec),
            selected_station_idx: 8,
            stations_table: TableState::default().with_offset(8).with_selected(Some(8)),
            ..UiState::default()
        };

        ui.sync_route_graph_table_viewport(1);

        assert_eq!(ui.selected_station_idx, 1);
        assert_eq!(ui.stations_table.selected(), Some(1));
        assert_eq!(
            ui.selected_route_graph_provider_row().map(|row| row.name),
            Some("input".to_string())
        );
    }

    #[test]
    fn reset_table_viewports_keeps_selection_but_clears_offsets() {
        let mut ui = UiState {
            stations_table: TableState::default().with_offset(8).with_selected(Some(9)),
            sessions_table: TableState::default().with_offset(3).with_selected(Some(4)),
            ..UiState::default()
        };

        ui.reset_table_viewports();

        assert_eq!(ui.stations_table.selected(), Some(9));
        assert_eq!(ui.stations_table.offset(), 0);
        assert_eq!(ui.sessions_table.selected(), Some(4));
        assert_eq!(ui.sessions_table.offset(), 0);
    }

    #[test]
    fn usage_balance_selection_uses_same_filtered_provider_rows_as_table() {
        let snapshot = sample_usage_snapshot();
        let ui = UiState {
            stats_focus: StatsFocus::Providers,
            stats_attention_only: true,
            selected_stats_provider_idx: 0,
            ..UiState::default()
        };

        let view = ui.usage_balance_view_for_report(&snapshot, 123);
        let rows = ui.filtered_usage_balance_provider_rows(&view);
        let selected = ui
            .selected_usage_balance_provider_row(&view)
            .expect("selected provider");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider_id, "stale-provider");
        assert_eq!(selected.provider_id, "stale-provider");
        assert_eq!(ui.usage_balance_provider_rows_len(&snapshot), 1);
    }

    #[test]
    fn usage_balance_selected_endpoints_follow_filtered_provider_selection() {
        let snapshot = sample_usage_snapshot();
        let ui = UiState {
            stats_focus: StatsFocus::Providers,
            stats_attention_only: true,
            selected_stats_provider_idx: 0,
            ..UiState::default()
        };

        let view = ui.usage_balance_view_for_report(&snapshot, 123);
        let endpoints = ui.selected_usage_balance_provider_endpoints(&view);

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].provider_id, "stale-provider");
        assert_eq!(endpoints[0].endpoint_id, "upstream#7");
    }
}
