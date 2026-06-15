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
            for line in session_list_detail_lines(row, lang, &client, &cwd, &activity) {
                ui.small(line);
            }
            ui.small(session_route_decision_status_line(row, lang));
        })
        .response
        .interact(egui::Sense::click())
}

fn session_list_detail_lines(
    row: &SessionRow,
    lang: Language,
    client: &str,
    cwd: &str,
    activity: &str,
) -> Vec<String> {
    let mut lines = vec![
        format!("client={client}"),
        format!("cwd={cwd}"),
        session_effective_route_inline_summary(row, lang),
        activity.to_string(),
    ];
    if let Some(usage) = row.last_usage.as_ref() {
        lines.push(format!("usage(last): {}", usage_line(usage)));
    }
    if row.last_output_tokens_per_second.is_some() || row.avg_output_tokens_per_second.is_some() {
        lines.push(format!(
            "out_tok/s: last={} avg={}",
            format_tok_per_second(row.last_output_tokens_per_second),
            format_tok_per_second(row.avg_output_tokens_per_second)
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session_row() -> SessionRow {
        SessionRow {
            session_id: Some("sid-1".to_string()),
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: Some("desktop".to_string()),
            last_client_addr: Some("127.0.0.1".to_string()),
            cwd: Some("F:/work/codex-helper".to_string()),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: Some(200),
            last_duration_ms: Some(120),
            last_ended_at_ms: Some(1_000),
            last_model: Some("gpt-5".to_string()),
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_station: None,
            last_upstream_base_url: None,
            last_usage: Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 20,
                reasoning_output_tokens: 0,
                total_tokens: 30,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                ..UsageMetrics::default()
            }),
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: Some(12.5),
            avg_output_tokens_per_second: Some(8.0),
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
            override_route_target: None,
            override_service_tier: None,
        }
    }

    #[test]
    fn session_list_detail_lines_include_usage_and_tok_per_second() {
        let row = sample_session_row();
        let client = "client=desktop @ 127.0.0.1".to_string();
        let cwd = "cwd=codex-helper".to_string();
        let activity = "status=200, duration=120ms, last=0s".to_string();

        let lines = session_list_detail_lines(&row, Language::En, &client, &cwd, &activity);

        assert!(
            lines
                .iter()
                .any(|line| line.contains("usage(last): tok in/out/rsn/ttl: 10/20/0/30"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("out_tok/s: last=12.5 avg=8.0"))
        );
    }
}
