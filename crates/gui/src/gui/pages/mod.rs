use eframe::egui;

use std::collections::{BTreeMap, HashMap, HashSet};

use super::autostart;
use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::proxy_control::{DiscoveredProxy, GuiRuntimeSnapshot, PortInUseAction, ProxyModeKind};
use super::util::open_in_file_manager;
use crate::config::{
    GroupConfigV2, GroupMemberRefV2, PersistedProviderSpec, PersistedStationProviderRef,
    PersistedStationSpec, ProviderConfigV2, ProviderEndpointV2, RetryConfig, RetryProfileName,
    RetryStrategy,
};
use crate::dashboard_core::{
    CapabilitySupport, ControlProfileOption, HostLocalControlPlaneCapabilities, ModelCatalogKind,
    RemoteAdminAccessCapabilities, StationCapabilitySummary, StationOption,
};
use crate::doctor::{DoctorLang, DoctorStatus};
use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, RuntimeConfigState, SessionContinuityMode,
    SessionIdentityCard, SessionObservationScope, SessionStats, StationHealth,
};
use crate::usage::UsageMetrics;

mod components;
mod config_document;
mod config_legacy;
mod config_raw;
mod config_shell;
mod config_v2;
mod doctor;
mod formatting;
mod history;
mod history_tools;
mod navigation;
mod overview;
mod profile_preview;
mod remote_attach;
mod requests;
mod retry_editor;
mod runtime_station;
mod session_presentation;
mod sessions;
mod settings;
mod setup;
mod stations;
mod stats;
mod view_state;

#[allow(unused_imports)]
use config_document::{
    parse_proxy_config_document, save_proxy_config_document, sync_codex_auth_into_document,
    working_legacy_config, working_legacy_config_mut,
};
#[allow(unused_imports)]
use formatting::*;
pub use history::HistoryViewState;
#[allow(unused_imports)]
use history_tools::*;
#[allow(unused_imports)]
use navigation::page_nav_groups;
#[allow(unused_imports)]
use profile_preview::{
    ProfilePreviewStationSource, ProfileRoutePreview, build_profile_route_preview,
    local_profile_preview_catalogs_from_text, render_profile_route_preview,
    session_profile_target_station_value, session_profile_target_value,
    session_route_preview_value,
};
#[allow(unused_imports)]
use remote_attach::*;
#[allow(unused_imports)]
use retry_editor::*;
use runtime_station::*;
use session_presentation::*;
#[cfg(test)]
use view_state::SessionOverrideEditor;
pub use view_state::ViewState;
use view_state::{
    ConfigMode, ConfigProfileEditorState, ConfigProviderEditorState,
    ConfigProviderEndpointEditorState, ConfigStationEditorState, ConfigStationMemberEditorState,
    ConfigViewState, ConfigWorkingDocument, RequestsViewState, SessionsViewState,
    StationsRetryEditorState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Overview,
    Stations,
    Doctor,
    Config,
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
    pub proxy_config_text: &'a mut String,
    pub proxy_config_path: &'a std::path::Path,
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
        Page::Config => config_shell::render(ui, ctx),
        Page::Sessions => sessions::render(ui, ctx),
        Page::Requests => requests::render(ui, ctx),
        Page::Stats => stats::render(ui, ctx),
        Page::History => history::render_history(ui, ctx),
        Page::Settings => settings::render(ui, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session_row() -> SessionRow {
        SessionRow {
            session_id: Some("sid-1".to_string()),
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: Some("G:/codes/rust/codex-helper".to_string()),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_station: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
            override_service_tier: None,
        }
    }

    #[test]
    fn explain_effective_route_uses_profile_context() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.effective_service_tier = Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::ProfileDefault,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::ServiceTier, Language::Zh);

        assert_eq!(explanation.value, "priority");
        assert_eq!(explanation.source_label, "profile 默认");
        assert!(explanation.reason.contains("profile fast"));
        assert!(explanation.reason.contains("service_tier"));
    }

    #[test]
    fn explain_effective_route_handles_station_mapping_for_model() {
        let mut row = sample_session_row();
        row.last_model = Some("gpt-5.4".to_string());
        row.last_station = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_station_value = Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        });
        row.effective_model = Some(ResolvedRouteValue {
            value: "gpt-5.4-fast".to_string(),
            source: RouteValueSource::StationMapping,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::Model, Language::Zh);

        assert_eq!(explanation.source_label, "站点映射");
        assert!(explanation.reason.contains("gpt-5.4"));
        assert!(explanation.reason.contains("right"));
        assert!(explanation.reason.contains("gpt-5.4-fast"));
    }

    #[test]
    fn explain_effective_route_marks_upstream_unresolved_after_station_switch() {
        let mut row = sample_session_row();
        row.last_station = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_station_value = Some(ResolvedRouteValue {
            value: "vibe".to_string(),
            source: RouteValueSource::GlobalOverride,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::Upstream, Language::Zh);

        assert_eq!(explanation.value, "-");
        assert_eq!(explanation.source_label, "未解析");
        assert!(explanation.reason.contains("vibe"));
        assert!(explanation.reason.contains("right"));
    }

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
            Page::Config,
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
    fn session_control_posture_warns_when_bound_profile_is_missing() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.binding_continuity_mode = Some(SessionContinuityMode::ManualProfile);

        let posture = session_control_posture(&row, &[], Language::Zh);

        assert_eq!(posture.tone, SessionControlTone::Warning);
        assert!(posture.headline.contains("已缺失"));
        assert!(posture.detail.contains("找不到这个 profile"));
    }

    #[test]
    fn session_control_posture_describes_session_overrides_without_binding() {
        let mut row = sample_session_row();
        row.override_station = Some("right".to_string());
        row.override_service_tier = Some("priority".to_string());

        let posture = session_control_posture(&row, &[], Language::En);

        assert_eq!(posture.tone, SessionControlTone::Neutral);
        assert!(posture.headline.contains("no profile binding"));
        assert!(posture.detail.contains("station"));
        assert!(posture.detail.contains("service_tier"));
    }

    #[test]
    fn route_decision_changed_fields_reports_effective_drift() {
        let mut row = sample_session_row();
        row.effective_model = Some(ResolvedRouteValue {
            value: "gpt-5.4-fast".to_string(),
            source: RouteValueSource::SessionOverride,
        });
        row.effective_station_value = Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        });
        row.last_route_decision = Some(RouteDecisionProvenance {
            decided_at_ms: 123,
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.4".to_string(),
                source: RouteValueSource::ProfileDefault,
            }),
            effective_station: Some(ResolvedRouteValue {
                value: "right".to_string(),
                source: RouteValueSource::RuntimeFallback,
            }),
            ..Default::default()
        });

        let changed = route_decision_changed_fields(&row, Language::En);

        assert_eq!(changed, vec!["model".to_string()]);
    }

    #[test]
    fn session_route_decision_status_line_mentions_changed_fields() {
        let mut row = sample_session_row();
        row.effective_service_tier = Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::SessionOverride,
        });
        row.last_route_decision = Some(RouteDecisionProvenance {
            decided_at_ms: 456,
            effective_service_tier: Some(ResolvedRouteValue {
                value: "default".to_string(),
                source: RouteValueSource::ProfileDefault,
            }),
            ..Default::default()
        });

        let status = session_route_decision_status_line(&row, Language::En);

        assert!(status.contains("snapshot"));
        assert!(status.contains("service_tier"));
    }

    #[test]
    fn build_session_rows_from_cards_preserves_last_route_decision() {
        let rows = build_session_rows_from_cards(&[SessionIdentityCard {
            session_id: Some("sid-1".to_string()),
            last_route_decision: Some(RouteDecisionProvenance {
                decided_at_ms: 789,
                provider_id: Some("right".to_string()),
                effective_model: Some(ResolvedRouteValue {
                    value: "gpt-5.4-fast".to_string(),
                    source: RouteValueSource::StationMapping,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }]);

        assert_eq!(rows.len(), 1);
        let decision = rows[0]
            .last_route_decision
            .as_ref()
            .expect("route decision");
        assert_eq!(decision.decided_at_ms, 789);
        assert_eq!(decision.provider_id.as_deref(), Some("right"));
        assert_eq!(
            decision
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.4-fast")
        );
    }

    #[test]
    fn session_list_control_label_prefers_profile_binding() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.override_station = Some("right".to_string());

        assert_eq!(session_list_control_label(&row), "pf:fast");
    }

    #[test]
    fn focus_session_in_sessions_resets_filters_and_focuses_sid() {
        let mut state = SessionsViewState {
            active_only: true,
            errors_only: true,
            overrides_only: true,
            lock_order: true,
            search: "old".to_string(),
            default_profile_selection: None,
            selected_session_id: None,
            selected_idx: 9,
            ordered_session_ids: Vec::new(),
            last_active_set: HashSet::new(),
            editor: SessionOverrideEditor::default(),
        };

        focus_session_in_sessions(&mut state, "sid-history".to_string());

        assert!(!state.active_only);
        assert!(!state.errors_only);
        assert!(!state.overrides_only);
        assert_eq!(state.search, "sid-history");
        assert_eq!(state.selected_session_id.as_deref(), Some("sid-history"));
        assert_eq!(state.selected_idx, 0);
        assert!(state.lock_order);
    }

    #[test]
    fn prepare_select_requests_for_session_sets_explicit_focus() {
        let mut state = RequestsViewState {
            errors_only: true,
            scope_session: false,
            focused_session_id: None,
            selected_idx: 7,
        };

        prepare_select_requests_for_session(&mut state, "sid-req".to_string());

        assert!(!state.errors_only);
        assert!(state.scope_session);
        assert_eq!(state.focused_session_id.as_deref(), Some("sid-req"));
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn request_history_summary_from_request_builds_observed_bridge() {
        let request = FinishedRequest {
            id: 7,
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
            retry: None,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 500,
            ttfb_ms: Some(120),
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

    #[test]
    fn local_profile_preview_catalogs_from_text_extracts_v2_station_provider_structure() {
        let text = r#"
version = 2

[codex]
active_station = "primary"

[codex.providers.right]
alias = "Right"
[codex.providers.right.auth]
auth_token_env = "RIGHT_API_KEY"
[codex.providers.right.endpoints.main]
base_url = "https://right.example.com/v1"

[codex.stations.primary]
alias = "Primary"
level = 3

[[codex.stations.primary.members]]
provider = "right"
preferred = true
"#;

        let (stations, providers) =
            local_profile_preview_catalogs_from_text(text, "codex").expect("catalog");

        let station = stations.get("primary").expect("primary station");
        assert_eq!(station.alias.as_deref(), Some("Primary"));
        assert_eq!(station.level, 3);
        assert_eq!(station.members.len(), 1);
        assert_eq!(station.members[0].provider, "right");

        let provider = providers.get("right").expect("right provider");
        assert_eq!(provider.alias.as_deref(), Some("Right"));
        assert_eq!(provider.endpoints.len(), 1);
        assert_eq!(provider.endpoints[0].name, "main");
    }

    #[test]
    fn build_profile_route_preview_resolves_station_source_in_order() {
        let explicit = build_profile_route_preview(
            &crate::config::ServiceControlProfile {
                station: Some("beta".to_string()),
                ..Default::default()
            },
            Some("alpha"),
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(
            explicit.station_source,
            ProfilePreviewStationSource::Profile
        );
        assert_eq!(explicit.resolved_station_name.as_deref(), Some("beta"));

        let configured = build_profile_route_preview(
            &crate::config::ServiceControlProfile::default(),
            Some("alpha"),
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(
            configured.station_source,
            ProfilePreviewStationSource::ConfiguredActive
        );
        assert_eq!(configured.resolved_station_name.as_deref(), Some("alpha"));

        let auto = build_profile_route_preview(
            &crate::config::ServiceControlProfile::default(),
            None,
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(auto.station_source, ProfilePreviewStationSource::Auto);
        assert_eq!(auto.resolved_station_name.as_deref(), Some("gamma"));
    }

    #[test]
    fn build_profile_route_preview_collects_member_routes_and_capability_checks() {
        let station_specs = BTreeMap::from([(
            "primary".to_string(),
            PersistedStationSpec {
                name: "primary".to_string(),
                alias: Some("Primary".to_string()),
                enabled: true,
                level: 2,
                members: vec![GroupMemberRefV2 {
                    provider: "right".to_string(),
                    endpoint_names: Vec::new(),
                    preferred: true,
                }],
            },
        )]);
        let provider_catalog = BTreeMap::from([(
            "right".to_string(),
            PersistedStationProviderRef {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: true,
                endpoints: vec![
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "hk".to_string(),
                        base_url: "https://hk.example.com/v1".to_string(),
                        enabled: true,
                    },
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "us".to_string(),
                        base_url: "https://us.example.com/v1".to_string(),
                        enabled: true,
                    },
                ],
            },
        )]);
        let runtime_catalog = BTreeMap::from([(
            "primary".to_string(),
            StationOption {
                name: "primary".to_string(),
                alias: Some("Primary".to_string()),
                enabled: true,
                level: 2,
                configured_enabled: true,
                configured_level: 2,
                runtime_enabled_override: None,
                runtime_level_override: None,
                runtime_state: RuntimeConfigState::Normal,
                runtime_state_override: None,
                capabilities: StationCapabilitySummary {
                    model_catalog_kind: ModelCatalogKind::Declared,
                    supported_models: vec!["gpt-5.4".to_string()],
                    supports_service_tier: CapabilitySupport::Supported,
                    supports_reasoning_effort: CapabilitySupport::Unsupported,
                },
            },
        )]);
        let preview = build_profile_route_preview(
            &crate::config::ServiceControlProfile {
                extends: None,
                station: Some("primary".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("priority".to_string()),
            },
            None,
            None,
            Some(&station_specs),
            Some(&provider_catalog),
            Some(&runtime_catalog),
        );

        assert!(preview.station_exists);
        assert_eq!(preview.station_alias.as_deref(), Some("Primary"));
        assert_eq!(preview.members.len(), 1);
        assert!(preview.members[0].uses_all_endpoints);
        assert_eq!(
            preview.members[0].endpoint_names,
            vec!["hk".to_string(), "us".to_string()]
        );
        assert_eq!(preview.model_supported, Some(true));
        assert_eq!(preview.service_tier_supported, Some(true));
        assert_eq!(preview.reasoning_supported, Some(false));
    }
}
