use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::stats_control_trace_loader::{
    refresh_control_trace_state, render_control_trace_source_summary,
};
use super::view_state::{ControlTraceKindFilter, ControlTraceRecordState};
use super::*;

pub(super) fn render_control_trace_panel(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let current_signature = ctx.proxy.control_trace_source_signature();
    if ctx.view.stats.control_trace_last_loaded_ms.is_none()
        || ctx.view.stats.control_trace_loaded_limit != ctx.view.stats.control_trace_limit
        || ctx.view.stats.control_trace_loaded_signature != current_signature
    {
        refresh_control_trace_state(&mut ctx.view.stats, ctx.lang, ctx.proxy, ctx.rt);
    }

    console_section(
        ui,
        pick(ctx.lang, "控制链事件", "Control trace"),
        ConsoleTone::Neutral,
        |ui| {
            render_control_trace_source_summary(ui, ctx.lang, &ctx.view.stats);
            if let Some(loaded_at) = ctx.view.stats.control_trace_last_loaded_ms {
                ui.small(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近加载", "Last loaded"),
                    format_age(now_ms(), Some(loaded_at))
                ));
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let refresh_clicked = ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked();

                ui.label(pick(ctx.lang, "最近条数", "Recent rows"));
                let mut limit = ctx.view.stats.control_trace_limit as u64;
                if ui
                    .add(egui::DragValue::new(&mut limit).range(20..=400).speed(1))
                    .changed()
                {
                    ctx.view.stats.control_trace_limit = limit as usize;
                }

                egui::ComboBox::from_id_salt("stats_control_trace_kind")
                    .selected_text(control_trace_kind_label(
                        ctx.lang,
                        ctx.view.stats.control_trace_kind,
                    ))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut ctx.view.stats.control_trace_kind,
                            ControlTraceKindFilter::All,
                            control_trace_kind_label(ctx.lang, ControlTraceKindFilter::All),
                        );
                        ui.selectable_value(
                            &mut ctx.view.stats.control_trace_kind,
                            ControlTraceKindFilter::RequestCompleted,
                            control_trace_kind_label(
                                ctx.lang,
                                ControlTraceKindFilter::RequestCompleted,
                            ),
                        );
                        ui.selectable_value(
                            &mut ctx.view.stats.control_trace_kind,
                            ControlTraceKindFilter::RetryTrace,
                            control_trace_kind_label(ctx.lang, ControlTraceKindFilter::RetryTrace),
                        );
                    });

                ui.label(pick(ctx.lang, "搜索", "Search"));
                ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.stats.control_trace_query)
                        .desired_width(220.0)
                        .hint_text(pick(
                            ctx.lang,
                            "trace_id / event / station / provider",
                            "trace_id / event / station / provider",
                        )),
                );

                if refresh_clicked {
                    refresh_control_trace_state(&mut ctx.view.stats, ctx.lang, ctx.proxy, ctx.rt);
                }
            });

            if let Some(err) = ctx.view.stats.control_trace_last_error.as_deref() {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }

            let filtered = ctx
                .view
                .stats
                .control_trace_entries
                .iter()
                .filter(|record| {
                    control_trace_kind_matches(ctx.view.stats.control_trace_kind, record)
                })
                .filter(|record| {
                    control_trace_record_matches_query(
                        record,
                        ctx.view.stats.control_trace_query.as_str(),
                    )
                })
                .take(ctx.view.stats.control_trace_limit)
                .cloned()
                .collect::<Vec<_>>();

            ui.add_space(6.0);
            if filtered.is_empty() {
                console_note(
                    ui,
                    if ctx.view.stats.control_trace_entries.is_empty() {
                        pick(
                            ctx.lang,
                            "当前没有可见的控制链事件；先通过代理发送请求，或确认本机 logs/control_trace.jsonl 已生成。",
                            "No control-trace events are visible yet. Send a request through the proxy first, or confirm logs/control_trace.jsonl exists on this device.",
                        )
                    } else {
                        pick(
                            ctx.lang,
                            "当前过滤条件没有匹配到控制链事件。",
                            "The current filters do not match any control-trace events.",
                        )
                    },
                );
                return;
            }

            egui::ScrollArea::vertical()
                .id_salt("stats_control_trace_scroll")
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Grid::new("stats_control_trace_grid")
                        .num_columns(5)
                        .spacing([12.0, 6.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong(pick(ctx.lang, "时间", "Time"));
                            ui.strong(pick(ctx.lang, "类型", "Kind"));
                            ui.strong("trace");
                            ui.strong(pick(ctx.lang, "事件", "Event"));
                            ui.strong(pick(ctx.lang, "摘要", "Summary"));
                            ui.end_row();

                            for record in filtered {
                                ui.small(format_age(now_ms(), Some(record.ts_ms)));
                                ui.monospace(record.kind.as_str());
                                ui.monospace(
                                    record
                                        .trace_id
                                        .clone()
                                        .or_else(|| {
                                            record.request_id.map(|value| value.to_string())
                                        })
                                        .unwrap_or_else(|| "-".to_string()),
                                );
                                ui.small(record.event.clone().unwrap_or_else(|| "-".to_string()));
                                ui.small(record.summary);
                                ui.end_row();
                            }
                        });
                });
        },
    );
}

fn control_trace_kind_label(lang: Language, kind: ControlTraceKindFilter) -> &'static str {
    match kind {
        ControlTraceKindFilter::All => pick(lang, "全部", "All"),
        ControlTraceKindFilter::RequestCompleted => pick(lang, "请求完成", "Request completed"),
        ControlTraceKindFilter::RetryTrace => pick(lang, "重试/路由", "Retry / routing"),
    }
}

fn control_trace_kind_matches(
    kind: ControlTraceKindFilter,
    record: &ControlTraceRecordState,
) -> bool {
    match kind {
        ControlTraceKindFilter::All => true,
        ControlTraceKindFilter::RequestCompleted => record.kind == "request_completed",
        ControlTraceKindFilter::RetryTrace => record.kind == "retry_trace",
    }
}

fn control_trace_record_matches_query(record: &ControlTraceRecordState, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    for candidate in [
        Some(record.kind.as_str()),
        record.service.as_deref(),
        record.trace_id.as_deref(),
        record.event.as_deref(),
        Some(record.summary.as_str()),
    ] {
        if let Some(candidate) = candidate
            && !candidate.is_empty()
            && candidate.to_ascii_lowercase().contains(q.as_str())
        {
            return true;
        }
    }
    record
        .request_id
        .map(|value| value.to_string())
        .is_some_and(|value| value.contains(q.as_str()))
}
