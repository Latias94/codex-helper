use super::*;

pub(super) fn render_balance_overview(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: Option<&crate::gui::proxy_control::GuiRuntimeSnapshot>,
    usage_balance: Option<&crate::usage_balance::UsageBalanceView>,
) {
    ui.separator();
    ui.label(pick(ctx.lang, "余额 / 配额", "Balance / quota"));
    render_balance_refresh_controls(ui, ctx);

    let Some(_snapshot) = snapshot else {
        ui.label(pick(
            ctx.lang,
            "(代理未运行，暂无余额/配额数据)",
            "(proxy is not running; no balance/quota data)",
        ));
        return;
    };
    let Some(view) = usage_balance else {
        ui.label(pick(
            ctx.lang,
            "(无余额/配额数据)",
            "(no balance/quota data)",
        ));
        return;
    };

    ui.label(format!(
        "providers={}  endpoints={}  snapshots={}  {}",
        view.provider_rows.len(),
        view.endpoint_rows.len(),
        view.refresh_status.total_snapshots,
        format_balance_counts(ctx.lang, &view.totals.balance_status_counts)
    ));

    egui::ScrollArea::vertical()
        .id_salt("stats_balance_provider_scroll")
        .max_height(300.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_balance_provider_grid")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "Provider", "Provider"));
                    ui.label(pick(ctx.lang, "状态", "Status"));
                    ui.label(pick(ctx.lang, "用量", "Usage"));
                    ui.label(pick(ctx.lang, "余额 / 配额", "Balance / quota"));
                    ui.label(pick(ctx.lang, "路由", "Route"));
                    ui.end_row();

                    for row in &view.provider_rows {
                        ui.label(shorten(&row.provider_id, 24));
                        ui.label(format_balance_status(ctx.lang, row.balance_status));
                        ui.label(format!(
                            "req={} tok={} cost={}",
                            row.usage.requests_total,
                            tokens_short(row.usage.usage.total_tokens),
                            row.cost_display
                        ));
                        ui.label(shorten(&format_provider_balance(row), 96));
                        ui.label(shorten(&format_provider_route(ctx.lang, row), 64));
                        ui.end_row();
                    }
                });
        });

    ui.add_space(8.0);
    ui.label(pick(
        ctx.lang,
        "Endpoints / 最近样本",
        "Endpoints / recent sample",
    ));
    egui::ScrollArea::vertical()
        .id_salt("stats_balance_endpoint_scroll")
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_balance_endpoint_grid")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "Provider", "Provider"));
                    ui.label(pick(ctx.lang, "Endpoint", "Endpoint"));
                    ui.label(pick(ctx.lang, "状态", "Status"));
                    ui.label(pick(ctx.lang, "用量", "Usage"));
                    ui.label(pick(ctx.lang, "路由", "Route"));
                    ui.end_row();

                    for row in view.endpoint_rows.iter().take(80) {
                        ui.label(shorten(&row.provider_id, 20));
                        ui.label(shorten(
                            row.base_url.as_deref().unwrap_or(row.endpoint_id.as_str()),
                            36,
                        ));
                        ui.label(format_balance_status(ctx.lang, row.balance_status));
                        ui.label(format!(
                            "req={} err={} tok={}",
                            row.usage.requests_total,
                            row.usage.requests_error,
                            tokens_short(row.usage.usage.total_tokens)
                        ));
                        ui.label(shorten(&format_endpoint_route(ctx.lang, row), 48));
                        ui.end_row();
                    }
                });
        });
}

fn render_balance_refresh_controls(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let status = ctx.proxy.provider_balance_refresh_status().clone();
    let can_refresh = ctx.proxy.supports_provider_balance_refresh();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                can_refresh && !status.refreshing,
                egui::Button::new(pick(ctx.lang, "刷新余额", "Refresh balances")),
            )
            .clicked()
        {
            match ctx
                .proxy
                .request_provider_balance_refresh(ctx.rt, None, None)
            {
                Ok(true) => {
                    *ctx.last_info = Some(
                        pick(ctx.lang, "余额刷新已开始", "Balance refresh started").to_string(),
                    );
                }
                Ok(false) => {
                    *ctx.last_info = Some(
                        pick(ctx.lang, "余额刷新进行中", "Balance refresh in progress").to_string(),
                    );
                }
                Err(err) => {
                    *ctx.last_error = Some(format!("balance refresh failed: {err}"));
                }
            }
        }

        if status.refreshing {
            ui.small(pick(ctx.lang, "刷新中...", "Refreshing..."));
        } else if !can_refresh {
            ui.small(pick(
                ctx.lang,
                "当前代理不支持刷新余额",
                "Current proxy does not support balance refresh",
            ));
        } else if let Some(err) = status.last_error.as_deref() {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), shorten(err, 72));
        } else if let Some(msg) = status.last_message.as_deref() {
            ui.small(format!("last: {}", shorten(msg, 72)));
        }
    });
}

fn format_provider_balance(row: &crate::usage_balance::UsageBalanceProviderRow) -> String {
    let mut parts = Vec::new();
    if let Some(primary) = row.primary_balance.as_ref() {
        parts.push(primary.amount_summary.clone());
    }
    if let Some(error) = row.latest_balance_error.as_deref()
        && !error.trim().is_empty()
    {
        parts.push(format!("lookup_failed={}", shorten(error, 48)));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("  ")
    }
}

fn format_provider_route(
    lang: Language,
    row: &crate::usage_balance::UsageBalanceProviderRow,
) -> String {
    if row.routing.selected {
        return row
            .routing
            .selected_endpoint_id
            .as_deref()
            .map(|endpoint| format!("{} {endpoint}", pick(lang, "选中", "selected")))
            .unwrap_or_else(|| pick(lang, "选中", "selected").to_string());
    }
    if !row.routing.skip_reasons.is_empty() {
        return row.routing.skip_reasons.join(",");
    }
    if row.routing.candidate_count > 0 {
        return pick(lang, "候选", "candidate").to_string();
    }
    "-".to_string()
}

fn format_endpoint_route(
    lang: Language,
    row: &crate::usage_balance::UsageBalanceEndpointRow,
) -> String {
    if row.route_selected {
        pick(lang, "选中", "selected").to_string()
    } else if row.route_skip_reasons.is_empty() {
        "-".to_string()
    } else {
        row.route_skip_reasons.join(",")
    }
}

fn format_balance_status(
    lang: Language,
    status: crate::usage_balance::UsageBalanceStatus,
) -> &'static str {
    match status {
        crate::usage_balance::UsageBalanceStatus::Ok => "ok",
        crate::usage_balance::UsageBalanceStatus::Unlimited => pick(lang, "不限量", "unlimited"),
        crate::usage_balance::UsageBalanceStatus::Exhausted => pick(lang, "耗尽", "exhausted"),
        crate::usage_balance::UsageBalanceStatus::Stale => pick(lang, "过期", "stale"),
        crate::usage_balance::UsageBalanceStatus::Error => pick(lang, "错误", "error"),
        crate::usage_balance::UsageBalanceStatus::Unknown => pick(lang, "未知", "unknown"),
    }
}

fn format_balance_counts(
    lang: Language,
    counts: &crate::usage_balance::UsageBalanceStatusCounts,
) -> String {
    let mut parts = Vec::new();
    if counts.ok > 0 {
        parts.push(format!("ok={}", counts.ok));
    }
    if counts.unlimited > 0 {
        parts.push(format!(
            "{}={}",
            pick(lang, "不限量", "unlimited"),
            counts.unlimited
        ));
    }
    if counts.exhausted > 0 {
        parts.push(format!(
            "{}={}",
            pick(lang, "耗尽", "exhausted"),
            counts.exhausted
        ));
    }
    if counts.stale > 0 {
        parts.push(format!("stale={}", counts.stale));
    }
    if counts.error > 0 {
        parts.push(format!("error={}", counts.error));
    }
    if counts.unknown > 0 {
        parts.push(format!("unknown={}", counts.unknown));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("  ")
    }
}

fn tokens_short(v: i64) -> String {
    let v = v.max(0) as u64;
    if v >= 1_000_000_000 {
        format!("{:.1}b", (v as f64) / 1_000_000_000.0)
    } else if v >= 1_000_000 {
        format!("{:.1}m", (v as f64) / 1_000_000.0)
    } else if v >= 1_000 {
        format!("{:.1}k", (v as f64) / 1_000.0)
    } else {
        v.to_string()
    }
}
