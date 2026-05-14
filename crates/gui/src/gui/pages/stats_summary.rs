use super::*;

pub(super) fn render_stats_summary(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: Option<&crate::gui::proxy_control::GuiRuntimeSnapshot>,
    usage_balance: Option<&crate::usage_balance::UsageBalanceView>,
) {
    let Some(snapshot) = snapshot else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看用量。",
            "No proxy is running or attached. Start or attach on Overview to view usage.",
        ));
        return;
    };

    let rollup = &snapshot.usage_rollup;
    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;

    ui.separator();

    ui.label(format!(
        "{}: {}  {}: {}",
        pick(ctx.lang, "模式", "Mode"),
        match snapshot.kind {
            ProxyModeKind::Running => pick(ctx.lang, "运行中", "Running"),
            ProxyModeKind::Attached => pick(ctx.lang, "已附着", "Attached"),
            _ => pick(ctx.lang, "未知", "Unknown"),
        },
        pick(ctx.lang, "服务", "Service"),
        snapshot.service_name.as_deref().unwrap_or("-")
    ));

    ui.add_space(8.0);

    egui::Grid::new("stats_kpis_grid")
        .striped(true)
        .show(ui, |ui| {
            let window = usage_balance.map(|view| &view.totals).map(|totals| {
                (
                    totals.requests_total,
                    totals.requests_error,
                    totals.success_per_mille,
                    totals.input_tokens,
                    totals.output_tokens,
                    totals.reasoning_output_tokens,
                    totals.total_tokens,
                    totals.cached_input_tokens,
                    totals.cost_display.clone(),
                )
            });
            let window_bucket = &rollup.window;
            let (
                requests_total,
                requests_error,
                success_per_mille,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                total_tokens,
                cached_input_tokens,
                cost_display,
            ) = window.unwrap_or_else(|| {
                (
                    window_bucket.requests_total,
                    window_bucket.requests_error,
                    window_bucket
                        .requests_total
                        .saturating_sub(window_bucket.requests_error)
                        .saturating_mul(1000)
                        .checked_div(window_bucket.requests_total)
                        .map(|value| value.min(1000) as u16),
                    window_bucket.usage.input_tokens,
                    window_bucket.usage.output_tokens,
                    window_bucket.usage.reasoning_output_tokens_total(),
                    window_bucket.usage.total_tokens,
                    window_bucket.usage.cache_read_tokens_total(),
                    window_bucket.cost.display_total_with_confidence(),
                )
            });
            ui.label(pick(ctx.lang, "请求(窗口)", "Requests (window)"));
            ui.label(format!(
                "total={}  errors={}  ok%={}",
                requests_total,
                requests_error,
                success_per_mille
                    .map(|value| format!("{:.1}%", f64::from(value) / 10.0))
                    .unwrap_or_else(|| "-".to_string())
            ));
            ui.end_row();

            ui.label(pick(ctx.lang, "Tokens(窗口)", "Tokens (window)"));
            ui.label(format!(
                "in={}  out={}  rsn={}  ttl={}",
                tokens_short(input_tokens),
                tokens_short(output_tokens),
                tokens_short(reasoning_tokens),
                tokens_short(total_tokens)
            ));
            ui.end_row();

            if cached_input_tokens > 0 {
                ui.label(pick(ctx.lang, "Cache Tokens", "Cache tokens"));
                ui.label(format!(
                    "read={}  create={}",
                    tokens_short(cached_input_tokens),
                    tokens_short(window_bucket.usage.cache_creation_tokens_total())
                ));
                ui.end_row();
            }

            ui.label(pick(ctx.lang, "成本", "Cost"));
            ui.label(cost_display);
            ui.end_row();

            if let Some(view) = usage_balance {
                ui.label(pick(ctx.lang, "余额状态", "Balance states"));
                ui.label(format_balance_status_counts(
                    ctx.lang,
                    &view.totals.balance_status_counts,
                ));
                ui.end_row();
            }

            ui.label(pick(ctx.lang, "窗口(5m)", "Window (5m)"));
            ui.label(format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                fmt_pct(s5.ok_2xx, s5.total),
                s5.p95_ms
                    .map(format_duration_ms)
                    .unwrap_or_else(|| "-".to_string()),
                s5.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.err_429,
                s5.err_5xx,
                s5.total
            ));
            ui.end_row();

            ui.label(pick(ctx.lang, "窗口(1h)", "Window (1h)"));
            ui.label(format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                fmt_pct(s1.ok_2xx, s1.total),
                s1.p95_ms
                    .map(format_duration_ms)
                    .unwrap_or_else(|| "-".to_string()),
                s1.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.err_429,
                s1.err_5xx,
                s1.total
            ));
            ui.end_row();
        });

    ui.add_space(10.0);
    ui.separator();
    ui.label(pick(
        ctx.lang,
        "Tokens / day（当前窗口）",
        "Tokens / day (current window)",
    ));

    let now_day = (now_ms() / 86_400_000) as i32;
    let mut by_day = rollup.by_day.clone();
    if by_day.len() > 14 {
        by_day = by_day[by_day.len().saturating_sub(14)..].to_vec();
    }
    let max_tok = by_day
        .iter()
        .map(|(_, bucket)| bucket.usage.total_tokens.max(0) as u64)
        .max()
        .unwrap_or(0);

    egui::Grid::new("stats_by_day_grid")
        .striped(true)
        .show(ui, |ui| {
            ui.label(pick(ctx.lang, "天", "Day"));
            ui.label(pick(ctx.lang, "Tokens", "Tokens"));
            ui.label(pick(ctx.lang, "条", "Requests"));
            ui.end_row();

            for (day, bucket) in by_day.iter() {
                let delta = day - now_day;
                let label = if delta == 0 {
                    "d+0".to_string()
                } else if delta > 0 {
                    format!("d+{delta}")
                } else {
                    format!("d{delta}")
                };
                let tokens = bucket.usage.total_tokens.max(0) as u64;
                let bar_len = if max_tok == 0 {
                    0
                } else {
                    ((tokens as f64) * 24.0 / (max_tok as f64)).round() as usize
                };
                let bar = "▮".repeat(bar_len);
                ui.label(label);
                ui.label(format!(
                    "{}  {}",
                    tokens_short(bucket.usage.total_tokens),
                    bar
                ));
                ui.label(bucket.requests_total.to_string());
                ui.end_row();
            }
        });

    ui.add_space(10.0);
    ui.separator();
    ui.label(pick(
        ctx.lang,
        "Top stations/providers（当前窗口）",
        "Top stations/providers (current window)",
    ));

    ui.columns(2, |cols| {
        cols[0].label(pick(ctx.lang, "Stations", "Stations"));
        egui::ScrollArea::vertical()
            .id_salt("stats_top_configs_scroll")
            .max_height(220.0)
            .show(&mut cols[0], |ui| {
                for (name, bucket) in rollup.by_config.iter().take(30) {
                    ui.label(format!(
                        "{}  tok={}  n={}  err={}",
                        shorten(name, 28),
                        tokens_short(bucket.usage.total_tokens),
                        bucket.requests_total,
                        bucket.requests_error
                    ));
                }
            });

        cols[1].label(pick(ctx.lang, "Providers", "Providers"));
        egui::ScrollArea::vertical()
            .id_salt("stats_top_providers_scroll")
            .max_height(220.0)
            .show(&mut cols[1], |ui| {
                for (name, bucket) in rollup.by_provider.iter().take(30) {
                    ui.label(format!(
                        "{}  tok={}  n={}  err={}",
                        shorten(name, 28),
                        tokens_short(bucket.usage.total_tokens),
                        bucket.requests_total,
                        bucket.requests_error
                    ));
                }
            });
    });
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

fn fmt_pct(ok: usize, total: usize) -> String {
    if total == 0 {
        return "-".to_string();
    }
    format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
}

fn format_balance_status_counts(
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
        parts.push(format!("{}={}", pick(lang, "过期", "stale"), counts.stale));
    }
    if counts.error > 0 {
        parts.push(format!("{}={}", pick(lang, "错误", "error"), counts.error));
    }
    if counts.unknown > 0 {
        parts.push(format!(
            "{}={}",
            pick(lang, "未知", "unknown"),
            counts.unknown
        ));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join("  ")
    }
}
