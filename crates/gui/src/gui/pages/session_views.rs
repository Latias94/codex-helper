use super::components::console_layout::{ConsoleTone, console_section};
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
