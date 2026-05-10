use super::*;

pub(super) fn format_station_balance_summary(
    balances: Option<&[ProviderBalanceSnapshot]>,
) -> String {
    let Some(balances) = balances else {
        return "unknown".to_string();
    };
    if balances.is_empty() {
        return "unknown".to_string();
    }

    let mut ok = 0usize;
    let mut exhausted = 0usize;
    let mut stale = 0usize;
    let mut error = 0usize;
    let mut unknown = 0usize;
    for balance in balances {
        match balance.status {
            BalanceSnapshotStatus::Ok => ok += 1,
            BalanceSnapshotStatus::Exhausted => exhausted += 1,
            BalanceSnapshotStatus::Stale => stale += 1,
            BalanceSnapshotStatus::Error => error += 1,
            BalanceSnapshotStatus::Unknown => unknown += 1,
        }
    }

    let primary = balances
        .iter()
        .min_by(|left, right| balance_priority(left, right));
    let headline = primary
        .map(|balance| balance_status_label(balance.status))
        .unwrap_or("unknown");
    let mut parts = vec![
        format!("status={headline}"),
        format!("rows={}", balances.len()),
    ];
    push_nonzero_count(&mut parts, "ok", ok);
    push_nonzero_count(&mut parts, "exhausted", exhausted);
    push_nonzero_count(&mut parts, "stale", stale);
    push_nonzero_count(&mut parts, "unknown", unknown + error);

    if let Some(primary) = primary {
        let target = match primary.upstream_index {
            Some(idx) => format!("{}#{}", shorten_middle(&primary.provider_id, 18), idx),
            None => shorten_middle(&primary.provider_id, 18),
        };
        parts.push(format!("primary={target}"));
        let amount = primary.amount_summary();
        if amount != "-" {
            parts.push(amount);
        }
    }

    parts.join(" ")
}

fn push_nonzero_count(parts: &mut Vec<String>, label: &str, count: usize) {
    if count > 0 {
        parts.push(format!("{label}={count}"));
    }
}

fn balance_priority(
    left: &ProviderBalanceSnapshot,
    right: &ProviderBalanceSnapshot,
) -> std::cmp::Ordering {
    balance_status_rank(left.status)
        .cmp(&balance_status_rank(right.status))
        .then_with(|| left.upstream_index.cmp(&right.upstream_index))
        .then_with(|| left.provider_id.cmp(&right.provider_id))
        .then_with(|| left.fetched_at_ms.cmp(&right.fetched_at_ms))
}

fn balance_status_rank(status: BalanceSnapshotStatus) -> u8 {
    match status {
        BalanceSnapshotStatus::Exhausted => 0,
        BalanceSnapshotStatus::Stale => 1,
        BalanceSnapshotStatus::Error | BalanceSnapshotStatus::Unknown => 2,
        BalanceSnapshotStatus::Ok => 3,
    }
}

fn balance_status_label(status: BalanceSnapshotStatus) -> &'static str {
    match status {
        BalanceSnapshotStatus::Ok => "ok",
        BalanceSnapshotStatus::Exhausted => "exhausted",
        BalanceSnapshotStatus::Stale => "stale",
        BalanceSnapshotStatus::Error | BalanceSnapshotStatus::Unknown => "unknown",
    }
}

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
                        .map(format_duration_ms)
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

pub(super) fn render_station_balance_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    balances: Option<&[ProviderBalanceSnapshot]>,
) {
    ui.label(pick(ctx.lang, "余额 / 配额", "Balance / quota"));
    if let Some(balances) = balances {
        if balances.is_empty() {
            ui.label(pick(
                ctx.lang,
                "(无余额/配额数据)",
                "(no balance/quota data)",
            ));
            return;
        }
        ui.small(format_station_balance_summary(Some(balances)));

        egui::ScrollArea::vertical()
            .id_salt(("stations_balance_scroll", cfg.name.as_str()))
            .max_height(150.0)
            .show(ui, |ui| {
                for snapshot in balances.iter().rev().take(12) {
                    let mut parts = vec![balance_status_label(snapshot.status).to_string()];
                    if let Some(plan) = snapshot.plan_name.as_deref()
                        && !plan.trim().is_empty()
                    {
                        parts.push(format!("plan={plan}"));
                    }
                    if snapshot.unlimited_quota == Some(true) {
                        parts.push("unlimited".to_string());
                    } else if snapshot.quota_period.is_some()
                        || snapshot.quota_remaining_usd.is_some()
                        || snapshot.quota_limit_usd.is_some()
                        || snapshot.quota_used_usd.is_some()
                    {
                        let quota_label = snapshot
                            .quota_period
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(|period| {
                                if period == "quota" {
                                    "quota".to_string()
                                } else {
                                    format!("{period} quota")
                                }
                            })
                            .unwrap_or_else(|| "quota".to_string());
                        parts.push(quota_label);
                        if let Some(remaining) = snapshot.quota_remaining_usd.as_deref() {
                            parts.push(format!("left=${remaining}"));
                        }
                        if let Some(limit) = snapshot.quota_limit_usd.as_deref() {
                            parts.push(format!("limit=${limit}"));
                        }
                        if let Some(used) = snapshot.quota_used_usd.as_deref() {
                            parts.push(format!("used=${used}"));
                        }
                    } else {
                        if let Some(total) = snapshot.total_balance_usd.as_deref() {
                            parts.push(format!("total=${total}"));
                        }
                        if let Some(budget) = snapshot.monthly_budget_usd.as_deref() {
                            parts.push(format!("budget=${budget}"));
                        }
                        if let Some(spent) = snapshot.monthly_spent_usd.as_deref() {
                            parts.push(format!("spent=${spent}"));
                        }
                        if let Some(used) = snapshot.total_used_usd.as_deref() {
                            parts.push(format!("used=${used}"));
                        }
                        if let Some(today) = snapshot.today_used_usd.as_deref() {
                            parts.push(format!("today=${today}"));
                        }
                        if let Some(sub) = snapshot.subscription_balance_usd.as_deref() {
                            parts.push(format!("sub=${sub}"));
                        }
                        if let Some(paygo) = snapshot.paygo_balance_usd.as_deref() {
                            parts.push(format!("paygo=${paygo}"));
                        }
                    }
                    if let Some(requests) = snapshot.total_requests {
                        parts.push(format!("req={requests}"));
                    }
                    if let Some(tokens) = snapshot.total_tokens {
                        parts.push(format!("tok={tokens}"));
                    }
                    if snapshot.stale {
                        parts.push("stale".to_string());
                    }
                    if let Some(err) = snapshot.error.as_deref()
                        && !err.trim().is_empty()
                    {
                        parts.push(format!("lookup_failed={}", shorten(err, 50)));
                    }

                    ui.label(format!(
                        "{}  {}  {}",
                        shorten_middle(&snapshot.provider_id, 18),
                        snapshot
                            .upstream_index
                            .map(|idx| format!("#{}", idx))
                            .unwrap_or_else(|| "-".to_string()),
                        parts.join(" | ")
                    ));
                }
                if balances.len() > 12 {
                    ui.label(format!("… +{} more", balances.len() - 12));
                }
            });
    } else {
        ui.label(pick(
            ctx.lang,
            "(无余额/配额数据)",
            "(no balance/quota data)",
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_balance_summary_prioritizes_problematic_primary() {
        let balances = vec![
            ProviderBalanceSnapshot {
                provider_id: "ok-provider".to_string(),
                upstream_index: Some(0),
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("3.5".to_string()),
                ..Default::default()
            },
            ProviderBalanceSnapshot {
                provider_id: "empty-provider".to_string(),
                upstream_index: Some(1),
                status: BalanceSnapshotStatus::Exhausted,
                total_balance_usd: Some("0".to_string()),
                ..Default::default()
            },
        ];

        let summary = format_station_balance_summary(Some(&balances));

        assert!(summary.contains("status=exhausted"));
        assert!(summary.contains("ok=1"));
        assert!(summary.contains("exhausted=1"));
        assert!(summary.contains("primary=empty-provider#1"));
        assert!(summary.contains("total=$0"));
    }
}
