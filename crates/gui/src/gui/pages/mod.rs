use eframe::egui;

use std::collections::{BTreeMap, HashMap, HashSet};

use super::autostart;
use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::proxy_control::{DiscoveredProxy, GuiRuntimeSnapshot, PortInUseAction, ProxyModeKind};
use super::util::open_in_file_manager;
use crate::config::{RetryConfig, RetryProfileName, RetryStrategy};
use crate::dashboard_core::{
    CapabilitySupport, ControlProfileOption, HostLocalControlPlaneCapabilities, ModelCatalogKind,
    RemoteAdminAccessCapabilities, StationCapabilitySummary, StationOption,
};
use crate::doctor::{DoctorLang, DoctorStatus};
use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::state::{
    ActiveRequest, BalanceSnapshotStatus, FinishedRequest, HealthCheckStatus, LbConfigView,
    ProviderBalanceSnapshot, ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource,
    RuntimeConfigState, SessionContinuityMode, SessionIdentityCard, SessionObservationScope,
    SessionStats, StationHealth,
};
use crate::usage::UsageMetrics;

mod components;
mod doctor;
mod formatting;
mod history;
mod history_all_by_date;
mod history_all_by_date_loader;
mod history_all_by_date_transcript;
mod history_all_by_date_view;
mod history_controller;
mod history_controller_filter;
mod history_controller_refresh;
mod history_external;
mod history_git;
mod history_main_view;
mod history_observed;
mod history_observed_card_summary;
mod history_observed_placeholder;
mod history_observed_request_summary;
mod history_observed_runtime;
mod history_observed_summary;
mod history_page;
mod history_state;
#[cfg(test)]
mod history_tests;
mod history_toolbar;
mod history_toolbar_all_by_date;
mod history_toolbar_controls;
mod history_toolbar_header;
mod history_toolbar_recent;
mod history_toolbar_shortcuts;
mod history_tools;
mod history_transcript_runtime;
mod navigation;
mod overview;
mod overview_connection;
mod overview_connection_attach;
mod overview_connection_status;
mod overview_port_modal;
mod overview_runtime_status;
mod overview_runtime_status_attached;
mod overview_runtime_status_running;
mod overview_station_summary;
mod proxy_discovery;
mod proxy_settings_document;
mod proxy_settings_form;
mod proxy_settings_raw;
mod proxy_settings_shell;
mod proxy_settings_v4_editors;
mod remote_attach;
mod remote_attach_admin;
mod remote_attach_host_local;
mod remote_attach_utils;
mod requests;
mod requests_filters;
mod requests_header_actions;
mod requests_page;
mod retry_editor;
mod runtime_station;
mod runtime_station_capabilities;
mod runtime_station_health;
mod runtime_station_maps;
mod runtime_station_options;
mod session_bridge;
mod session_control_posture;
mod session_control_profile;
mod session_controls;
mod session_route_explanations;
mod session_route_fields;
mod session_route_logic;
mod session_route_reason_runtime;
mod session_route_reason_sources;
mod session_route_reasoning;
mod session_route_state;
mod session_row;
mod session_rows_aggregate;
mod session_rows_builder;
mod session_rows_from_cards;
mod session_rows_sources;
#[cfg(test)]
mod session_tests;
mod session_views;
mod session_views_identity;
mod session_views_list;
mod session_views_summary;
mod sessions;
mod sessions_controller;
mod sessions_controller_actions;
mod sessions_controller_render_data;
mod sessions_controller_types;
mod sessions_detail_controls;
mod sessions_override_editors;
mod sessions_override_effort;
mod sessions_override_model;
mod sessions_override_service_tier;
mod sessions_override_station;
mod sessions_profile_binding_editor;
mod sessions_quick_actions;
mod sessions_quick_actions_actions;
mod sessions_quick_actions_explanation;
mod sessions_quick_actions_general;
mod sessions_quick_actions_navigation;
mod sessions_split_view;
mod sessions_toolbar;
mod settings;
mod setup;
mod setup_client_step;
mod setup_config_step;
mod setup_proxy_step;
mod stations;
mod stations_detail_controls;
mod stations_detail_health;
mod stations_detail_quick_switch;
mod stations_detail_recent_hits;
mod stations_detail_runtime_control;
mod stations_detail_summary;
mod stations_empty_state;
mod stations_list_panel;
mod stations_panels;
mod stations_profile_management;
mod stations_retry_panel;
mod stations_routing_preview;
mod stations_runtime_summary;
mod stats;
mod stats_balance;
mod stats_control_trace;
mod stats_control_trace_loader;
mod stats_control_trace_summary;
mod stats_pricing;
mod stats_pricing_editor;
mod stats_request_ledger;
mod stats_summary;
mod view_state;

#[allow(unused_imports)]
use components::route_explanation::{
    format_resolved_route_value_for_field, format_route_value_for_field,
    format_service_tier_display, render_effective_route_explanation_grid,
    render_last_route_decision_card, render_observed_route_snapshot_card,
    render_session_route_snapshot_card,
};
#[allow(unused_imports)]
use formatting::*;
pub use history::HistoryViewState;
#[allow(unused_imports)]
use history_tools::*;
#[allow(unused_imports)]
use navigation::page_nav_groups;
#[allow(unused_imports)]
use proxy_settings_document::{
    parse_proxy_settings_document, sync_codex_auth_into_settings_document,
};
#[allow(unused_imports)]
use remote_attach::*;
#[allow(unused_imports)]
use retry_editor::*;
use runtime_station::*;
use session_bridge::*;
use session_controls::*;
#[allow(unused_imports)]
use session_route_explanations::*;
use session_route_logic::*;
use session_row::*;
use session_rows_builder::*;
use session_views::*;
#[cfg(test)]
use view_state::SessionOverrideEditor;
pub use view_state::ViewState;
use view_state::{
    ProxySettingsMode, ProxySettingsProviderEditorService, ProxySettingsProviderEditorState,
    ProxySettingsRoutingEditorState, ProxySettingsWorkingDocument, RequestsViewState,
    SessionsViewState, StationsRetryEditorState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Overview,
    Stations,
    Doctor,
    ProxySettings,
    Sessions,
    Requests,
    Stats,
    History,
    Settings,
}

pub struct PageCtx<'a> {
    pub lang: Language,
    pub view: &'a mut ViewState,
    pub gui_cfg: &'a mut GuiConfig,
    pub proxy_settings_text: &'a mut String,
    pub proxy_settings_path: &'a std::path::Path,
    pub last_error: &'a mut Option<String>,
    pub last_info: &'a mut Option<String>,
    pub rt: &'a tokio::runtime::Runtime,
    pub proxy: &'a mut super::proxy_control::ProxyController,
}

pub fn nav(
    ui: &mut egui::Ui,
    lang: Language,
    current: &mut Page,
    proxy: &super::proxy_control::ProxyController,
) {
    navigation::render_nav(ui, lang, current, proxy);
}

pub fn render(ui: &mut egui::Ui, page: Page, ctx: &mut PageCtx<'_>) {
    match page {
        Page::Setup => setup::render(ui, ctx),
        Page::Overview => overview::render(ui, ctx),
        Page::Stations => stations::render(ui, ctx),
        Page::Doctor => doctor::render(ui, ctx),
        Page::ProxySettings => proxy_settings_shell::render(ui, ctx),
        Page::Sessions => sessions::render(ui, ctx),
        Page::Requests => requests::render(ui, ctx),
        Page::Stats => stats::render(ui, ctx),
        Page::History => history::render_history(ui, ctx),
        Page::Settings => settings::render(ui, ctx),
    }
}

pub fn poll_global_tasks(ctx: &mut PageCtx<'_>) {
    requests_filters::poll_request_ledger_loader(ctx);
    stats_control_trace_loader::poll_control_trace_loader(ctx);
    stats_request_ledger::poll_request_ledger_summary_loader(ctx);
    session_bridge::poll_history_open_loader(ctx);
    setup_config_step::poll_setup_config_init(ctx);
    proxy_settings_document::poll_proxy_settings_save(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_base_url_loopback_detection_handles_localhosts() {
        assert!(management_base_url_is_loopback("http://127.0.0.1:3211"));
        assert!(management_base_url_is_loopback("http://localhost:3211"));
        assert!(management_base_url_is_loopback("http://[::1]:3211"));
        assert!(!management_base_url_is_loopback("http://100.79.12.5:3211"));
        assert!(!management_base_url_is_loopback(
            "https://relay.example.com/admin"
        ));
    }

    #[test]
    fn attached_host_local_session_features_require_loopback_and_capability() {
        assert!(attached_host_local_session_features_available(
            "http://127.0.0.1:3211",
            true,
        ));
        assert!(!attached_host_local_session_features_available(
            "http://127.0.0.1:3211",
            false,
        ));
        assert!(!attached_host_local_session_features_available(
            "http://100.79.12.5:3211",
            true,
        ));
        assert!(!attached_host_local_session_features_available(
            "https://relay.example.com/admin",
            true,
        ));
    }

    #[test]
    fn page_nav_groups_cover_each_page_once() {
        let all_pages = [
            Page::Setup,
            Page::Overview,
            Page::Stations,
            Page::Doctor,
            Page::ProxySettings,
            Page::Sessions,
            Page::Requests,
            Page::Stats,
            Page::History,
            Page::Settings,
        ];

        for page in all_pages {
            let count = page_nav_groups()
                .iter()
                .flat_map(|group| group.items.iter())
                .filter(|item| item.page == page)
                .count();
            assert_eq!(count, 1, "page should appear exactly once: {page:?}");
        }
    }

    #[test]
    fn remote_safe_surface_status_line_absent_for_loopback_attach() {
        let status = remote_safe_surface_status_line(
            "http://127.0.0.1:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: true,
                cwd_enrichment: true,
            },
            Language::Zh,
        );
        assert!(status.is_none());
    }

    #[test]
    fn remote_safe_surface_status_line_mentions_host_only_capabilities() {
        let status = remote_safe_surface_status_line(
            "http://100.79.12.5:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: false,
                cwd_enrichment: true,
            },
            Language::En,
        )
        .expect("remote surface status");

        assert!(status.contains("Remote attach"));
        assert!(status.contains("Session Console"));
        assert!(status.contains("session history / cwd enrichment"));
    }

    #[test]
    fn remote_local_only_warning_absent_for_loopback_attach() {
        let warning = remote_local_only_warning_message(
            "http://127.0.0.1:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: true,
                cwd_enrichment: true,
            },
            Language::Zh,
            &["cwd", "transcript"],
        );
        assert!(warning.is_none());
    }

    #[test]
    fn remote_local_only_warning_mentions_host_only_capabilities() {
        let warning = remote_local_only_warning_message(
            "http://100.79.12.5:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: false,
                cwd_enrichment: true,
            },
            Language::En,
            &["cwd", "transcript"],
        )
        .expect("remote warning");
        assert!(warning.contains("cwd / transcript"));
        assert!(warning.contains("session history / cwd enrichment"));
        assert!(warning.contains("proxy host"));
    }

    #[test]
    fn prepare_select_requests_for_session_sets_explicit_focus() {
        let mut state = RequestsViewState {
            errors_only: true,
            scope_session: false,
            focused_session_id: None,
            selected_idx: 7,
            ..RequestsViewState::default()
        };

        prepare_select_requests_for_session(&mut state, "sid-req".to_string());

        assert!(!state.errors_only);
        assert!(state.scope_session);
        assert_eq!(state.focused_session_id.as_deref(), Some("sid-req"));
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn request_service_tier_display_marks_priority_as_fast_mode() {
        assert_eq!(
            requests::request_service_tier_display(Some("priority"), Language::En),
            "priority (fast mode)"
        );
    }

    #[test]
    fn request_output_rate_uses_generation_time_after_ttfb() {
        let request = FinishedRequest {
            id: 43,
            trace_id: Some("codex-43".to_string()),
            session_id: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            station_name: None,
            provider_id: None,
            upstream_base_url: None,
            route_decision: None,
            usage: Some(UsageMetrics {
                output_tokens: 180,
                total_tokens: 180,
                ..UsageMetrics::default()
            }),
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 1_500,
            ttfb_ms: Some(500),
            streaming: false,
            ended_at_ms: 1_000,
        };

        let rate = requests::request_output_tok_per_sec(&request).expect("rate");

        assert!((rate - 180.0).abs() < f64::EPSILON);
    }

    #[test]
    fn request_route_decision_reason_explains_model_mapping() {
        let request = FinishedRequest {
            id: 42,
            trace_id: Some("codex-42".to_string()),
            session_id: Some("sid-req".to_string()),
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("right".to_string()),
            provider_id: Some("right".to_string()),
            upstream_base_url: Some("https://api.example.com/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 900,
            ttfb_ms: Some(120),
            streaming: false,
            ended_at_ms: 9_000,
        };
        let decision = RouteDecisionProvenance {
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.4-fast".to_string(),
                source: RouteValueSource::StationMapping,
            }),
            effective_station: Some(ResolvedRouteValue {
                value: "right".to_string(),
                source: RouteValueSource::RuntimeFallback,
            }),
            effective_upstream_base_url: Some(ResolvedRouteValue {
                value: "https://api.example.com/v1".to_string(),
                source: RouteValueSource::RuntimeFallback,
            }),
            provider_id: Some("right".to_string()),
            ..Default::default()
        };

        let reason = requests::request_route_decision_reason(
            &request,
            &decision,
            EffectiveRouteField::Model,
            Language::En,
        );

        assert!(reason.contains("model mapping"));
        assert!(reason.contains("gpt-5.4"));
        assert!(reason.contains("gpt-5.4-fast"));
    }

    #[test]
    fn request_history_summary_from_request_builds_observed_bridge() {
        let request = FinishedRequest {
            id: 7,
            trace_id: Some("codex-7".to_string()),
            session_id: Some("sid-req".to_string()),
            client_name: Some("Tablet".to_string()),
            client_addr: Some("100.64.0.13".to_string()),
            cwd: Some("/remote/recent".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("vibe".to_string()),
            provider_id: Some("vibe".to_string()),
            upstream_base_url: Some("https://api.example.com/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 500,
            ttfb_ms: Some(120),
            streaming: false,
            ended_at_ms: 9_000,
        };

        let summary =
            request_history_summary_from_request(&request, None, Language::En).expect("summary");

        assert_eq!(summary.id, "sid-req");
        assert_eq!(summary.source, SessionSummarySource::ObservedOnly);
        assert_eq!(summary.sort_hint_ms, Some(9_000));
        assert!(
            summary.first_user_message.as_deref().is_some_and(
                |msg| msg.contains("station=vibe") && msg.contains("path=/v1/responses")
            )
        );
    }
}
