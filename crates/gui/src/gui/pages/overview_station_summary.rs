use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentProviderHitSummary {
    provider: String,
    requests: usize,
    errors: usize,
    retry_requests: usize,
    attempts_total: u64,
}

impl RecentProviderHitSummary {
    fn avg_attempts(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.attempts_total as f64 / self.requests as f64
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RecentRetryRollup {
    retried_requests: usize,
    cross_station_failovers: usize,
    fast_mode_requests: usize,
}

fn recent_provider_hit_summaries(recent: &[FinishedRequest]) -> Vec<RecentProviderHitSummary> {
    let mut grouped = BTreeMap::<String, RecentProviderHitSummary>::new();

    for request in recent {
        let provider = request.provider_id.as_deref().unwrap_or("-").to_string();
        let entry = grouped
            .entry(provider.clone())
            .or_insert_with(|| RecentProviderHitSummary {
                provider,
                requests: 0,
                errors: 0,
                retry_requests: 0,
                attempts_total: 0,
            });
        entry.requests += 1;
        if request.status_code >= 400 {
            entry.errors += 1;
        }
        let attempts = request
            .retry
            .as_ref()
            .map(|retry| retry.attempts)
            .unwrap_or(1);
        if attempts > 1 {
            entry.retry_requests += 1;
        }
        entry.attempts_total = entry.attempts_total.saturating_add(attempts as u64);
    }

    let mut rows = grouped.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn recent_retry_rollup(recent: &[FinishedRequest]) -> RecentRetryRollup {
    let mut rollup = RecentRetryRollup::default();
    for request in recent {
        if request
            .service_tier
            .as_deref()
            .is_some_and(|tier| tier.eq_ignore_ascii_case("priority"))
        {
            rollup.fast_mode_requests += 1;
        }

        let Some(retry) = request.retry.as_ref() else {
            continue;
        };
        if retry.attempts <= 1 {
            continue;
        }
        rollup.retried_requests += 1;
        if retry.touched_other_station(request.station_name.as_deref()) {
            rollup.cross_station_failovers += 1;
        }
    }
    rollup
}

pub(super) fn render_overview_station_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };
    if snapshot.stations.is_empty() {
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let override_count = snapshot
        .stations
        .iter()
        .filter(|cfg| {
            cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some()
        })
        .count();
    let health_count = runtime_maps.station_health.len();
    let active_station = current_runtime_active_station(ctx.proxy);
    let provider_hits = recent_provider_hit_summaries(&snapshot.recent);
    let retry_rollup = recent_retry_rollup(&snapshot.recent);
    let same_station_retries = retry_rollup
        .retried_requests
        .saturating_sub(retry_rollup.cross_station_failovers);
    let configured_default_profile = snapshot
        .configured_default_profile
        .as_deref()
        .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"));
    let effective_default_profile = snapshot
        .default_profile
        .as_deref()
        .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"));

    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "控制台摘要", "Control console summary"));
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "站点数", "Stations"),
            snapshot.stations.len()
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "健康记录", "Health records"),
            health_count
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行时覆盖", "Runtime overrides"),
            override_count
        ));
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
        if ui
            .button(pick(ctx.lang, "打开 Requests 页", "Open Requests page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Requests);
        }
        if ui
            .button(pick(ctx.lang, "打开代理设置页", "Open Proxy Settings"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::ProxySettings);
        }
    });

    ui.small(format!(
        "{}: {}",
        pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
        snapshot
            .global_station_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));
    ui.small(format!(
        "{}: {}",
        pick(ctx.lang, "当前生效站点", "Current effective station"),
        active_station.as_deref().unwrap_or_else(|| pick(
            ctx.lang,
            "<未知/仅本机可见>",
            "<unknown/local-only>"
        ))
    ));
    ui.small(format!(
        "{}: {}  |  {}: {}",
        pick(
            ctx.lang,
            "配置 default_profile",
            "Configured default_profile"
        ),
        configured_default_profile,
        pick(ctx.lang, "当前 default_profile", "Current default_profile"),
        effective_default_profile
    ));

    if !provider_hits.is_empty() {
        let top = provider_hits
            .iter()
            .take(3)
            .map(|summary| {
                format!(
                    "{} n={} err={} retry={} att={:.1}",
                    summary.provider,
                    summary.requests,
                    summary.errors,
                    summary.retry_requests,
                    summary.avg_attempts()
                )
            })
            .collect::<Vec<_>>()
            .join("  |  ");
        ui.small(format!(
            "{}: {top}",
            pick(ctx.lang, "最近 provider 命中", "Recent provider hits")
        ));
    } else {
        ui.small(format!(
            "{}: {}",
            pick(ctx.lang, "最近 provider 命中", "Recent provider hits"),
            pick(ctx.lang, "<暂无请求>", "<no requests yet>")
        ));
    }

    ui.small(format!(
        "{}: retried={}  cross_station={}  same_station={}  fast_mode={}",
        pick(ctx.lang, "最近 failover", "Recent failover"),
        retry_rollup.retried_requests,
        retry_rollup.cross_station_failovers,
        same_station_retries,
        retry_rollup.fast_mode_requests
    ));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "Overview 只给出控制态势；更细的 quick switch、drain、breaker、provider/member 结构和 retry 细节已经拆到 Stations / Requests / Proxy Settings。",
            "Overview only shows control posture. Detailed quick switch, drain, breaker, provider/member structure, and retry details live in Stations / Requests / Proxy Settings.",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(
        provider: &str,
        station: &str,
        status_code: u16,
        tier: Option<&str>,
        retry: Option<crate::logging::RetryInfo>,
    ) -> FinishedRequest {
        FinishedRequest {
            id: 1,
            trace_id: Some("codex-1".to_string()),
            session_id: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: tier.map(ToOwned::to_owned),
            station_name: Some(station.to_string()),
            provider_id: Some(provider.to_string()),
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            retry,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code,
            duration_ms: 500,
            ttfb_ms: None,
            ended_at_ms: 1_000,
        }
    }

    #[test]
    fn recent_provider_hit_summaries_sort_by_request_count() {
        let recent = vec![
            sample_request("right", "right", 200, None, None),
            sample_request("vibe", "vibe", 200, None, None),
            sample_request("right", "right", 500, None, None),
        ];

        let summaries = recent_provider_hit_summaries(&recent);

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].provider, "right");
        assert_eq!(summaries[0].requests, 2);
        assert_eq!(summaries[0].errors, 1);
        assert_eq!(summaries[1].provider, "vibe");
    }

    #[test]
    fn recent_retry_rollup_counts_cross_station_failover_and_fast_mode() {
        let recent = vec![
            sample_request(
                "vibe",
                "vibe",
                200,
                Some("priority"),
                Some(crate::logging::RetryInfo {
                    attempts: 2,
                    upstream_chain: vec![
                        "right:https://api.right.example/v1 (idx=0) transport_error=timeout"
                            .to_string(),
                        "https://api.vibe.example/v1 (idx=1) status=200 class=-".to_string(),
                    ],
                    route_attempts: Vec::new(),
                }),
            ),
            sample_request(
                "right",
                "right",
                200,
                None,
                Some(crate::logging::RetryInfo {
                    attempts: 2,
                    upstream_chain: vec![
                        "right:https://api.right.example/v1 (idx=0) transport_error=timeout"
                            .to_string(),
                        "https://api.right.example/v1 (idx=1) status=200 class=-".to_string(),
                    ],
                    route_attempts: Vec::new(),
                }),
            ),
        ];

        let rollup = recent_retry_rollup(&recent);

        assert_eq!(rollup.retried_requests, 2);
        assert_eq!(rollup.cross_station_failovers, 1);
        assert_eq!(rollup.fast_mode_requests, 1);
    }
}
