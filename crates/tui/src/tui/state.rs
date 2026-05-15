use ratatui::widgets::{ListState, TableState};

use crate::config::{ResolvedRetryConfig, is_supported_route_graph_config_version};
use crate::dashboard_core::ControlProfileOption;
use crate::routing_explain::RoutingExplainResponse;
use crate::sessions::{
    SessionMeta, SessionSummary, SessionSummarySource, SessionTranscriptMessage,
};
use crate::usage_balance::{
    UsageBalanceBuildInput, UsageBalanceEndpointRow, UsageBalanceProviderRow,
    UsageBalanceRefreshInput, UsageBalanceView,
};
use crate::usage_providers::UsageProviderRefreshSummary;
use std::collections::HashMap;

use super::Language;
use super::model::{RoutingSpecView, Snapshot, filtered_requests_len, routing_provider_names};
use super::types::{Focus, Overlay, Page, StatsFocus};

#[derive(Debug, Clone)]
pub(in crate::tui) struct RecentCodexRow {
    pub(in crate::tui) root: String,
    pub(in crate::tui) branch: Option<String>,
    pub(in crate::tui) session_id: String,
    pub(in crate::tui) cwd: Option<String>,
    pub(in crate::tui) mtime_ms: u64,
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

#[derive(Debug)]
pub(in crate::tui) struct UiState {
    pub(in crate::tui) service_name: &'static str,
    pub(in crate::tui) language: Language,
    pub(in crate::tui) refresh_ms: u64,
    pub(in crate::tui) config_version: Option<u32>,
    pub(in crate::tui) page: Page,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) overlay: Overlay,
    pub(in crate::tui) selected_station_idx: usize,
    pub(in crate::tui) selected_session_idx: usize,
    pub(in crate::tui) selected_session_id: Option<String>,
    pub(in crate::tui) selected_request_idx: usize,
    pub(in crate::tui) selected_request_page_idx: usize,
    pub(in crate::tui) focused_request_session_id: Option<String>,
    pub(in crate::tui) request_page_errors_only: bool,
    pub(in crate::tui) request_page_scope_session: bool,
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
    pub(in crate::tui) codex_recent_branch_cache: HashMap<String, Option<String>>,
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
    pub(in crate::tui) should_exit: bool,
    pub(in crate::tui) stations_table: TableState,
    pub(in crate::tui) sessions_table: TableState,
    pub(in crate::tui) requests_table: TableState,
    pub(in crate::tui) request_page_table: TableState,
    pub(in crate::tui) sessions_page_table: TableState,
    pub(in crate::tui) codex_history_table: TableState,
    pub(in crate::tui) codex_recent_table: TableState,
    pub(in crate::tui) stats_stations_table: TableState,
    pub(in crate::tui) stats_providers_table: TableState,
    pub(in crate::tui) menu_list: ListState,
    pub(in crate::tui) station_info_scroll: u16,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            service_name: "codex",
            language: Language::En,
            refresh_ms: 500,
            config_version: None,
            page: Page::Dashboard,
            focus: Focus::Sessions,
            overlay: Overlay::None,
            selected_station_idx: 0,
            selected_session_idx: 0,
            selected_session_id: None,
            selected_request_idx: 0,
            selected_request_page_idx: 0,
            focused_request_session_id: None,
            request_page_errors_only: false,
            request_page_scope_session: false,
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
            session_model_options: Vec::new(),
            session_model_input: String::new(),
            session_model_input_hint: None,
            session_service_tier_input: String::new(),
            session_service_tier_input_hint: None,
            profile_options: Vec::new(),
            configured_default_profile: None,
            effective_default_profile: None,
            runtime_default_profile_override: None,
            stats_focus: StatsFocus::Stations,
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
            codex_recent_branch_cache: HashMap::new(),
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
            should_exit: false,
            stations_table: TableState::default(),
            sessions_table: TableState::default(),
            requests_table: TableState::default(),
            request_page_table: TableState::default(),
            sessions_page_table: TableState::default(),
            codex_history_table: TableState::default(),
            codex_recent_table: TableState::default(),
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

        let stats_stations_len = snapshot.usage_rollup.by_config.len();
        self.selected_stats_station_idx = clamp_table_selection(
            &mut self.stats_stations_table,
            Some(self.selected_stats_station_idx),
            stats_stations_len,
        )
        .unwrap_or(0);

        let stats_providers_len = self.usage_balance_provider_rows_len(snapshot);
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
        self.routing_spec.as_ref().map(routing_provider_names)
    }

    pub(in crate::tui) fn routing_provider_count(&self) -> Option<usize> {
        self.routing_provider_order().map(|order| order.len())
    }

    pub(in crate::tui) fn selected_route_graph_provider_name(&self) -> Option<String> {
        self.routing_provider_order()?
            .get(self.selected_station_idx)
            .cloned()
    }

    pub(in crate::tui) fn selected_routing_menu_provider_name(&self) -> Option<String> {
        self.routing_provider_order()?
            .get(self.routing_menu_idx)
            .cloned()
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

    pub(in crate::tui) fn sync_codex_history_selection(&mut self) {
        let len = self.codex_history_sessions.len();
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
            self.codex_history_sessions.len(),
        )
        .unwrap_or(0);
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

    pub(in crate::tui) fn usage_balance_provider_rows_len(&self, snapshot: &Snapshot) -> usize {
        let view = self.usage_balance_view_for_selection(snapshot);
        self.filtered_usage_balance_provider_rows(&view).len()
    }

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

    pub(in crate::tui) fn filtered_usage_balance_provider_rows<'a>(
        &self,
        view: &'a UsageBalanceView,
    ) -> Vec<&'a UsageBalanceProviderRow> {
        view.provider_rows
            .iter()
            .filter(|row| !self.stats_attention_only || row.needs_attention())
            .collect()
    }

    pub(in crate::tui) fn selected_usage_balance_provider_row<'a>(
        &self,
        view: &'a UsageBalanceView,
    ) -> Option<&'a UsageBalanceProviderRow> {
        self.filtered_usage_balance_provider_rows(view)
            .into_iter()
            .nth(self.selected_stats_provider_idx)
    }

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
        BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket, UsageRollupView,
    };
    use crate::tui::model::RoutingProviderRef;
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
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: std::time::Instant::now(),
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

    #[test]
    fn route_graph_selection_tracks_provider_order_and_menu_sync() {
        let spec = RoutingSpecView {
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
        };
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
        assert_eq!(
            ui.selected_route_graph_provider_name().as_deref(),
            Some("input")
        );

        ui.sync_routing_menu_with_station_selection();
        assert_eq!(ui.routing_menu_idx, 1);
        assert_eq!(
            ui.selected_routing_menu_provider_name().as_deref(),
            Some("input")
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
