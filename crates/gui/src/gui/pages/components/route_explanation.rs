use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::{
    EffectiveRouteField, SessionRow, effective_route_field_label, effective_route_field_value,
    explain_effective_route_field, format_age, non_empty_trimmed, now_ms,
    route_decision_changed_fields, route_decision_field_value, route_value_source_label,
    session_binding_mode_label, shorten_middle,
};
use super::console_layout::{ConsoleTone, console_kv_grid, console_note, console_section};
use crate::state::ResolvedRouteValue;

pub(in super::super) fn format_service_tier_display(
    value: Option<&str>,
    lang: Language,
    empty: &str,
) -> String {
    match non_empty_trimmed(value) {
        Some(value) if value.eq_ignore_ascii_case("priority") => {
            format!("{value} ({})", pick(lang, "fast mode", "fast mode"))
        }
        Some(value) => value,
        None => empty.to_string(),
    }
}

pub(in super::super) fn format_route_value_for_field(
    value: Option<&ResolvedRouteValue>,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match value {
        Some(value) => match field {
            EffectiveRouteField::ServiceTier => {
                format_service_tier_display(Some(value.value.as_str()), lang, "-")
            }
            EffectiveRouteField::Upstream => shorten_middle(&value.value, 72),
            _ => value.value.clone(),
        },
        None => "-".to_string(),
    }
}

pub(in super::super) fn format_resolved_route_value_for_field(
    value: Option<&ResolvedRouteValue>,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match value {
        Some(value) => format!(
            "{} [{}]",
            format_route_value_for_field(Some(value), field, lang),
            route_value_source_label(value.source, lang)
        ),
        None => "-".to_string(),
    }
}

pub(in super::super) fn render_effective_route_explanation_grid(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    grid_id_prefix: &'static str,
) {
    egui::Grid::new((
        grid_id_prefix,
        row.session_id.as_deref().unwrap_or("<aggregate>"),
    ))
    .num_columns(3)
    .spacing([12.0, 6.0])
    .striped(true)
    .show(ui, |ui| {
        ui.strong(pick(lang, "字段", "Field"));
        ui.strong(pick(lang, "当前值 / 来源", "Value / source"));
        ui.strong(pick(lang, "为什么", "Why"));
        ui.end_row();

        for field in EffectiveRouteField::ALL {
            let explanation = explain_effective_route_field(row, field, lang);
            ui.label(effective_route_field_label(field, lang));
            ui.vertical(|ui| {
                ui.monospace(format_route_value_for_field(
                    effective_route_field_value(row, field),
                    field,
                    lang,
                ));
                ui.small(format!("[{}]", explanation.source_label));
            });
            ui.small(explanation.reason);
            ui.end_row();
        }
    });
}

pub(in super::super) fn render_last_route_decision_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
) {
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
                    ui.monospace(format_resolved_route_value_for_field(decided, field, lang));
                    ui.monospace(format_resolved_route_value_for_field(current, field, lang));
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

fn observed_route_snapshot_rows(
    row: &SessionRow,
    lang: Language,
    key_suffix: &str,
    shorten_upstream: bool,
) -> Vec<(String, String)> {
    vec![
        (
            format!("model{key_suffix}"),
            row.last_model.as_deref().unwrap_or("-").to_string(),
        ),
        (
            format!("station{key_suffix}"),
            row.last_station_name().unwrap_or("-").to_string(),
        ),
        (
            format!("upstream{key_suffix}"),
            row.last_upstream_base_url
                .as_deref()
                .map(|value| {
                    if shorten_upstream {
                        shorten_middle(value, 72)
                    } else {
                        value.to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            format!("effort{key_suffix}"),
            row.last_reasoning_effort
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            format!("service_tier{key_suffix}"),
            format_service_tier_display(row.last_service_tier.as_deref(), lang, "-"),
        ),
    ]
}

fn effective_route_snapshot_rows(row: &SessionRow, lang: Language) -> Vec<(String, String)> {
    vec![
        (
            "model".to_string(),
            format_resolved_route_value_for_field(
                row.effective_model.as_ref(),
                EffectiveRouteField::Model,
                lang,
            ),
        ),
        (
            "station".to_string(),
            format_resolved_route_value_for_field(
                row.effective_station(),
                EffectiveRouteField::Station,
                lang,
            ),
        ),
        (
            "upstream".to_string(),
            format_resolved_route_value_for_field(
                row.effective_upstream_base_url.as_ref(),
                EffectiveRouteField::Upstream,
                lang,
            ),
        ),
        (
            "effort".to_string(),
            format_resolved_route_value_for_field(
                row.effective_reasoning_effort.as_ref(),
                EffectiveRouteField::Effort,
                lang,
            ),
        ),
        (
            "service_tier".to_string(),
            format_resolved_route_value_for_field(
                row.effective_service_tier.as_ref(),
                EffectiveRouteField::ServiceTier,
                lang,
            ),
        ),
    ]
}

pub(in super::super) fn render_observed_route_snapshot_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
) {
    let observed_rows = observed_route_snapshot_rows(row, lang, "", true);
    let effective_rows = effective_route_snapshot_rows(row, lang);

    console_section(
        ui,
        pick(lang, "共享路由快照", "Observed route snapshot"),
        ConsoleTone::Accent,
        |ui| {
            ui.columns(2, |cols| {
                cols[0].label(pick(lang, "最近观测", "Observed"));
                console_kv_grid(
                    &mut cols[0],
                    (
                        "history_observed_route_observed_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &observed_rows,
                );

                cols[1].label(pick(lang, "当前生效", "Effective"));
                console_kv_grid(
                    &mut cols[1],
                    (
                        "history_observed_route_effective_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &effective_rows,
                );
            });

            if row
                .effective_service_tier
                .as_ref()
                .is_some_and(|value| value.value.eq_ignore_ascii_case("priority"))
            {
                ui.add_space(6.0);
                console_note(
                    ui,
                    pick(
                        lang,
                        "当前 effective route 显示 service_tier=priority，可视为 fast mode。",
                        "The current effective route shows service_tier=priority, which can be treated as fast mode.",
                    ),
                );
            }
        },
    );
}

pub(in super::super) fn render_session_route_snapshot_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    global_station_override: Option<&str>,
) {
    let observed_rows = observed_route_snapshot_rows(row, lang, "(last)", false);
    let effective_rows = effective_route_snapshot_rows(row, lang);
    let override_summary = format!(
        "model={}, effort={}, station={}, tier={}, global_station={}",
        row.override_model.as_deref().unwrap_or("-"),
        row.override_effort.as_deref().unwrap_or("-"),
        row.override_station_name().unwrap_or("-"),
        row.override_service_tier.as_deref().unwrap_or("-"),
        global_station_override.unwrap_or("-"),
    );

    console_section(
        ui,
        pick(lang, "路由快照", "Route snapshot"),
        ConsoleTone::Accent,
        |ui| {
            ui.columns(2, |cols| {
                cols[0].label(pick(lang, "最近观测", "Observed"));
                console_kv_grid(
                    &mut cols[0],
                    (
                        "sessions_route_observed_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &observed_rows,
                );

                cols[1].label(pick(lang, "当前生效", "Effective"));
                console_kv_grid(
                    &mut cols[1],
                    (
                        "sessions_route_effective_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &effective_rows,
                );
            });
            ui.add_space(6.0);
            console_note(
                ui,
                format!(
                    "{}: {override_summary}",
                    pick(lang, "覆盖概览", "Override summary")
                ),
            );
        },
    );
}
