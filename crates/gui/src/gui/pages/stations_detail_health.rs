use super::*;

pub(super) fn render_station_health_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    health: Option<&StationHealth>,
    health_status: Option<&HealthCheckStatus>,
) {
    ui.label(pick(ctx.lang, "健康检查", "Health check"));
    if let Some(status) = health_status {
        ui.label(format!(
            "status: {}/{} ok={} err={} cancel={} done={}",
            status.completed,
            status.total,
            status.ok,
            status.err,
            status.cancel_requested,
            status.done
        ));
        if let Some(err) = status.last_error.as_deref() {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }
    } else {
        ui.label(pick(ctx.lang, "(无状态)", "(no status)"));
    }
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "探测当前", "Probe selected"))
            .clicked()
        {
            match ctx.proxy.probe_station(ctx.rt, cfg.name.clone()) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已开始探测", "Probe started").to_string());
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("station probe failed: {e}"));
                }
            }
        }
        if ui
            .button(pick(ctx.lang, "取消当前", "Cancel selected"))
            .clicked()
        {
            match ctx
                .proxy
                .cancel_health_checks(ctx.rt, false, vec![cfg.name.clone()])
            {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                }
            }
        }
        if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
            match ctx.proxy.start_health_checks(ctx.rt, true, Vec::new()) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已开始健康检查", "Health check started").to_string());
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("health check start failed: {e}"));
                }
            }
        }
        if ui
            .button(pick(ctx.lang, "取消全部", "Cancel all"))
            .clicked()
        {
            match ctx.proxy.cancel_health_checks(ctx.rt, true, Vec::new()) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                }
            }
        }
    });

    if let Some(health) = health {
        ui.add_space(6.0);
        ui.label(format!(
            "{}: {}  upstreams={}",
            pick(ctx.lang, "最近检查", "Last checked"),
            health.checked_at_ms,
            health.upstreams.len()
        ));
        egui::ScrollArea::vertical()
            .id_salt(("stations_health_upstreams_scroll", cfg.name.as_str()))
            .max_height(140.0)
            .show(ui, |ui| {
                let max = 12usize;
                for upstream in health.upstreams.iter().rev().take(max) {
                    let ok = upstream
                        .ok
                        .map(|value| if value { "ok" } else { "err" })
                        .unwrap_or("-");
                    let status_code = upstream
                        .status_code
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let latency = upstream
                        .latency_ms
                        .map(|value| format!("{value}ms"))
                        .unwrap_or_else(|| "-".to_string());
                    let error = upstream
                        .error
                        .as_deref()
                        .map(|value| shorten(value, 60))
                        .unwrap_or_else(|| "-".to_string());
                    ui.label(format!(
                        "{ok} {status_code} {latency}  {}  {error}",
                        shorten_middle(&upstream.base_url, 52)
                    ));
                }
                if health.upstreams.len() > max {
                    ui.label(format!("… +{} more", health.upstreams.len() - max));
                }
            });
    }
}

pub(super) fn render_station_breaker_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    lb: Option<&LbConfigView>,
) {
    ui.label(pick(ctx.lang, "熔断/冷却细节", "Breaker/cooldown details"));
    if let Some(lb) = lb {
        if lb.upstreams.is_empty() {
            ui.label(pick(ctx.lang, "(无上游状态)", "(no upstream state)"));
        } else {
            egui::ScrollArea::vertical()
                .id_salt(("stations_lb_scroll", cfg.name.as_str()))
                .max_height(120.0)
                .show(ui, |ui| {
                    for (idx, upstream) in lb.upstreams.iter().enumerate() {
                        let cooldown = upstream
                            .cooldown_remaining_secs
                            .map(|secs| format!("{secs}s"))
                            .unwrap_or_else(|| "-".to_string());
                        ui.label(format!(
                            "#{} fail={} cooldown={} quota_exhausted={}",
                            idx, upstream.failure_count, cooldown, upstream.usage_exhausted
                        ));
                    }
                    if let Some(last_good_index) = lb.last_good_index {
                        ui.small(format!("last_good_index={last_good_index}"));
                    }
                });
        }
    } else {
        ui.label(pick(ctx.lang, "(无熔断数据)", "(no breaker data)"));
    }
}

fn refresh_runtime_snapshot(ctx: &mut PageCtx<'_>) {
    ctx.proxy
        .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
}
