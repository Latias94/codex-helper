use super::components::request_details;
use super::requests_filters::{
    ensure_request_ledger_loaded, filtered_recent_requests, render_requests_filters,
    selected_requests_session_id,
};
use super::requests_header_actions::render_request_header;
use super::*;

fn render_requests_unavailable(ui: &mut egui::Ui, lang: Language) {
    ui.separator();
    ui.label(pick(
        lang,
        "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看请求。",
        "No proxy is running or attached. Start or attach on Overview to view requests.",
    ));
}

fn render_empty_requests_detail(
    ui: &mut egui::Ui,
    ctx: &PageCtx<'_>,
    selected_sid_ref: Option<&str>,
) {
    ui.label(if ctx.view.requests.scope_session {
        if let Some(sid) = selected_sid_ref {
            format!(
                "{} {sid}",
                pick(
                    ctx.lang,
                    "当前没有匹配这个 session 的请求：",
                    "No requests currently match session:",
                )
            )
        } else {
            pick(
                ctx.lang,
                "当前没有可匹配的请求。",
                "No requests match the current filters.",
            )
            .to_string()
        }
    } else {
        pick(
            ctx.lang,
            "无请求数据。",
            "No requests match current filters.",
        )
        .to_string()
    });
}

fn render_requests_list(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, filtered: &[&FinishedRequest]) {
    ui.heading(pick(ctx.lang, "列表", "List"));
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .id_salt("requests_list_scroll")
        .max_height(520.0)
        .show(ui, |ui| {
            let now = now_ms();
            for (pos, request) in filtered.iter().enumerate() {
                let selected = pos == ctx.view.requests.selected_idx;
                let label = request_list_label(request, now);
                if ui.selectable_label(selected, label).clicked() {
                    ctx.view.requests.selected_idx = pos;
                }
            }
        });
}

fn request_list_label(request: &FinishedRequest, now_ms: u64) -> String {
    let age = format_age(now_ms, Some(request.ended_at_ms));
    let model = request.model.as_deref().unwrap_or("-");
    let station = request.station_name.as_deref().unwrap_or("-");
    let provider = request.provider_id.as_deref().unwrap_or("-");
    let request_line = shorten_middle(&format!("{} {}", request.method, request.path), 44);
    let metrics = request_list_metrics(request);
    format!(
        "{age}  http={}  total={}  ttfb={}  att={}  {}  stn={}  prv={}  {}  req={}",
        request.status_code,
        format_duration_ms(request.duration_ms),
        request_list_ttfb(request),
        request.attempt_count(),
        shorten(model, 18),
        shorten(station, 14),
        shorten(provider, 12),
        shorten(&metrics, 96),
        request_line
    )
}

fn request_list_metrics(request: &FinishedRequest) -> String {
    let mut parts = Vec::new();
    if request.is_fast_mode() {
        parts.push("fast".to_string());
    } else if let Some(tier) = request
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("tier={tier}"));
    }

    if let Some(usage) = request.usage.as_ref() {
        parts.push(format!(
            "tok={}/{}/{}",
            compact_count(usage.input_tokens),
            compact_count(usage.output_tokens),
            compact_count(usage.total_tokens)
        ));
        let cache_total = usage
            .cached_input_tokens
            .max(0)
            .saturating_add(usage.cache_read_input_tokens.max(0))
            .saturating_add(usage.cache_creation_tokens_total().max(0));
        if cache_total > 0 {
            parts.push(format!("cache={}", compact_count(cache_total)));
        }
    }

    if let Some(rate) = request.output_tokens_per_second() {
        parts.push(format!("out/s={rate:.1}"));
    }
    if !request.cost.is_unknown() {
        parts.push(format!(
            "cost={}",
            request.cost.display_total_with_confidence()
        ));
    }

    let observability = request.observability_view();
    if observability.cross_station_failover {
        parts.push("failover=x-station".to_string());
    } else if observability.same_station_retry {
        parts.push("retry=same-station".to_string());
    } else if observability.retried {
        parts.push("retry=yes".to_string());
    }
    if observability.route_attempt_count > 0 {
        parts.push(format!("route={}", observability.route_attempt_count));
    }

    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(" ")
    }
}

fn request_list_ttfb(request: &FinishedRequest) -> String {
    format_duration_ms_opt(request.ttfb_ms)
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

fn render_requests_detail(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    filtered: &[&FinishedRequest],
    selected_sid_ref: Option<&str>,
) {
    ui.heading(pick(ctx.lang, "详情", "Details"));
    ui.add_space(4.0);

    let Some(request) = filtered.get(ctx.view.requests.selected_idx).copied() else {
        render_empty_requests_detail(ui, ctx, selected_sid_ref);
        return;
    };

    egui::ScrollArea::vertical()
        .id_salt("requests_detail_scroll")
        .max_height(520.0)
        .show(ui, |ui| {
            render_request_header(ui, ctx, request);
            ui.add_space(8.0);
            request_details::render_request_detail_cards(ui, ctx.lang, request);
        });
}

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "请求", "Requests"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        render_requests_unavailable(ui, ctx.lang);
        return;
    };

    let last_error = snapshot.last_error.clone();
    if let Some(error) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
        ui.add_space(4.0);
    }

    let selected_sid = selected_requests_session_id(ctx);
    let selected_sid_ref = selected_sid.as_deref();

    render_requests_filters(ui, ctx, selected_sid_ref);
    ui.add_space(6.0);

    ensure_request_ledger_loaded(ctx);
    let recent = if ctx.view.requests.include_request_ledger {
        ctx.view.requests.request_ledger_records.clone()
    } else {
        snapshot.recent.clone()
    };
    let filtered = filtered_recent_requests(&recent, ctx, selected_sid_ref);

    if filtered.is_empty() {
        ctx.view.requests.selected_idx = 0;
    } else {
        ctx.view.requests.selected_idx = ctx
            .view
            .requests
            .selected_idx
            .min(filtered.len().saturating_sub(1));
    }

    ui.columns(2, |cols| {
        render_requests_list(&mut cols[0], ctx, &filtered);
        render_requests_detail(&mut cols[1], ctx, &filtered, selected_sid_ref);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_fixture() -> FinishedRequest {
        FinishedRequest {
            id: 7,
            trace_id: Some("codex-7".to_string()),
            session_id: Some("sid".to_string()),
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("primary".to_string()),
            provider_id: Some("provider-a".to_string()),
            upstream_base_url: Some("https://api.example.com/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 1_000,
            ttfb_ms: Some(500),
            streaming: false,
            ended_at_ms: 9_000,
        }
    }

    #[test]
    fn request_list_label_surfaces_fast_cache_cost_and_speed() {
        let usage = UsageMetrics {
            input_tokens: 1_500,
            output_tokens: 100,
            cached_input_tokens: 50,
            cache_read_input_tokens: 250,
            total_tokens: 1_600,
            ..UsageMetrics::default()
        };
        let price = crate::pricing::ModelPrice::from_per_million_usd(
            "gpt-5.4",
            None,
            "1",
            "2",
            Some("0.1"),
            Some("0"),
            "test",
        )
        .expect("price");
        let mut request = request_fixture();
        request.usage = Some(usage.clone());
        request.cost = crate::pricing::estimate_usage_cost(
            &usage,
            &price,
            crate::pricing::CostAdjustments::default(),
        );

        let label = request_list_label(&request, 10_000);

        assert!(label.contains("fast"));
        assert!(label.contains("tok=1.5k/100/1.6k"));
        assert!(label.contains("cache=300"));
        assert!(label.contains("out/s=200.0"));
        assert!(label.contains("cost=$"));
    }

    #[test]
    fn request_list_label_marks_cross_station_failover_route_count() {
        let mut request = request_fixture();
        request.retry = Some(crate::logging::RetryInfo {
            attempts: 2,
            upstream_chain: Vec::new(),
            route_attempts: vec![
                crate::logging::RouteAttemptLog {
                    attempt_index: 0,
                    station_name: Some("backup".to_string()),
                    decision: "select".to_string(),
                    raw: "backup".to_string(),
                    ..Default::default()
                },
                crate::logging::RouteAttemptLog {
                    attempt_index: 1,
                    station_name: Some("primary".to_string()),
                    decision: "select".to_string(),
                    raw: "primary".to_string(),
                    ..Default::default()
                },
            ],
        });

        let label = request_list_label(&request, 10_000);

        assert!(label.contains("failover=x-station"));
        assert!(label.contains("route=2"));
        assert!(label.contains("att=2"));
    }
}
