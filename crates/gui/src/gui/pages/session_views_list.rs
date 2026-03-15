use super::session_views_summary::{
    session_effective_route_inline_summary, session_last_activity_summary,
    session_list_control_label,
};
use super::*;

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
