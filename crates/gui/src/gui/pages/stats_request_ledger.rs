use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::view_state::{
    RequestLedgerSummaryFilterParseError, RequestLedgerSummaryFilterState,
    RequestLedgerSummaryLoad, StatsViewState,
};
use super::*;
use crate::request_ledger::{RequestLogFilters, RequestUsageSummaryGroup, RequestUsageSummaryRow};
use std::sync::mpsc::TryRecvError;

pub(super) fn render_request_ledger_summary_panel(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let current_signature = ctx.proxy.request_ledger_summary_source_signature();

    console_section(
        ui,
        pick(ctx.lang, "请求日志汇总", "Request ledger summary"),
        ConsoleTone::Neutral,
        |ui| {
            if current_signature.is_none() {
                console_note(
                    ui,
                    pick(
                        ctx.lang,
                        "当前代理没有可用的请求日志汇总来源；本机运行模式可读 requests.jsonl，附着模式需要目标代理暴露 request-ledger/summary API。",
                        "No request-ledger summary source is available. Local running mode can read requests.jsonl; attached mode requires the target proxy to expose the request-ledger/summary API.",
                    ),
                );
                return;
            }

            render_source_summary(ui, ctx.lang, &ctx.view.stats);
            if let Some(loaded_at) = ctx.view.stats.request_ledger_summary_loaded_at_ms {
                ui.small(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近加载", "Last loaded"),
                    format_age(now_ms(), Some(loaded_at))
                ));
            }
            if ctx.view.stats.request_ledger_summary_load.is_some() {
                ui.small(pick(ctx.lang, "正在加载...", "Loading..."));
            }
            render_active_filter_summary(
                ui,
                ctx.lang,
                &ctx.view.stats.request_ledger_summary_filters,
            );

            ui.add_space(6.0);
            let refresh_clicked = render_summary_controls(ui, ctx.lang, &mut ctx.view.stats);
            let current_filters = match ctx
                .view
                .stats
                .request_ledger_summary_filters
                .to_request_log_filters()
            {
                Ok(filters) => filters,
                Err(error) => {
                    render_filter_parse_error(ui, ctx.lang, error);
                    return;
                }
            };

            let should_auto_refresh = ctx.view.stats.request_ledger_summary_load.is_none()
                && (ctx.view.stats.request_ledger_summary_requested_signature != current_signature
                    || ctx.view.stats.request_ledger_summary_requested_group
                        != ctx.view.stats.request_ledger_summary_group
                    || ctx.view.stats.request_ledger_summary_requested_limit
                        != ctx.view.stats.request_ledger_summary_limit);
            if refresh_clicked {
                refresh_request_ledger_summary(ctx, true, current_filters.clone());
            } else if should_auto_refresh {
                refresh_request_ledger_summary(ctx, false, current_filters.clone());
            }

            if ctx.view.stats.request_ledger_summary_loaded_at_ms.is_some()
                && ctx.view.stats.request_ledger_summary_last_error.is_some()
                && ctx.view.stats.request_ledger_summary_load.is_none()
                && (ctx.view.stats.request_ledger_summary_loaded_signature
                    != ctx.view.stats.request_ledger_summary_requested_signature
                    || ctx.view.stats.request_ledger_summary_loaded_group
                        != ctx.view.stats.request_ledger_summary_requested_group
                    || ctx.view.stats.request_ledger_summary_loaded_limit
                        != ctx.view.stats.request_ledger_summary_requested_limit)
            {
                ui.add_space(6.0);
                console_note(
                    ui,
                    pick(
                        ctx.lang,
                        "当前显示的是上次成功结果；最新刷新失败。",
                        "Showing the last successful result; the latest refresh failed.",
                    ),
                );
            }

            if ctx.view.stats.request_ledger_summary_loaded_filters != current_filters {
                ui.add_space(6.0);
                console_note(
                    ui,
                    pick(
                        ctx.lang,
                        "筛选条件已更改；点击刷新后再查看汇总。",
                        "Filters changed; refresh before trusting the summary.",
                    ),
                );
            }

            if let Some(err) = ctx.view.stats.request_ledger_summary_last_error.as_deref() {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }

            ui.add_space(6.0);
            if ctx.view.stats.request_ledger_summary_rows.is_empty() {
                console_note(
                    ui,
                    pick(
                        ctx.lang,
                        "当前请求日志汇总为空；先通过代理完成请求，或确认 requests.jsonl 已生成。",
                        "The request-ledger summary is empty. Complete requests through the proxy first, or confirm requests.jsonl exists.",
                    ),
                );
                return;
            }

            render_summary_rows(ui, ctx.lang, &ctx.view.stats.request_ledger_summary_rows);
        },
    );
}

pub(super) fn poll_request_ledger_summary_loader(ctx: &mut PageCtx<'_>) {
    let current_signature = ctx.proxy.request_ledger_summary_source_signature();
    let current_group = ctx.view.stats.request_ledger_summary_group;
    let current_limit = ctx.view.stats.request_ledger_summary_limit.clamp(1, 100);
    let Some(load) = ctx.view.stats.request_ledger_summary_load.as_mut() else {
        return;
    };

    match load.rx.try_recv() {
        Ok((seq, result)) => {
            if seq != load.seq
                || load.source_signature != current_signature
                || load.group != current_group
                || load.limit != current_limit
            {
                ctx.view.stats.request_ledger_summary_load = None;
                return;
            }

            let limit = load.limit;
            let group = load.group;
            let filters = load.filters.clone();
            ctx.view.stats.request_ledger_summary_load = None;
            match result {
                Ok(result) => {
                    apply_request_ledger_summary_source_state(&mut ctx.view.stats, &result.source);
                    ctx.view.stats.request_ledger_summary_loaded_signature =
                        Some(result.source.signature());
                    ctx.view.stats.request_ledger_summary_loaded_group = group;
                    ctx.view.stats.request_ledger_summary_loaded_limit = limit;
                    ctx.view.stats.request_ledger_summary_loaded_filters = filters;
                    ctx.view.stats.request_ledger_summary_rows = result.rows;
                    ctx.view.stats.request_ledger_summary_loaded_at_ms = Some(now_ms());
                    ctx.view.stats.request_ledger_summary_last_error = None;
                }
                Err(err) => {
                    ctx.view.stats.request_ledger_summary_last_error = Some(err.to_string());
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.stats.request_ledger_summary_load = None;
        }
    }
}

fn cancel_request_ledger_summary_load(state: &mut StatsViewState) {
    if let Some(load) = state.request_ledger_summary_load.take() {
        load.join.abort();
    }
}

fn refresh_request_ledger_summary(ctx: &mut PageCtx<'_>, force: bool, filters: RequestLogFilters) {
    let source = ctx.proxy.request_ledger_summary_source();
    if source.is_none() {
        ctx.view.stats.request_ledger_summary_last_error =
            Some("request ledger summary source is unavailable for this proxy".to_string());
        return;
    }

    let source_signature = source.as_ref().map(|source| source.signature());
    let source_detail = source.as_ref().map(|source| source.display_detail());
    let limit = ctx.view.stats.request_ledger_summary_limit.clamp(1, 100);
    ctx.view.stats.request_ledger_summary_limit = limit;
    let group = ctx.view.stats.request_ledger_summary_group;
    apply_request_ledger_summary_source_state(&mut ctx.view.stats, source.as_ref().unwrap());

    if !force
        && ctx.view.stats.request_ledger_summary_load.is_none()
        && ctx.view.stats.request_ledger_summary_requested_signature == source_signature
        && ctx.view.stats.request_ledger_summary_requested_group == group
        && ctx.view.stats.request_ledger_summary_requested_limit == limit
    {
        return;
    }

    if force {
        cancel_request_ledger_summary_load(&mut ctx.view.stats);
    } else if ctx.view.stats.request_ledger_summary_load.is_some() {
        return;
    }

    ctx.view.stats.request_ledger_summary_requested_signature = source_signature.clone();
    ctx.view.stats.request_ledger_summary_requested_group = group;
    ctx.view.stats.request_ledger_summary_requested_limit = limit;
    ctx.view.stats.request_ledger_summary_requested_filters = filters.clone();
    ctx.view.stats.request_ledger_summary_source_detail = source_detail;
    ctx.view.stats.request_ledger_summary_last_error = None;
    let requested_filters = ctx
        .view
        .stats
        .request_ledger_summary_requested_filters
        .clone();

    let future = match ctx
        .proxy
        .read_request_ledger_summary_task(group, limit, filters)
    {
        Ok(future) => future,
        Err(err) => {
            ctx.view.stats.request_ledger_summary_last_error = Some(err.to_string());
            return;
        }
    };

    ctx.view.stats.request_ledger_summary_load_seq = ctx
        .view
        .stats
        .request_ledger_summary_load_seq
        .saturating_add(1);
    let seq = ctx.view.stats.request_ledger_summary_load_seq;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = future.await;
        let _ = tx.send((seq, result));
    });
    ctx.view.stats.request_ledger_summary_load = Some(RequestLedgerSummaryLoad {
        seq,
        source_signature,
        group,
        limit,
        filters: requested_filters,
        rx,
        join,
    });
}

fn apply_request_ledger_summary_source_state(
    state: &mut StatsViewState,
    source: &crate::gui::proxy_control::RequestLedgerDataSource,
) {
    state.request_ledger_summary_source_detail = Some(source.display_detail());
}

fn render_summary_controls(ui: &mut egui::Ui, lang: Language, state: &mut StatsViewState) -> bool {
    let mut refresh_clicked = false;
    ui.horizontal_wrapped(|ui| {
        refresh_clicked = ui.button(pick(lang, "刷新", "Refresh")).clicked();

        egui::ComboBox::from_id_salt("stats_request_ledger_summary_group")
            .selected_text(summary_group_label(
                lang,
                state.request_ledger_summary_group,
            ))
            .show_ui(ui, |ui| {
                for group in [
                    RequestUsageSummaryGroup::Station,
                    RequestUsageSummaryGroup::Provider,
                    RequestUsageSummaryGroup::Model,
                    RequestUsageSummaryGroup::Session,
                ] {
                    ui.selectable_value(
                        &mut state.request_ledger_summary_group,
                        group,
                        summary_group_label(lang, group),
                    );
                }
            });

        ui.label(pick(lang, "行数", "Rows"));
        let mut limit = state.request_ledger_summary_limit as u64;
        if ui
            .add(egui::DragValue::new(&mut limit).range(1..=100).speed(1))
            .changed()
        {
            state.request_ledger_summary_limit = limit as usize;
        }

        if filter_state_has_values(&state.request_ledger_summary_filters)
            && ui.button(pick(lang, "清除筛选", "Clear filters")).clicked()
        {
            state.request_ledger_summary_filters.clear();
        }
    });

    ui.add_space(4.0);
    render_summary_filter_controls(ui, lang, &mut state.request_ledger_summary_filters);

    refresh_clicked
}

fn render_summary_filter_controls(
    ui: &mut egui::Ui,
    lang: Language,
    filters: &mut RequestLedgerSummaryFilterState,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("session");
        ui.add(
            egui::TextEdit::singleline(&mut filters.session)
                .desired_width(110.0)
                .hint_text(pick(lang, "全部", "any")),
        );
        ui.label(pick(lang, "模型", "Model"));
        ui.add(
            egui::TextEdit::singleline(&mut filters.model)
                .desired_width(110.0)
                .hint_text(pick(lang, "全部", "any")),
        );
        ui.label(pick(lang, "站点", "Station"));
        ui.add(
            egui::TextEdit::singleline(&mut filters.station)
                .desired_width(100.0)
                .hint_text(pick(lang, "全部", "any")),
        );
        ui.label(pick(lang, "提供商", "Provider"));
        ui.add(
            egui::TextEdit::singleline(&mut filters.provider)
                .desired_width(100.0)
                .hint_text(pick(lang, "全部", "any")),
        );
        ui.label("status >=");
        ui.add(
            egui::TextEdit::singleline(&mut filters.status_min)
                .desired_width(58.0)
                .hint_text(pick(lang, "不限", "any")),
        );
        ui.label("status <=");
        ui.add(
            egui::TextEdit::singleline(&mut filters.status_max)
                .desired_width(58.0)
                .hint_text(pick(lang, "不限", "any")),
        );
        ui.checkbox(&mut filters.fast_only, pick(lang, "fast", "fast"));
        ui.checkbox(&mut filters.retried_only, pick(lang, "重试", "retried"));
    });
}

fn render_active_filter_summary(
    ui: &mut egui::Ui,
    lang: Language,
    filters: &RequestLedgerSummaryFilterState,
) {
    let parts = active_filter_parts(filters);
    let value = if parts.is_empty() {
        pick(lang, "全部请求", "all requests").to_string()
    } else {
        parts.join("  ")
    };
    ui.small(format!("{}: {value}", pick(lang, "筛选", "Filters")));
}

fn active_filter_parts(filters: &RequestLedgerSummaryFilterState) -> Vec<String> {
    let mut parts = Vec::new();
    push_text_filter_part(&mut parts, "session", &filters.session);
    push_text_filter_part(&mut parts, "model", &filters.model);
    push_text_filter_part(&mut parts, "station", &filters.station);
    push_text_filter_part(&mut parts, "provider", &filters.provider);
    let status_min = filters.status_min.trim();
    let status_max = filters.status_max.trim();
    if !status_min.is_empty() || !status_max.is_empty() {
        parts.push(format!(
            "status={}..{}",
            if status_min.is_empty() {
                "*"
            } else {
                status_min
            },
            if status_max.is_empty() {
                "*"
            } else {
                status_max
            }
        ));
    }
    if filters.fast_only {
        parts.push("fast".to_string());
    }
    if filters.retried_only {
        parts.push("retried".to_string());
    }
    parts
}

fn push_text_filter_part(parts: &mut Vec<String>, label: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        parts.push(format!("{label}={}", shorten(value, 24)));
    }
}

fn filter_state_has_values(filters: &RequestLedgerSummaryFilterState) -> bool {
    !active_filter_parts(filters).is_empty()
}

fn render_filter_parse_error(
    ui: &mut egui::Ui,
    lang: Language,
    error: RequestLedgerSummaryFilterParseError,
) {
    ui.add_space(6.0);
    ui.colored_label(
        egui::Color32::from_rgb(200, 120, 40),
        match error {
            RequestLedgerSummaryFilterParseError::StatusMin => pick(
                lang,
                "状态码下限必须是非负整数。",
                "Status lower bound must be a non-negative integer.",
            ),
            RequestLedgerSummaryFilterParseError::StatusMax => pick(
                lang,
                "状态码上限必须是非负整数。",
                "Status upper bound must be a non-negative integer.",
            ),
        },
    );
}

fn render_source_summary(ui: &mut egui::Ui, lang: Language, state: &StatsViewState) {
    ui.small(format!(
        "{}: {}",
        pick(lang, "来源", "Source"),
        state
            .request_ledger_summary_source_detail
            .as_deref()
            .unwrap_or("-")
    ));
}

fn render_summary_rows(ui: &mut egui::Ui, lang: Language, rows: &[RequestUsageSummaryRow]) {
    egui::ScrollArea::vertical()
        .id_salt("stats_request_ledger_summary_scroll")
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_request_ledger_summary_grid")
                .num_columns(8)
                .spacing([12.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.strong(pick(lang, "分组", "Group"));
                    ui.strong(pick(lang, "请求", "Requests"));
                    ui.strong("total");
                    ui.strong("input");
                    ui.strong("output");
                    ui.strong("cache read");
                    ui.strong("cache create");
                    ui.strong("avg");
                    ui.end_row();

                    for row in rows {
                        ui.small(shorten(&row.group_value, 32));
                        ui.small(row.aggregate.requests.to_string());
                        ui.small(compact_count(row.aggregate.total_tokens));
                        ui.small(compact_count(row.aggregate.input_tokens));
                        ui.small(compact_count(row.aggregate.output_tokens));
                        ui.small(compact_count(row.aggregate.cache_read_input_tokens));
                        ui.small(compact_count(row.aggregate.cache_creation_input_tokens));
                        ui.small(format_duration_ms(row.aggregate.average_duration_ms()));
                        ui.end_row();
                    }
                });
        });
}

fn summary_group_label(lang: Language, group: RequestUsageSummaryGroup) -> &'static str {
    match group {
        RequestUsageSummaryGroup::Station => pick(lang, "按站点", "By station"),
        RequestUsageSummaryGroup::Provider => pick(lang, "按提供商", "By provider"),
        RequestUsageSummaryGroup::Model => pick(lang, "按模型", "By model"),
        RequestUsageSummaryGroup::Session => pick(lang, "按会话", "By session"),
    }
}

fn compact_count(value: i64) -> String {
    let value = value.max(0) as f64;
    if value >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_group_labels_are_stable() {
        assert_eq!(
            summary_group_label(Language::En, RequestUsageSummaryGroup::Provider),
            "By provider"
        );
        assert_eq!(
            summary_group_label(Language::Zh, RequestUsageSummaryGroup::Model),
            "按模型"
        );
    }

    #[test]
    fn summary_filter_state_builds_request_log_filters() {
        let filters = RequestLedgerSummaryFilterState {
            session: " sid-1 ".to_string(),
            model: " gpt-5.4 ".to_string(),
            station: " main ".to_string(),
            provider: " relay ".to_string(),
            status_min: " 400 ".to_string(),
            status_max: " 499 ".to_string(),
            fast_only: true,
            retried_only: true,
        }
        .to_request_log_filters()
        .expect("filters");

        assert_eq!(filters.session.as_deref(), Some("sid-1"));
        assert_eq!(filters.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(filters.station.as_deref(), Some("main"));
        assert_eq!(filters.provider.as_deref(), Some("relay"));
        assert_eq!(filters.status_min, Some(400));
        assert_eq!(filters.status_max, Some(499));
        assert!(filters.fast);
        assert!(filters.retried);
    }

    #[test]
    fn summary_filter_state_rejects_invalid_status_bounds() {
        let error = RequestLedgerSummaryFilterState {
            status_min: "oops".to_string(),
            ..RequestLedgerSummaryFilterState::default()
        }
        .to_request_log_filters()
        .expect_err("parse error");

        assert_eq!(error, RequestLedgerSummaryFilterParseError::StatusMin);
    }
}
