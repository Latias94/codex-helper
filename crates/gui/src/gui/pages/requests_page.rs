use super::components::request_details;
use super::requests_filters::{
    filtered_recent_requests, render_requests_filters, selected_requests_session_id,
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
                let age = format_age(now, Some(request.ended_at_ms));
                let attempts = request.attempt_count();
                let model = request.model.as_deref().unwrap_or("-");
                let station = request.station_name.as_deref().unwrap_or("-");
                let provider = request.provider_id.as_deref().unwrap_or("-");
                let path = shorten_middle(&request.path, 60);
                let label = format!(
                    "{age}  st={}  {}ms  att={}  {}  {}  {}  {}",
                    request.status_code,
                    request.duration_ms,
                    attempts,
                    shorten(model, 18),
                    shorten(station, 14),
                    shorten(provider, 10),
                    path
                );
                if ui.selectable_label(selected, label).clicked() {
                    ctx.view.requests.selected_idx = pos;
                }
            }
        });
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
    let recent = snapshot.recent.clone();

    if let Some(error) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
        ui.add_space(4.0);
    }

    let selected_sid = selected_requests_session_id(ctx);
    let selected_sid_ref = selected_sid.as_deref();

    render_requests_filters(ui, ctx, selected_sid_ref);
    ui.add_space(6.0);

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
