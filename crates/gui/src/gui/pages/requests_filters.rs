use super::*;

pub(super) fn selected_requests_session_id(ctx: &PageCtx<'_>) -> Option<String> {
    ctx.view
        .requests
        .focused_session_id
        .clone()
        .or_else(|| ctx.view.sessions.selected_session_id.clone())
}

pub(super) fn render_requests_filters(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_sid_ref: Option<&str>,
) {
    let request_ledger_available = ctx.proxy.request_ledger_source().is_some();
    if !request_ledger_available {
        ctx.view.requests.include_request_ledger = false;
    }

    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.requests.scope_session,
            pick(ctx.lang, "跟随所选会话", "Scope to selected session"),
        );
        ui.checkbox(
            &mut ctx.view.requests.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        ui.add_enabled_ui(request_ledger_available, |ui| {
            let changed = ui
                .checkbox(
                    &mut ctx.view.requests.include_request_ledger,
                    pick(ctx.lang, "请求日志", "Request ledger"),
                )
                .changed();
            if changed && ctx.view.requests.include_request_ledger {
                refresh_request_ledger(ctx);
            }
        });
        if ctx.view.requests.include_request_ledger {
            ui.label(pick(ctx.lang, "条数", "Limit"));
            let limit_response = ui.add(
                egui::DragValue::new(&mut ctx.view.requests.request_ledger_limit)
                    .range(20..=5000)
                    .speed(20),
            );
            if limit_response.changed() {
                ctx.view.requests.request_ledger_limit =
                    ctx.view.requests.request_ledger_limit.clamp(20, 5000);
            }
            if ui.button(pick(ctx.lang, "载入日志", "Load log")).clicked() {
                refresh_request_ledger(ctx);
            }
        }
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
            if ctx.view.requests.include_request_ledger {
                refresh_request_ledger(ctx);
            }
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(pick(ctx.lang, "模型", "Model"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.requests.model_filter).desired_width(110.0),
        );
        ui.label(pick(ctx.lang, "站点", "Station"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.requests.station_filter).desired_width(100.0),
        );
        ui.label(pick(ctx.lang, "提供商", "Provider"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.requests.provider_filter).desired_width(100.0),
        );
        ui.checkbox(
            &mut ctx.view.requests.fast_only,
            pick(ctx.lang, "fast", "fast"),
        );
        ui.checkbox(
            &mut ctx.view.requests.retried_only,
            pick(ctx.lang, "重试", "retried"),
        );

        let has_filters = !ctx.view.requests.model_filter.trim().is_empty()
            || !ctx.view.requests.station_filter.trim().is_empty()
            || !ctx.view.requests.provider_filter.trim().is_empty()
            || ctx.view.requests.fast_only
            || ctx.view.requests.retried_only;
        if has_filters
            && ui
                .button(pick(ctx.lang, "清除筛选", "Clear filters"))
                .clicked()
        {
            ctx.view.requests.model_filter.clear();
            ctx.view.requests.station_filter.clear();
            ctx.view.requests.provider_filter.clear();
            ctx.view.requests.fast_only = false;
            ctx.view.requests.retried_only = false;
        }
    });

    if ctx.view.requests.include_request_ledger {
        render_request_ledger_status(ui, ctx);
    }

    if ctx.view.requests.scope_session {
        ui.horizontal_wrapped(|ui| {
            if let Some(sid) = selected_sid_ref {
                ui.small(format!("session: {sid}"));
                if ctx.view.requests.focused_session_id.is_some() {
                    ui.small(pick(ctx.lang, "（显式聚焦）", "(explicit focus)"));
                    if ui
                        .button(pick(ctx.lang, "改为跟随 Sessions", "Follow Sessions instead"))
                        .clicked()
                    {
                        ctx.view.requests.focused_session_id = None;
                    }
                } else {
                    ui.small(pick(ctx.lang, "（跟随 Sessions）", "(following Sessions)"));
                }
            } else {
                ui.small(pick(
                    ctx.lang,
                    "当前没有可用于限定的 session_id；显示全部请求。",
                    "No session_id is available for scoping right now; all requests remain visible.",
                ));
            }
        });
    }
}

pub(super) fn ensure_request_ledger_loaded(ctx: &mut PageCtx<'_>) {
    if !ctx.view.requests.include_request_ledger {
        return;
    }
    let source_signature = ctx.proxy.request_ledger_source_signature();
    if source_signature.is_none() {
        ctx.view.requests.include_request_ledger = false;
        return;
    }
    let source_changed = ctx.view.requests.request_ledger_loaded_signature != source_signature;
    let never_loaded = ctx.view.requests.request_ledger_loaded_at_ms.is_none()
        && ctx.view.requests.request_ledger_last_error.is_none();
    if never_loaded || source_changed {
        refresh_request_ledger(ctx);
    }
}

fn refresh_request_ledger(ctx: &mut PageCtx<'_>) {
    let limit = ctx.view.requests.request_ledger_limit.clamp(20, 5000);
    ctx.view.requests.request_ledger_limit = limit;
    let source = ctx.proxy.request_ledger_source();
    let source_signature = source.as_ref().map(|source| source.signature());
    let source_detail = source.as_ref().map(|source| source.display_detail());
    match ctx.proxy.read_request_ledger_records(ctx.rt, limit) {
        Ok(result) => {
            ctx.view.requests.request_ledger_loaded_signature = Some(result.source.signature());
            ctx.view.requests.request_ledger_source_detail = Some(result.source.display_detail());
            ctx.view.requests.request_ledger_records = result.records;
            ctx.view.requests.request_ledger_loaded_limit = limit;
            ctx.view.requests.request_ledger_loaded_at_ms = Some(now_ms());
            ctx.view.requests.request_ledger_last_error = None;
            ctx.view.requests.selected_idx = 0;
        }
        Err(err) => {
            ctx.view.requests.request_ledger_records.clear();
            ctx.view.requests.request_ledger_loaded_limit = limit;
            ctx.view.requests.request_ledger_loaded_signature = source_signature;
            ctx.view.requests.request_ledger_source_detail = source_detail;
            ctx.view.requests.request_ledger_loaded_at_ms = Some(now_ms());
            ctx.view.requests.request_ledger_last_error = Some(err.to_string());
            ctx.view.requests.selected_idx = 0;
        }
    }
}

fn render_request_ledger_status(ui: &mut egui::Ui, ctx: &PageCtx<'_>) {
    ui.horizontal_wrapped(|ui| {
        if let Some(error) = ctx.view.requests.request_ledger_last_error.as_deref() {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                format!(
                    "{}: {error}",
                    pick(ctx.lang, "请求日志错误", "Request ledger error")
                ),
            );
            return;
        }
        ui.small(format!(
            "{}: {} / {}  {}",
            pick(ctx.lang, "请求日志", "Request ledger"),
            ctx.view.requests.request_ledger_records.len(),
            ctx.view.requests.request_ledger_loaded_limit,
            ctx.view
                .requests
                .request_ledger_source_detail
                .as_deref()
                .unwrap_or("-")
        ));
    });
}

pub(super) fn filtered_recent_requests<'a>(
    recent: &'a [FinishedRequest],
    ctx: &PageCtx<'_>,
    selected_sid_ref: Option<&str>,
) -> Vec<&'a FinishedRequest> {
    recent
        .iter()
        .filter(|request| request_matches_filters(request, &ctx.view.requests, selected_sid_ref))
        .take(600)
        .collect()
}

fn request_matches_filters(
    request: &FinishedRequest,
    filters: &RequestsViewState,
    selected_sid_ref: Option<&str>,
) -> bool {
    if filters.errors_only && request.status_code < 400 {
        return false;
    }
    if filters.scope_session {
        match (selected_sid_ref, request.session_id.as_deref()) {
            (Some(sid), Some(request_sid)) if sid == request_sid => {}
            (Some(_), _) => return false,
            (None, _) => {}
        }
    }
    if !field_matches_filter(request.model.as_deref(), &filters.model_filter) {
        return false;
    }
    if !field_matches_filter(request.station_name.as_deref(), &filters.station_filter) {
        return false;
    }
    if !field_matches_filter(request.provider_id.as_deref(), &filters.provider_filter) {
        return false;
    }
    if filters.fast_only && !request.is_fast_mode() {
        return false;
    }
    if filters.retried_only && !request.observability_view().retried {
        return false;
    }
    true
}

fn field_matches_filter(value: Option<&str>, filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    value
        .map(|value| value.to_ascii_lowercase().contains(&filter))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_fixture() -> FinishedRequest {
        let mut request = FinishedRequest {
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
            provider_id: Some("relay".to_string()),
            upstream_base_url: Some("https://relay.example/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: Some(crate::logging::RetryInfo {
                attempts: 2,
                upstream_chain: vec![
                    "primary:https://relay.example/v1".to_string(),
                    "backup:https://relay.example/v1".to_string(),
                ],
                route_attempts: Vec::new(),
            }),
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 1_000,
            ttfb_ms: Some(200),
            streaming: false,
            ended_at_ms: 9_000,
        };
        request.refresh_observability();
        request
    }

    #[test]
    fn request_filters_match_route_fast_and_retry_fields() {
        let request = request_fixture();
        let filters = RequestsViewState {
            scope_session: true,
            model_filter: "5.4".to_string(),
            station_filter: "prim".to_string(),
            provider_filter: "relay".to_string(),
            fast_only: true,
            retried_only: true,
            ..RequestsViewState::default()
        };

        assert!(request_matches_filters(&request, &filters, Some("sid")));
    }

    #[test]
    fn request_filters_reject_nonmatching_provider() {
        let request = request_fixture();
        let filters = RequestsViewState {
            scope_session: false,
            provider_filter: "other".to_string(),
            ..RequestsViewState::default()
        };

        assert!(!request_matches_filters(&request, &filters, None));
    }
}
