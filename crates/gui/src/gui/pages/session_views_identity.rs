use super::components::console_layout::{ConsoleTone, console_section};
use super::session_views_summary::{
    session_current_target_summary, session_effective_route_inline_summary,
    session_last_activity_summary, session_last_executed_target_summary,
    session_route_affinity_summary,
};
use super::*;

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
    let binding_mode = session_binding_mode_label(row.binding_continuity_mode, lang);
    let binding_profile_summary = session_binding_profile_summary(row, profiles, lang);
    let manual_override_summary = session_manual_override_summary(row, lang);
    let route_decision_status = session_route_decision_status_line(row, lang);
    let current_target_summary = session_current_target_summary(row, lang);
    let last_executed_target_summary = session_last_executed_target_summary(row, lang);
    let route_affinity_summary = session_route_affinity_summary(row, lang);

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
            if let Some(profile_name) = row.binding_profile_name.as_deref() {
                ui.small(format!(
                    "{}: {profile_name}",
                    pick(lang, "binding(profile)", "Binding (profile)")
                ));
            }
            if row.binding_profile_name.is_some() || row.binding_continuity_mode.is_some() {
                ui.small(format!("binding mode: {binding_mode}"));
            }
            if let Some(profile_summary) = binding_profile_summary {
                ui.small(format!(
                    "{}: {profile_summary}",
                    pick(lang, "binding(profile spec)", "Binding (profile spec)")
                ));
            }
            ui.small(format!(
                "{}: {manual_override_summary}",
                pick(lang, "manual overrides", "Manual overrides")
            ));
            ui.small(format!(
                "{}: {route_decision_status}",
                pick(lang, "route decision", "Route decision")
            ));
            ui.colored_label(session_control_tone_color(posture.tone), posture.headline);
            ui.small(posture.detail);
            ui.small(format!(
                "{}: {current_target_summary}",
                pick(lang, "当前目标", "Current target")
            ));
            if let Some(summary) = route_affinity_summary {
                ui.small(format!(
                    "{}: {summary}",
                    pick(lang, "session 粘性", "Session affinity")
                ));
            }
            ui.small(format!(
                "{}: {last_executed_target_summary}",
                pick(lang, "最近执行目标", "Last executed target")
            ));
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
