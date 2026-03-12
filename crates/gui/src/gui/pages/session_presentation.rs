use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::*;

pub(super) fn session_row_matches_query(row: &SessionRow, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    for s in [
        row.session_id.as_deref(),
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
        row.cwd.as_deref(),
        row.last_model.as_deref(),
        row.last_service_tier.as_deref(),
        row.last_provider_id.as_deref(),
        row.last_station_name(),
        row.last_upstream_base_url.as_deref(),
        row.binding_profile_name.as_deref(),
        row.effective_model.as_ref().map(|v| v.value.as_str()),
        row.effective_reasoning_effort
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_service_tier
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_station().map(|v| v.value.as_str()),
        row.effective_upstream_base_url
            .as_ref()
            .map(|v| v.value.as_str()),
        row.override_model.as_deref(),
        row.override_effort.as_deref(),
        row.override_station_name(),
        row.override_service_tier.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if s.to_lowercase().contains(q) {
            return true;
        }
    }
    false
}

pub(super) fn format_profile_display(name: &str, profile: Option<&ControlProfileOption>) -> String {
    match profile {
        Some(profile) if profile.is_default => format!("{name} [default]"),
        _ => name.to_string(),
    }
}

pub(super) fn format_profile_summary(profile: &ControlProfileOption) -> String {
    let extends = profile.extends.as_deref().unwrap_or("<none>");
    let station = profile.station.as_deref().unwrap_or("auto");
    let model = profile.model.as_deref().unwrap_or("auto");
    let effort = profile.reasoning_effort.as_deref().unwrap_or("auto");
    let tier = profile.service_tier.as_deref().unwrap_or("auto");
    format!("extends={extends}, station={station}, model={model}, effort={effort}, tier={tier}")
}

pub(super) fn service_profile_from_option(
    profile: &ControlProfileOption,
) -> crate::config::ServiceControlProfile {
    crate::config::ServiceControlProfile {
        extends: profile.extends.clone(),
        station: profile.station.clone(),
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
    }
}

pub(super) fn resolve_service_profile_from_options(
    profile_name: &str,
    profiles: &[ControlProfileOption],
) -> anyhow::Result<crate::config::ServiceControlProfile> {
    let profile_catalog = profiles
        .iter()
        .map(|profile| (profile.name.clone(), service_profile_from_option(profile)))
        .collect::<BTreeMap<_, _>>();
    crate::config::resolve_service_profile_from_catalog(&profile_catalog, profile_name)
}

pub(super) fn render_session_profile_apply_preview(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    profile_name: &str,
    profile: &crate::config::ServiceControlProfile,
    preview: &ProfileRoutePreview,
) {
    let has_manual_overrides = session_has_manual_overrides(row);

    ui.add_space(6.0);
    ui.group(|ui| {
        ui.label(pick(lang, "应用预览", "Apply preview"));
        ui.small(pick(
            lang,
            "应用 profile 会重写当前 session binding，并清空当前会话的 station / model / reasoning / service_tier overrides。",
            "Applying a profile rewrites the current session binding and clears the session's station / model / reasoning / service_tier overrides.",
        ));

        if row.binding_profile_name.as_deref() == Some(profile_name) {
            ui.small(if has_manual_overrides {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile，但重新应用仍会清空手动 session overrides。",
                    "This session is already bound to this profile, but reapplying it will still clear manual session overrides.",
                )
            } else {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile；重新应用通常只会刷新同一份绑定。",
                    "This session is already bound to this profile; reapplying it usually just refreshes the same binding.",
                )
            });
        }

        ui.small(format!(
            "{}: {} -> {}",
            pick(lang, "binding profile", "binding profile"),
            row.binding_profile_name
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<无>", "<none>")),
            profile_name
        ));
        ui.small(format!(
            "station: {} -> {}",
            session_route_preview_value(
                row.effective_station(),
                row.last_station_name(),
                lang,
            ),
            session_profile_target_station_value(preview, lang)
        ));
        ui.small(format!(
            "model: {} -> {}",
            session_route_preview_value(row.effective_model.as_ref(), row.last_model.as_deref(), lang),
            session_profile_target_value(profile.model.as_deref(), lang)
        ));
        ui.small(format!(
            "reasoning: {} -> {}",
            session_route_preview_value(
                row.effective_reasoning_effort.as_ref(),
                row.last_reasoning_effort.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.reasoning_effort.as_deref(), lang)
        ));
        ui.small(format!(
            "service_tier: {} -> {}",
            session_route_preview_value(
                row.effective_service_tier.as_ref(),
                row.last_service_tier.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.service_tier.as_deref(), lang)
        ));
    });

    render_profile_route_preview(ui, lang, profile, preview);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionControlTone {
    Positive,
    Neutral,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionControlPosture {
    pub(super) headline: String,
    pub(super) detail: String,
    pub(super) tone: SessionControlTone,
}

pub(super) fn session_has_manual_overrides(row: &SessionRow) -> bool {
    row.override_model.is_some()
        || row.override_station_name().is_some()
        || row.override_effort.is_some()
        || row.override_service_tier.is_some()
}

pub(super) fn session_override_field_labels(row: &SessionRow, lang: Language) -> Vec<String> {
    let mut fields = Vec::new();
    if row.override_station_name().is_some() {
        fields.push(pick(lang, "station", "station").to_string());
    }
    if row.override_model.is_some() {
        fields.push("model".to_string());
    }
    if row.override_effort.is_some() {
        fields.push(pick(lang, "reasoning", "reasoning").to_string());
    }
    if row.override_service_tier.is_some() {
        fields.push("service_tier".to_string());
    }
    fields
}

pub(super) fn session_binding_mode_label(
    mode: Option<SessionContinuityMode>,
    lang: Language,
) -> String {
    match mode {
        Some(SessionContinuityMode::DefaultProfile) => {
            pick(lang, "default_profile 继承", "default_profile inherited").to_string()
        }
        Some(SessionContinuityMode::ManualProfile) => {
            pick(lang, "手动应用", "manual apply").to_string()
        }
        None => pick(lang, "<无>", "<none>").to_string(),
    }
}

pub(super) fn session_control_posture(
    row: &SessionRow,
    profiles: &[ControlProfileOption],
    lang: Language,
) -> SessionControlPosture {
    if row.session_id.is_none() {
        return SessionControlPosture {
            headline: pick(
                lang,
                "这是没有 session_id 的聚合观测条目",
                "This is an aggregated entry without a session_id",
            )
            .to_string(),
            detail: pick(
                lang,
                "它只能展示观测到的请求路由，不能建立真正的 session binding 或单会话 overrides。",
                "It can only show observed request routing, and cannot own a real session binding or per-session overrides.",
            )
            .to_string(),
            tone: SessionControlTone::Warning,
        };
    }

    let override_fields = session_override_field_labels(row, lang);
    let override_summary = override_fields.join(", ");
    let bound_profile_exists = row
        .binding_profile_name
        .as_deref()
        .is_some_and(|name| profiles.iter().any(|profile| profile.name.as_str() == name));

    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        if bound_profile_exists {
            let headline = match row.binding_continuity_mode {
                Some(SessionContinuityMode::ManualProfile) => format!(
                    "{} {profile_name}",
                    pick(
                        lang,
                        "当前由 profile 手动绑定:",
                        "Currently manually bound to profile:"
                    )
                ),
                _ => format!(
                    "{} {profile_name}",
                    pick(
                        lang,
                        "当前由 profile 继承绑定:",
                        "Currently inherited from profile:",
                    )
                ),
            };
            let detail = if override_fields.is_empty() {
                match row.binding_continuity_mode {
                    Some(SessionContinuityMode::ManualProfile) => pick(
                        lang,
                        "这是显式 apply 到该 session 的 binding；除非重新应用别的 profile 或设置 session overrides，它会继续沿用。",
                        "This binding was explicitly applied to the session; it keeps applying until another profile is reapplied or session overrides replace part of it.",
                    )
                    .to_string(),
                    _ => pick(
                        lang,
                        "这是会话创建或恢复时继承的 binding；切换“新会话默认 profile”不会自动改写它。",
                        "This binding was inherited when the session was created or restored; switching the new-session default profile does not rewrite it automatically.",
                    )
                    .to_string(),
                }
            } else {
                format!(
                    "{} {}。{}",
                    pick(
                        lang,
                        "当前还有 session overrides 覆盖这些字段:",
                        "This session also has overrides on:",
                    ),
                    override_summary,
                    pick(
                        lang,
                        "这些字段优先于 binding / profile 默认。",
                        "Those fields take priority over the binding and profile defaults.",
                    )
                )
            };
            return SessionControlPosture {
                headline,
                detail,
                tone: SessionControlTone::Positive,
            };
        }

        return SessionControlPosture {
            headline: format!(
                "{} {profile_name}",
                pick(
                    lang,
                    "当前仍绑定到已缺失的 profile:",
                    "Still bound to a missing profile:",
                )
            ),
            detail: if override_fields.is_empty() {
                pick(
                    lang,
                    "当前配置里已经找不到这个 profile；effective route 只能依赖 binding 快照和运行态结果继续解释。",
                    "The current config no longer contains this profile; the effective route can only be explained from the stored binding snapshot and runtime results.",
                )
                .to_string()
            } else {
                format!(
                    "{} {}。{}",
                    pick(
                        lang,
                        "当前配置里已经找不到这个 profile；另外还有 session overrides:",
                        "The current config no longer contains this profile; there are also session overrides on:",
                    ),
                    override_summary,
                    pick(
                        lang,
                        "这些字段仍会覆盖 binding 快照里的默认值。",
                        "Those fields still override the defaults stored in the binding snapshot.",
                    )
                )
            },
            tone: SessionControlTone::Warning,
        };
    }

    if !override_fields.is_empty() {
        return SessionControlPosture {
            headline: pick(
                lang,
                "当前没有 profile binding，靠 session overrides 控制",
                "There is no profile binding; this session is controlled by session overrides",
            )
            .to_string(),
            detail: format!(
                "{} {}。{}",
                pick(
                    lang,
                    "当前 session 显式覆盖了",
                    "This session explicitly overrides",
                ),
                override_summary,
                pick(
                    lang,
                    "这些字段优先于 profile 默认、global pin 和请求默认。",
                    "Those fields take priority over profile defaults, the global pin, and request defaults.",
                )
            ),
            tone: SessionControlTone::Neutral,
        };
    }

    if let Some(station) = row
        .effective_station()
        .filter(|value| value.source == RouteValueSource::GlobalOverride)
        .map(|value| value.value.as_str())
    {
        return SessionControlPosture {
            headline: format!(
                "{} {station}",
                pick(
                    lang,
                    "当前没有 profile binding，站点跟随全局 pin:",
                    "There is no profile binding; station follows the global pin:",
                )
            ),
            detail: pick(
                lang,
                "如果全局 pin 切换，这个 session 的 effective station 也会一起变化。",
                "If the global pin changes, this session's effective station changes with it.",
            )
            .to_string(),
            tone: SessionControlTone::Neutral,
        };
    }

    SessionControlPosture {
        headline: pick(
            lang,
            "当前没有固定 binding",
            "This session has no fixed binding",
        )
        .to_string(),
        detail: pick(
            lang,
            "effective route 主要由最近请求值、active/default station 与运行态回填共同决定。",
            "The effective route is currently determined mostly by recent request values, the active/default station, and runtime fallback.",
        )
        .to_string(),
        tone: SessionControlTone::Neutral,
    }
}

pub(super) fn session_control_tone_color(tone: SessionControlTone) -> egui::Color32 {
    match tone {
        SessionControlTone::Positive => egui::Color32::from_rgb(60, 160, 90),
        SessionControlTone::Neutral => egui::Color32::from_rgb(120, 120, 120),
        SessionControlTone::Warning => egui::Color32::from_rgb(200, 120, 40),
    }
}

pub(super) fn session_effective_route_inline_summary(row: &SessionRow, lang: Language) -> String {
    format!(
        "station={}, model={}, reasoning={}, service_tier={}",
        session_route_preview_value(row.effective_station(), row.last_station_name(), lang),
        session_route_preview_value(
            row.effective_model.as_ref(),
            row.last_model.as_deref(),
            lang
        ),
        session_route_preview_value(
            row.effective_reasoning_effort.as_ref(),
            row.last_reasoning_effort.as_deref(),
            lang,
        ),
        session_route_preview_value(
            row.effective_service_tier.as_ref(),
            row.last_service_tier.as_deref(),
            lang,
        )
    )
}

pub(super) fn session_last_activity_summary(row: &SessionRow) -> String {
    let status = row
        .last_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "-".to_string());
    let duration = row
        .last_duration_ms
        .map(|duration| format!("{duration} ms"))
        .unwrap_or_else(|| "-".to_string());
    let last = format_age(now_ms(), row.last_ended_at_ms);
    format!("status={status}, duration={duration}, last={last}")
}

pub(super) fn session_history_bridge_summary(row: &SessionRow, lang: Language) -> String {
    let mut parts = vec![session_effective_route_inline_summary(row, lang)];
    if let Some(profile) = row.binding_profile_name.as_deref() {
        parts.push(format!("profile={profile}"));
    }
    if let Some(client) = format_observed_client_identity(
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    if let Some(status) = row.last_status {
        parts.push(format!("status={status}"));
    }
    if row.active_count > 0 {
        parts.push(format!("active={}", row.active_count));
    }
    format!(
        "{}: {}",
        pick(lang, "来自 Sessions", "From Sessions"),
        parts.join(", ")
    )
}

pub(super) fn session_history_summary_from_row(
    row: &SessionRow,
    path: Option<std::path::PathBuf>,
    lang: Language,
) -> Option<SessionSummary> {
    let sid = row.session_id.clone()?;
    let sort_hint_ms = row.last_ended_at_ms.or(row.active_started_at_ms_min);
    let updated_at = sort_hint_ms.map(|ms| format_age(now_ms(), Some(ms)));
    let turns = row.turns_total.unwrap_or(0).min(usize::MAX as u64) as usize;
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    Some(SessionSummary {
        id: sid,
        path: path.unwrap_or_default(),
        cwd: row.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(session_history_bridge_summary(row, lang)),
        source,
        sort_hint_ms,
    })
}

pub(super) fn host_transcript_path_from_session_row(
    row: &SessionRow,
) -> Option<std::path::PathBuf> {
    row.host_local_transcript_path
        .as_deref()
        .map(std::path::PathBuf::from)
}

pub(super) fn request_history_bridge_summary(request: &FinishedRequest, lang: Language) -> String {
    let mut parts = vec![
        format!(
            "station={}",
            request.station_name.as_deref().unwrap_or("auto")
        ),
        format!("model={}", request.model.as_deref().unwrap_or("auto")),
        format!("tier={}", request.service_tier.as_deref().unwrap_or("auto")),
    ];
    if let Some(provider) = request.provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(client) = format_observed_client_identity(
        request.client_name.as_deref(),
        request.client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    parts.push(format!("status={}", request.status_code));
    parts.push(format!("path={}", request.path));
    format!(
        "{}: {}",
        pick(lang, "来自 Requests", "From Requests"),
        parts.join(", ")
    )
}

pub(super) fn request_history_summary_from_request(
    request: &FinishedRequest,
    path: Option<std::path::PathBuf>,
    lang: Language,
) -> Option<SessionSummary> {
    let sid = request.session_id.clone()?;
    let updated_at = Some(format_age(now_ms(), Some(request.ended_at_ms)));
    let turns = 1usize;
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    Some(SessionSummary {
        id: sid,
        path: path.unwrap_or_default(),
        cwd: request.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(request_history_bridge_summary(request, lang)),
        source,
        sort_hint_ms: Some(request.ended_at_ms),
    })
}

pub(super) fn focus_session_in_sessions(state: &mut SessionsViewState, sid: String) {
    state.active_only = false;
    state.errors_only = false;
    state.overrides_only = false;
    state.search = sid.clone();
    state.selected_session_id = Some(sid);
    state.selected_idx = 0;
}

pub(super) fn prepare_select_requests_for_session(state: &mut RequestsViewState, sid: String) {
    state.errors_only = false;
    state.scope_session = true;
    state.focused_session_id = Some(sid);
    state.selected_idx = 0;
}

pub(super) fn session_list_control_label(row: &SessionRow) -> String {
    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        return format!("pf:{}", shorten(profile_name, 10));
    }
    if let Some(station_name) = row.override_station_name() {
        return format!("pin:{}", shorten(station_name, 10));
    }
    let override_count = usize::from(row.override_model.is_some())
        + usize::from(row.override_effort.is_some())
        + usize::from(row.override_service_tier.is_some());
    if override_count > 0 {
        return format!("ovr:{override_count}");
    }
    if row.effective_station_source() == Some(RouteValueSource::GlobalOverride) {
        return "global".to_string();
    }
    "-".to_string()
}

pub(super) fn render_session_list_entry(
    ui: &mut egui::Ui,
    row: &SessionRow,
    selected: bool,
    lang: Language,
) -> egui::Response {
    let sid = row
        .session_id
        .as_deref()
        .map(|value| short_sid(value, 18))
        .unwrap_or_else(|| pick(lang, "<全局/未知>", "<all/unknown>").to_string());
    let scope = session_observation_scope_short_label(lang, row.observation_scope);
    let control = session_list_control_label(row);
    let client = format_observed_client_identity(
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
    )
    .unwrap_or_else(|| "-".to_string());
    let cwd = row
        .cwd
        .as_deref()
        .map(|value| basename(value).to_string())
        .unwrap_or_else(|| "-".to_string());
    let activity = if row.active_count > 0 {
        format!(
            "{}: {}",
            pick(lang, "活跃请求", "Active requests"),
            row.active_count
        )
    } else {
        session_last_activity_summary(row)
    };
    let stroke = if selected {
        egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().faint_bg_color
    };
    egui::Frame::group(ui.style())
        .fill(fill)
        .stroke(stroke)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(sid).monospace().strong());
                ui.small(format!("ctl={control}"));
                ui.small(format!("src={scope}"));
            });
            ui.small(format!("client={client}"));
            ui.small(format!("cwd={cwd}"));
            ui.small(session_effective_route_inline_summary(row, lang));
            ui.small(activity);
            ui.small(session_route_decision_status_line(row, lang));
        })
        .response
        .interact(egui::Sense::click())
}

pub(super) fn render_session_identity_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    profiles: &[ControlProfileOption],
    host_local_session_features: bool,
) {
    let posture = session_control_posture(row, profiles, lang);
    let sid_full = row
        .session_id
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<未知>", "<unknown>"));
    let client_full = format_observed_client_identity(
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
    )
    .unwrap_or_else(|| "-".to_string());
    let observation_scope = session_observation_scope_label(lang, row.observation_scope);
    let transcript_host_status = session_transcript_host_status_label(lang, row);
    let transcript_access =
        session_transcript_access_message(lang, row, host_local_session_features);
    let cwd_full = row.cwd.as_deref().unwrap_or("-");
    let provider = row.last_provider_id.as_deref().unwrap_or("-");
    let binding_mode = session_binding_mode_label(row.binding_continuity_mode, lang);
    let route_decision_status = session_route_decision_status_line(row, lang);

    console_section(
        ui,
        pick(lang, "会话身份卡", "Session identity"),
        ConsoleTone::Accent,
        |ui| {
            ui.monospace(format!("session_id: {sid_full}"));
            ui.small(format!(
                "{}: {observation_scope}",
                pick(lang, "identity source", "Identity source")
            ));
            ui.small(format!(
                "{}: {transcript_host_status}",
                pick(lang, "transcript(host)", "Transcript (host)")
            ));
            if let Some(path) = row.host_local_transcript_path.as_deref() {
                ui.monospace(format!("transcript_path(host): {path}"));
            }
            ui.small(transcript_access);
            ui.small(format!("client(last): {client_full}"));
            ui.small(format!("cwd: {cwd_full}"));
            ui.small(format!("provider(last): {provider}"));
            if row.binding_profile_name.is_some() || row.binding_continuity_mode.is_some() {
                ui.small(format!("binding mode: {binding_mode}"));
            }
            ui.small(format!(
                "{}: {route_decision_status}",
                pick(lang, "route decision", "Route decision")
            ));
            ui.colored_label(session_control_tone_color(posture.tone), posture.headline);
            ui.small(posture.detail);
            ui.small(format!(
                "{}: {}",
                pick(lang, "当前 effective route", "Current effective route"),
                session_effective_route_inline_summary(row, lang)
            ));
            if row.active_count > 0 {
                ui.small(format!(
                    "{}: {}",
                    pick(lang, "活跃请求数", "Active requests"),
                    row.active_count
                ));
            } else if row.last_status.is_some()
                || row.last_duration_ms.is_some()
                || row.last_ended_at_ms.is_some()
            {
                ui.small(format!(
                    "{}: {}",
                    pick(lang, "最近活动", "Last activity"),
                    session_last_activity_summary(row)
                ));
            }
            if row.session_id.is_some() {
                ui.small(if host_local_session_features {
                match row.observation_scope {
                    SessionObservationScope::HostLocalEnriched => pick(
                        lang,
                        "这台设备可直接尝试打开本地 cwd / transcript。",
                        "This device can attempt local cwd / transcript access directly.",
                    ),
                    SessionObservationScope::ObservedOnly => pick(
                        lang,
                        "这台设备具备 host-local 能力，但当前记录仍主要来自共享观测。",
                        "This device has host-local capabilities, but this record still comes primarily from shared observation data.",
                    ),
                }
            } else {
                pick(
                    lang,
                    "当前是远端附着或非本机 host 视角；只能共享观测与控制，不能假设本地有 transcript / cwd。",
                    "This is a remote-attached or non-local host view; observability and control are shared, but local transcript / cwd cannot be assumed.",
                )
            });
            }
        },
    );
}

pub(super) fn render_last_route_decision_card(ui: &mut egui::Ui, lang: Language, row: &SessionRow) {
    let changed = route_decision_changed_fields(row, lang);
    let tone = if changed.is_empty() {
        ConsoleTone::Positive
    } else {
        ConsoleTone::Warning
    };
    console_section(
        ui,
        pick(lang, "最近路由决策快照", "Last route decision"),
        tone,
        |ui| {
            let Some(decision) = row.last_route_decision.as_ref() else {
                console_note(
                    ui,
                    pick(
                        lang,
                        "当前还没有最近路由决策快照；这通常意味着还没有新的请求经过控制面，或者当前数据来自旧 attach 回退。",
                        "There is no recent route decision snapshot yet; this usually means no fresh request passed through the control plane or the current data came from the legacy attach fallback.",
                    ),
                );
                return;
            };

            ui.small(format!(
                "{}: {}",
                pick(lang, "决策时间", "Decided"),
                format_age(now_ms(), Some(decision.decided_at_ms))
            ));
            if let Some(profile_name) = decision.binding_profile_name.as_deref() {
                ui.small(format!(
                    "{}: {profile_name}",
                    pick(lang, "binding(profile)", "Binding (profile)")
                ));
            }
            if decision.binding_continuity_mode.is_some() {
                ui.small(format!(
                    "{}: {}",
                    pick(lang, "continuity", "Continuity"),
                    session_binding_mode_label(decision.binding_continuity_mode, lang)
                ));
            }
            if let Some(provider) = decision.provider_id.as_deref() {
                ui.small(format!("provider(decided): {provider}"));
            }

            egui::Grid::new((
                "sessions_last_route_decision_grid",
                row.session_id.as_deref().unwrap_or("<aggregate>"),
            ))
            .num_columns(4)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong(pick(lang, "字段", "Field"));
                ui.strong(pick(lang, "决策值", "Decision"));
                ui.strong(pick(lang, "当前值", "Current"));
                ui.strong(pick(lang, "状态", "Status"));
                ui.end_row();

                for field in EffectiveRouteField::ALL {
                    let decided = route_decision_field_value(decision, field);
                    let current = effective_route_field_value(row, field);
                    let changed = decided != current;
                    ui.label(effective_route_field_label(field, lang));
                    ui.monospace(format_resolved_route_value(decided, lang));
                    ui.monospace(format_resolved_route_value(current, lang));
                    ui.small(if changed {
                        pick(
                            lang,
                            "已偏离当前 effective route",
                            "Drifted from current effective route",
                        )
                    } else {
                        pick(lang, "与当前一致", "Matches current")
                    });
                    ui.end_row();
                }
            });

            if changed.is_empty() {
                console_note(
                    ui,
                    pick(
                        lang,
                        "当前 effective route 与最近一次决策快照一致。",
                        "The current effective route still matches the last decision snapshot.",
                    ),
                );
            } else {
                console_note(
                    ui,
                    format!(
                        "{}: {}",
                        pick(
                            lang,
                            "当前 effective route 与最近快照存在差异",
                            "The current effective route differs from the last snapshot",
                        ),
                        changed.join(", ")
                    ),
                );
            }
        },
    );
}

pub(super) fn sync_session_order(state: &mut SessionsViewState, rows: &[SessionRow]) {
    let mut current_set: HashSet<Option<String>> = HashSet::new();
    let mut active_set: HashSet<Option<String>> = HashSet::new();
    for row in rows {
        current_set.insert(row.session_id.clone());
        if row.active_count > 0 {
            active_set.insert(row.session_id.clone());
        }
    }

    if state.ordered_session_ids.is_empty() {
        state.ordered_session_ids = rows.iter().map(|r| r.session_id.clone()).collect();
        state.last_active_set = active_set;
        return;
    }

    // Always prune sessions that no longer exist in the current snapshot.
    state
        .ordered_session_ids
        .retain(|id| current_set.contains(id));

    // Ensure new sessions show up in the list. When auto reordering is enabled, insert them
    // just after the active partition (newest first, based on current snapshot ordering).
    let mut known: HashSet<Option<String>> = state.ordered_session_ids.iter().cloned().collect();
    let mut missing_active: Vec<Option<String>> = Vec::new();
    let mut missing_inactive: Vec<Option<String>> = Vec::new();
    for row in rows {
        if known.contains(&row.session_id) {
            continue;
        }
        known.insert(row.session_id.clone());
        if active_set.contains(&row.session_id) {
            missing_active.push(row.session_id.clone());
        } else {
            missing_inactive.push(row.session_id.clone());
        }
    }

    if state.lock_order {
        state.ordered_session_ids.extend(missing_active);
        state.ordered_session_ids.extend(missing_inactive);
        state.last_active_set = active_set;
        return;
    }

    // Partition active sessions to the top, without reshuffling within each partition.
    let mut active_ids: Vec<Option<String>> = Vec::new();
    let mut inactive_ids: Vec<Option<String>> = Vec::new();
    for id in state.ordered_session_ids.drain(..) {
        if active_set.contains(&id) {
            active_ids.push(id);
        } else {
            inactive_ids.push(id);
        }
    }
    state.ordered_session_ids.extend(active_ids);
    state.ordered_session_ids.extend(inactive_ids);

    let insert_at = state
        .ordered_session_ids
        .iter()
        .take_while(|id| active_set.contains(*id))
        .count();
    let active_missing_len = missing_active.len();
    state
        .ordered_session_ids
        .splice(insert_at..insert_at, missing_active);
    let insert_at2 = insert_at + active_missing_len;
    state
        .ordered_session_ids
        .splice(insert_at2..insert_at2, missing_inactive);

    state.last_active_set = active_set;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionRow {
    pub(super) session_id: Option<String>,
    pub(super) observation_scope: SessionObservationScope,
    pub(super) host_local_transcript_path: Option<String>,
    pub(super) last_client_name: Option<String>,
    pub(super) last_client_addr: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) active_count: u64,
    pub(super) active_started_at_ms_min: Option<u64>,
    pub(super) last_status: Option<u16>,
    pub(super) last_duration_ms: Option<u64>,
    pub(super) last_ended_at_ms: Option<u64>,
    pub(super) last_model: Option<String>,
    pub(super) last_reasoning_effort: Option<String>,
    pub(super) last_service_tier: Option<String>,
    pub(super) last_provider_id: Option<String>,
    pub(super) last_station: Option<String>,
    pub(super) last_upstream_base_url: Option<String>,
    pub(super) last_usage: Option<UsageMetrics>,
    pub(super) total_usage: Option<UsageMetrics>,
    pub(super) turns_total: Option<u64>,
    pub(super) turns_with_usage: Option<u64>,
    pub(super) binding_profile_name: Option<String>,
    pub(super) binding_continuity_mode: Option<crate::state::SessionContinuityMode>,
    pub(super) last_route_decision: Option<RouteDecisionProvenance>,
    pub(super) effective_model: Option<ResolvedRouteValue>,
    pub(super) effective_reasoning_effort: Option<ResolvedRouteValue>,
    pub(super) effective_service_tier: Option<ResolvedRouteValue>,
    pub(super) effective_station_value: Option<ResolvedRouteValue>,
    pub(super) effective_upstream_base_url: Option<ResolvedRouteValue>,
    pub(super) override_model: Option<String>,
    pub(super) override_effort: Option<String>,
    pub(super) override_station: Option<String>,
    pub(super) override_service_tier: Option<String>,
}

impl SessionRow {
    pub(super) fn last_station_name(&self) -> Option<&str> {
        self.last_station.as_deref()
    }

    pub(super) fn effective_station(&self) -> Option<&ResolvedRouteValue> {
        self.effective_station_value.as_ref()
    }

    pub(super) fn effective_station_name(&self) -> Option<&str> {
        self.effective_station()
            .map(|resolved| resolved.value.as_str())
    }

    pub(super) fn effective_station_source(&self) -> Option<RouteValueSource> {
        self.effective_station().map(|resolved| resolved.source)
    }

    pub(super) fn override_station_name(&self) -> Option<&str> {
        self.override_station.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectiveRouteField {
    Model,
    Station,
    Upstream,
    Effort,
    ServiceTier,
}

impl EffectiveRouteField {
    pub(super) const ALL: [Self; 5] = [
        Self::Model,
        Self::Station,
        Self::Upstream,
        Self::Effort,
        Self::ServiceTier,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectiveRouteExplanation {
    pub(super) value: String,
    pub(super) source_label: String,
    pub(super) reason: String,
}

pub(super) fn build_session_rows_from_cards(cards: &[SessionIdentityCard]) -> Vec<SessionRow> {
    let mut rows = cards
        .iter()
        .map(|card| SessionRow {
            session_id: card.session_id.clone(),
            observation_scope: card.observation_scope,
            host_local_transcript_path: card.host_local_transcript_path.clone(),
            last_client_name: card.last_client_name.clone(),
            last_client_addr: card.last_client_addr.clone(),
            cwd: card.cwd.clone(),
            active_count: card.active_count,
            active_started_at_ms_min: card.active_started_at_ms_min,
            last_status: card.last_status,
            last_duration_ms: card.last_duration_ms,
            last_ended_at_ms: card.last_ended_at_ms,
            last_model: card.last_model.clone(),
            last_reasoning_effort: card.last_reasoning_effort.clone(),
            last_service_tier: card.last_service_tier.clone(),
            last_provider_id: card.last_provider_id.clone(),
            last_station: card.last_station_name.clone(),
            last_upstream_base_url: card.last_upstream_base_url.clone(),
            last_usage: card.last_usage.clone(),
            total_usage: card.total_usage.clone(),
            turns_total: card.turns_total,
            turns_with_usage: card.turns_with_usage,
            binding_profile_name: card.binding_profile_name.clone(),
            binding_continuity_mode: card.binding_continuity_mode,
            last_route_decision: card.last_route_decision.clone(),
            effective_model: card.effective_model.clone(),
            effective_reasoning_effort: card.effective_reasoning_effort.clone(),
            effective_service_tier: card.effective_service_tier.clone(),
            effective_station_value: card.effective_station.clone(),
            effective_upstream_base_url: card.effective_upstream_base_url.clone(),
            override_model: card.override_model.clone(),
            override_effort: card.override_effort.clone(),
            override_station: card.override_station_name.clone(),
            override_service_tier: card.override_service_tier.clone(),
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

pub(super) fn build_session_rows(
    active: Vec<ActiveRequest>,
    recent: &[FinishedRequest],
    model_overrides: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
    station_overrides: &HashMap<String, String>,
    service_tier_overrides: &HashMap<String, String>,
    global_station_override: Option<&str>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionRow> {
    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionRow> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: req.client_name.clone(),
            last_client_addr: req.client_addr.clone(),
            cwd: req.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: Some(req.started_at_ms),
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: req.model.clone(),
            last_reasoning_effort: req.reasoning_effort.clone(),
            last_service_tier: req.service_tier.clone(),
            last_provider_id: req.provider_id.clone(),
            last_station: req.station_name.clone(),
            last_upstream_base_url: req.upstream_base_url.clone(),
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: req.route_decision.clone(),
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
            override_service_tier: None,
        });

        entry.active_count = entry.active_count.saturating_add(1);
        entry.active_started_at_ms_min = Some(
            entry
                .active_started_at_ms_min
                .unwrap_or(req.started_at_ms)
                .min(req.started_at_ms),
        );
        if entry.cwd.is_none() {
            entry.cwd = req.cwd;
        }
        if entry.last_client_name.is_none() {
            entry.last_client_name = req.client_name;
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = req.client_addr;
        }
        if let Some(effort) = req.reasoning_effort {
            entry.last_reasoning_effort = Some(effort);
        }
        if let Some(service_tier) = req.service_tier {
            entry.last_service_tier = Some(service_tier);
        }
        if entry.last_model.is_none() {
            entry.last_model = req.model;
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = req.provider_id;
        }
        if entry.last_station.is_none() {
            entry.last_station = req.station_name;
        }
        if entry.last_upstream_base_url.is_none() {
            entry.last_upstream_base_url = req.upstream_base_url;
        }
        update_session_row_route_decision(
            &mut entry.last_route_decision,
            req.route_decision.as_ref(),
        );
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: r.client_name.clone(),
            last_client_addr: r.client_addr.clone(),
            cwd: r.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: r.model.clone(),
            last_reasoning_effort: r.reasoning_effort.clone(),
            last_service_tier: r.service_tier.clone(),
            last_provider_id: r.provider_id.clone(),
            last_station: r.station_name.clone(),
            last_upstream_base_url: r.upstream_base_url.clone(),
            last_usage: r.usage.clone(),
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: r.route_decision.clone(),
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
            override_service_tier: None,
        });

        let should_update = entry
            .last_ended_at_ms
            .is_none_or(|prev| r.ended_at_ms >= prev);
        if should_update {
            entry.last_status = Some(r.status_code);
            entry.last_duration_ms = Some(r.duration_ms);
            entry.last_ended_at_ms = Some(r.ended_at_ms);
            entry.last_client_name = r.client_name.clone().or(entry.last_client_name.clone());
            entry.last_client_addr = r.client_addr.clone().or(entry.last_client_addr.clone());
            entry.last_model = r.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = r
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_service_tier = r.service_tier.clone().or(entry.last_service_tier.clone());
            entry.last_provider_id = r.provider_id.clone().or(entry.last_provider_id.clone());
            entry.last_station = r.station_name.clone().or(entry.last_station.clone());
            entry.last_upstream_base_url = r
                .upstream_base_url
                .clone()
                .or(entry.last_upstream_base_url.clone());
            entry.last_usage = r.usage.clone().or(entry.last_usage.clone());
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
        update_session_row_route_decision(
            &mut entry.last_route_decision,
            r.route_decision.as_ref(),
        );
    }

    for (sid, st) in stats.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
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
            last_route_decision: st.last_route_decision.clone(),
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
            override_service_tier: None,
        });

        if entry.turns_total.is_none() {
            entry.turns_total = Some(st.turns_total);
        }
        if entry.last_client_name.is_none() {
            entry.last_client_name = st.last_client_name.clone();
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = st.last_client_addr.clone();
        }
        if entry.last_status.is_none() {
            entry.last_status = st.last_status;
        }
        if entry.last_duration_ms.is_none() {
            entry.last_duration_ms = st.last_duration_ms;
        }
        if entry.last_ended_at_ms.is_none() {
            entry.last_ended_at_ms = st.last_ended_at_ms;
        }
        if entry.last_model.is_none() {
            entry.last_model = st.last_model.clone();
        }
        if entry.last_reasoning_effort.is_none() {
            entry.last_reasoning_effort = st.last_reasoning_effort.clone();
        }
        if entry.last_service_tier.is_none() {
            entry.last_service_tier = st.last_service_tier.clone();
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = st.last_provider_id.clone();
        }
        if entry.last_station.is_none() {
            entry.last_station = st.last_station_name.clone();
        }
        if entry.last_usage.is_none() {
            entry.last_usage = st.last_usage.clone();
        }
        if entry.total_usage.is_none() {
            entry.total_usage = Some(st.total_usage.clone());
        }
        if entry.turns_with_usage.is_none() {
            entry.turns_with_usage = Some(st.turns_with_usage);
        }
        update_session_row_route_decision(
            &mut entry.last_route_decision,
            st.last_route_decision.as_ref(),
        );
    }

    for (sid, model) in model_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
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
        });
        entry.override_model = Some(model.clone());
    }

    for (sid, eff) in overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
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
        });
        entry.override_effort = Some(eff.clone());
    }

    for (sid, cfg_name) in station_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
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
        });
        entry.override_station = Some(cfg_name.clone());
    }

    for (sid, service_tier) in service_tier_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
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
        });
        entry.override_service_tier = Some(service_tier.clone());
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    for row in &mut rows {
        if row.cwd.is_some() {
            row.observation_scope = SessionObservationScope::HostLocalEnriched;
        }
        apply_effective_route_to_row(row, global_station_override);
    }
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

pub(super) fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

pub(super) fn update_session_row_route_decision(
    slot: &mut Option<RouteDecisionProvenance>,
    candidate: Option<&RouteDecisionProvenance>,
) {
    let Some(candidate) = candidate.cloned() else {
        return;
    };
    let current_at = slot
        .as_ref()
        .map(|decision| decision.decided_at_ms)
        .unwrap_or(0);
    if current_at <= candidate.decided_at_ms {
        *slot = Some(candidate);
    }
}

pub(super) fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn format_observed_client_identity(
    client_name: Option<&str>,
    client_addr: Option<&str>,
) -> Option<String> {
    match (
        non_empty_trimmed(client_name),
        non_empty_trimmed(client_addr),
    ) {
        (Some(name), Some(addr)) => Some(format!("{name} @ {addr}")),
        (Some(name), None) => Some(name),
        (None, Some(addr)) => Some(addr),
        (None, None) => None,
    }
}

pub(super) fn session_observation_scope_short_label(
    lang: Language,
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "obs", "obs"),
        SessionObservationScope::HostLocalEnriched => pick(lang, "host", "host"),
    }
}

pub(super) fn session_observation_scope_label(
    lang: Language,
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "仅共享观测", "Observed only"),
        SessionObservationScope::HostLocalEnriched => {
            pick(lang, "代理主机 enrich", "Host-local enriched")
        }
    }
}

pub(super) fn session_transcript_host_status_label(lang: Language, row: &SessionRow) -> String {
    if row.host_local_transcript_path.is_some() {
        pick(
            lang,
            "已在 ~/.codex/sessions 链接",
            "linked under ~/.codex/sessions",
        )
        .to_string()
    } else if row.session_id.is_some() {
        pick(
            lang,
            "未检测到 host-local transcript",
            "no host-local transcript detected",
        )
        .to_string()
    } else {
        pick(lang, "无 session_id，无法匹配", "no session_id to match").to_string()
    }
}

pub(super) fn session_transcript_access_message(
    lang: Language,
    row: &SessionRow,
    host_local_session_features: bool,
) -> String {
    match (
        row.session_id.is_some(),
        row.host_local_transcript_path.is_some(),
        host_local_session_features,
    ) {
        (false, _, _) => pick(
            lang,
            "当前记录没有 session_id，不能建立 transcript 映射。",
            "This record has no session_id, so transcript mapping is unavailable.",
        )
        .to_string(),
        (true, true, true) => pick(
            lang,
            "这台设备可直接打开这个 host-local transcript。",
            "This device can open the linked host-local transcript directly.",
        )
        .to_string(),
        (true, true, false) => pick(
            lang,
            "代理主机已链接到 transcript，但当前附着设备不能直接访问代理主机的文件系统。",
            "The proxy host has a linked transcript, but this attached device cannot access the proxy host filesystem directly.",
        )
        .to_string(),
        (true, false, true) => pick(
            lang,
            "这台设备具备 host-local 能力，但当前未在 ~/.codex/sessions 下找到匹配文件。",
            "This device has host-local access, but no matching file was found under ~/.codex/sessions yet.",
        )
        .to_string(),
        (true, false, false) => pick(
            lang,
            "当前是远端附着视角；可控制该 session_id，但不能假设本机可读取代理主机的 transcript。",
            "This is a remote-attached view; the session_id is controllable, but local transcript access on the proxy host cannot be assumed here.",
        )
        .to_string(),
    }
}

pub(super) fn resolve_effective_observed_value(
    override_value: Option<&str>,
    observed_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(override_value) {
        return Some(ResolvedRouteValue {
            value,
            source: RouteValueSource::SessionOverride,
        });
    }
    non_empty_trimmed(observed_value).map(|value| ResolvedRouteValue {
        value,
        source: RouteValueSource::RequestPayload,
    })
}

pub(super) fn apply_effective_route_to_row(
    row: &mut SessionRow,
    global_station_override: Option<&str>,
) {
    row.effective_model =
        resolve_effective_observed_value(row.override_model.as_deref(), row.last_model.as_deref());
    row.effective_reasoning_effort = resolve_effective_observed_value(
        row.override_effort.as_deref(),
        row.last_reasoning_effort.as_deref(),
    );
    row.effective_service_tier = resolve_effective_observed_value(
        row.override_service_tier.as_deref(),
        row.last_service_tier.as_deref(),
    );
    row.effective_station_value =
        if let Some(value) = non_empty_trimmed(row.override_station_name()) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::SessionOverride,
            })
        } else if let Some(value) = non_empty_trimmed(global_station_override) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::GlobalOverride,
            })
        } else {
            non_empty_trimmed(row.last_station_name()).map(|value| ResolvedRouteValue {
                value,
                source: RouteValueSource::RuntimeFallback,
            })
        };
    row.effective_upstream_base_url = match (
        row.effective_station(),
        non_empty_trimmed(row.last_station_name()),
        non_empty_trimmed(row.last_upstream_base_url.as_deref()),
    ) {
        (Some(config), Some(last_config), Some(upstream)) if config.value == last_config => {
            Some(ResolvedRouteValue {
                value: upstream,
                source: RouteValueSource::RuntimeFallback,
            })
        }
        _ => None,
    };
}

pub(super) fn route_value_source_label(source: RouteValueSource, lang: Language) -> &'static str {
    match source {
        RouteValueSource::RequestPayload => pick(lang, "请求体", "request payload"),
        RouteValueSource::SessionOverride => pick(lang, "会话覆盖", "session override"),
        RouteValueSource::GlobalOverride => pick(lang, "全局覆盖", "global override"),
        RouteValueSource::ProfileDefault => pick(lang, "profile 默认", "profile default"),
        RouteValueSource::StationMapping => pick(lang, "站点映射", "station mapping"),
        RouteValueSource::RuntimeFallback => pick(lang, "运行时兜底", "runtime fallback"),
    }
}

pub(super) fn format_resolved_route_value(
    value: Option<&ResolvedRouteValue>,
    lang: Language,
) -> String {
    match value {
        Some(value) => format!(
            "{} [{}]",
            value.value,
            route_value_source_label(value.source, lang)
        ),
        None => "-".to_string(),
    }
}

pub(super) fn unresolved_route_source_label(lang: Language) -> &'static str {
    pick(lang, "未解析", "unresolved")
}

pub(super) fn effective_route_field_label(
    field: EffectiveRouteField,
    lang: Language,
) -> &'static str {
    match field {
        EffectiveRouteField::Model => pick(lang, "模型", "model"),
        EffectiveRouteField::Station => pick(lang, "站点", "station"),
        EffectiveRouteField::Upstream => "upstream",
        EffectiveRouteField::Effort => pick(lang, "思考强度", "effort"),
        EffectiveRouteField::ServiceTier => "service_tier",
    }
}

pub(super) fn effective_route_field_value(
    row: &SessionRow,
    field: EffectiveRouteField,
) -> Option<&ResolvedRouteValue> {
    match field {
        EffectiveRouteField::Model => row.effective_model.as_ref(),
        EffectiveRouteField::Station => row.effective_station(),
        EffectiveRouteField::Upstream => row.effective_upstream_base_url.as_ref(),
        EffectiveRouteField::Effort => row.effective_reasoning_effort.as_ref(),
        EffectiveRouteField::ServiceTier => row.effective_service_tier.as_ref(),
    }
}

pub(super) fn route_decision_field_value(
    decision: &RouteDecisionProvenance,
    field: EffectiveRouteField,
) -> Option<&ResolvedRouteValue> {
    match field {
        EffectiveRouteField::Model => decision.effective_model.as_ref(),
        EffectiveRouteField::Station => decision.effective_station.as_ref(),
        EffectiveRouteField::Upstream => decision.effective_upstream_base_url.as_ref(),
        EffectiveRouteField::Effort => decision.effective_reasoning_effort.as_ref(),
        EffectiveRouteField::ServiceTier => decision.effective_service_tier.as_ref(),
    }
}

pub(super) fn route_decision_changed_fields(row: &SessionRow, lang: Language) -> Vec<String> {
    let Some(decision) = row.last_route_decision.as_ref() else {
        return Vec::new();
    };
    EffectiveRouteField::ALL
        .into_iter()
        .filter(|field| {
            effective_route_field_value(row, *field) != route_decision_field_value(decision, *field)
        })
        .map(|field| effective_route_field_label(field, lang).to_string())
        .collect()
}

pub(super) fn session_route_decision_status_line(row: &SessionRow, lang: Language) -> String {
    let Some(decision) = row.last_route_decision.as_ref() else {
        return pick(
            lang,
            "暂无最近路由决策快照",
            "No recent route decision snapshot",
        )
        .to_string();
    };
    let age = format_age(now_ms(), Some(decision.decided_at_ms));
    let changed = route_decision_changed_fields(row, lang);
    if changed.is_empty() {
        format!(
            "{}: {}",
            pick(
                lang,
                "最近路由决策仍与当前 effective route 一致",
                "Last route decision still matches the current effective route",
            ),
            age
        )
    } else {
        format!(
            "{}: {} ({})",
            pick(lang, "最近路由决策快照", "Last route decision snapshot",),
            age,
            changed.join(", ")
        )
    }
}

pub(super) fn binding_profile_reference(row: &SessionRow, lang: Language) -> String {
    match row.binding_profile_name.as_deref() {
        Some(name) => format!("profile {name}"),
        None => pick(lang, "当前绑定 profile", "the bound profile").to_string(),
    }
}

pub(super) fn runtime_fallback_explanation(
    row: &SessionRow,
    field: EffectiveRouteField,
    value: &ResolvedRouteValue,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => match row.last_station_name() {
            Some(last_config) if last_config == value.value => pick(
                lang,
                "当前没有 session pin、global pin 或 profile 默认，沿用最近观测到的站点。",
                "No session pin, global pin, or profile default applies, so the station falls back to the most recently observed value.",
            )
            .to_string(),
            Some(last_config) => format!(
                "{} {}；{} {}。",
                pick(
                    lang,
                    "当前没有 session pin、global pin 或 profile 默认，运行态把站点回填为",
                    "No session pin, global pin, or profile default applies, so runtime filled the station as",
                ),
                value.value,
                pick(
                    lang,
                    "最近观测到的站点仍是",
                    "while the most recently observed station is still",
                ),
                last_config
            ),
            None => format!(
                "{} {}。",
                pick(
                    lang,
                    "当前没有更明确的站点来源，运行态回填为",
                    "No more explicit station source is available, so runtime filled it as",
                ),
                value.value
            ),
        },
        EffectiveRouteField::Upstream => {
            let effective_station = row.effective_station_name();
            match (
                effective_station,
                row.last_station_name(),
                row.last_upstream_base_url.as_deref(),
            ) {
                (Some(station), Some(last_config), Some(last_upstream))
                    if station == last_config && last_upstream == value.value =>
                {
                    format!(
                        "{} {}，{} {}。",
                        pick(
                            lang,
                            "当前生效站点与最近观测一致，沿用该站点最近命中的 upstream",
                            "The effective station matches the last observed station, so the upstream falls back to the most recently observed target",
                        ),
                        value.value,
                        pick(lang, "所属站点", "for station"),
                        station
                    )
                }
                (Some(station), _, _) => format!(
                    "{} {}，{} {}。",
                    pick(
                        lang,
                        "当前站点可在运行态唯一补全 upstream",
                        "The current station can be completed to a single upstream at runtime",
                    ),
                    value.value,
                    pick(lang, "所属站点", "for station"),
                    station
                ),
                _ => format!(
                    "{} {}。",
                    pick(
                        lang,
                        "运行态补全了当前 upstream",
                        "Runtime completed the current upstream as",
                    ),
                    value.value
                ),
            }
        }
        _ => format!(
            "{} {}，{}。",
            pick(
                lang,
                "当前没有更高优先级的覆盖或默认值，沿用最近观测到的",
                "No higher-priority override or default applies, so the field falls back to the most recently observed",
            ),
            effective_route_field_label(field, lang),
            value.value
        ),
    }
}

pub(super) fn unresolved_effective_route_reason(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => pick(
            lang,
            "当前没有 session pin、global pin、profile 默认，也没有最近可用的站点记录。",
            "There is no session pin, global pin, profile default, or recent station observation to resolve the current station.",
        )
        .to_string(),
        EffectiveRouteField::Upstream => {
            let effective_station = row.effective_station_name();
            match (effective_station, row.last_station_name()) {
                (Some(station), Some(last_station))
                    if station != last_station && row.last_upstream_base_url.is_some() =>
                {
                    format!(
                        "{} {}，{} {}，{}。",
                        pick(
                            lang,
                            "当前生效站点已经切到",
                            "The effective station has already switched to",
                        ),
                        station,
                        pick(
                            lang,
                            "但最近观测到的 upstream 仍属于站点",
                            "but the most recently observed upstream still belongs to station",
                        ),
                        last_station,
                        pick(
                            lang,
                            "所以不能直接把它当成当前 upstream",
                            "so it cannot be treated as the current upstream",
                        )
                    )
                }
                (Some(station), _) => format!(
                    "{} {}，{}。",
                    pick(
                        lang,
                        "当前站点是",
                        "The current station is",
                    ),
                    station,
                    pick(
                        lang,
                        "但缺少最近 upstream 观测或唯一映射，因此暂时无法解释 upstream",
                        "but there is no recent upstream observation or unique mapping, so the upstream cannot be explained yet",
                    )
                ),
                (None, _) => pick(
                    lang,
                    "当前连 effective station 都还没有判定，因此无法解释 upstream。",
                    "The effective station itself is still unresolved, so the upstream cannot be explained.",
                )
                .to_string(),
            }
        }
        _ => format!(
            "{} {}。",
            pick(
                lang,
                "当前既没有覆盖、profile 默认，也没有最近请求值，无法判定",
                "There is no override, profile default, or recent request value to resolve",
            ),
            effective_route_field_label(field, lang)
        ),
    }
}

pub(super) fn explain_effective_route_field(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> EffectiveRouteExplanation {
    let value = effective_route_field_value(row, field);
    let value_label = value
        .map(|resolved| resolved.value.clone())
        .unwrap_or_else(|| "-".to_string());
    let source_label = value
        .map(|resolved| route_value_source_label(resolved.source, lang).to_string())
        .unwrap_or_else(|| unresolved_route_source_label(lang).to_string());
    let field_label = effective_route_field_label(field, lang);

    let reason = match value {
        Some(resolved) => match resolved.source {
            RouteValueSource::SessionOverride => format!(
                "{} {}={}，{}。",
                pick(
                    lang,
                    "当前 session 显式覆盖了",
                    "The current session explicitly overrides",
                ),
                field_label,
                resolved.value,
                pick(
                    lang,
                    "因此它优先于其他来源生效",
                    "so it takes priority over every other source",
                )
            ),
            RouteValueSource::GlobalOverride => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前 session 没有单独站点覆盖，命中了全局 pin，当前站点固定为",
                    "The current session has no dedicated station override and therefore follows the global pin to",
                ),
                resolved.value,
                pick(
                    lang,
                    "所以这里以全局结果为准",
                    "so the global choice is authoritative here",
                )
            ),
            RouteValueSource::ProfileDefault => format!(
                "{} {}，{} {}={}。",
                pick(
                    lang,
                    "当前 session 绑定到",
                    "The current session is bound to",
                ),
                binding_profile_reference(row, lang),
                pick(lang, "其默认", "whose default",),
                field_label,
                resolved.value
            ),
            RouteValueSource::RequestPayload => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前没有 session override 或 profile 默认，沿用最近请求体里的",
                    "There is no session override or profile default, so the field follows the latest request payload for",
                ),
                field_label,
                resolved.value
            ),
            RouteValueSource::StationMapping => {
                let requested_model = row.last_model.as_deref().unwrap_or("-");
                let station = row
                    .effective_station_name()
                    .or(row.last_station_name())
                    .unwrap_or("-");
                let upstream = row.last_upstream_base_url.as_deref().unwrap_or("-");
                format!(
                    "{} {}，{} {} / {} {}，{} {}。",
                    pick(
                        lang,
                        "最近请求提交的模型是",
                        "The most recent request submitted model",
                    ),
                    requested_model,
                    pick(lang, "但站点", "but station"),
                    station,
                    pick(lang, "upstream", "upstream"),
                    upstream,
                    pick(
                        lang,
                        "的 model mapping 将实际模型改写为",
                        "rewrote the effective model through model mapping to",
                    ),
                    resolved.value
                )
            }
            RouteValueSource::RuntimeFallback => {
                runtime_fallback_explanation(row, field, resolved, lang)
            }
        },
        None => unresolved_effective_route_reason(row, field, lang),
    };

    EffectiveRouteExplanation {
        value: value_label,
        source_label,
        reason,
    }
}
