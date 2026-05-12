use super::*;
use crate::routing_explain::RoutingExplainResponse;

pub(super) fn render_running_proxy_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(running) = ctx.proxy.running() else {
        return;
    };

    ui.label(format!(
        "{}: 127.0.0.1:{} ({})",
        pick(ctx.lang, "运行中", "Running"),
        running.port,
        running.service_name
    ));
    if let Some(error) = running.last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
    }

    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "活跃请求", "Active requests"),
        running.active.len()
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
        running.recent.len()
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
        running
            .global_station_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));

    let active_name = match running.service_name {
        "claude" => running.cfg.claude.active.clone(),
        _ => running.cfg.codex.active.clone(),
    };
    let active_fallback = match running.service_name {
        "claude" => running
            .cfg
            .claude
            .active_station()
            .map(|config| config.name.clone()),
        _ => running
            .cfg
            .codex
            .active_station()
            .map(|config| config.name.clone()),
    };
    if let Some(routing_explain) = running.routing_explain.as_ref()
        && let Some(summary) = format_running_route_summary(routing_explain)
    {
        ui.small(format!(
            "{}: {summary}",
            pick(ctx.lang, "当前路由", "Current route")
        ));
    }
    let active_display = active_name
        .or(active_fallback)
        .unwrap_or_else(|| "-".to_string());
    ui.label(format!(
        "{}: {}",
        pick(
            ctx.lang,
            "配置 active_station(兼容)",
            "Configured active_station (compat)"
        ),
        active_display
    ));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "默认 active_station / global pin / drain / breaker 已移到 Stations 页集中操作。",
                "Default active_station / global pin / drain / breaker now live in the Stations page.",
            ),
        );
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
    });

    let warnings =
        crate::config::model_routing_warnings(running.cfg.as_ref(), running.service_name);
    if !warnings.is_empty() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "模型路由配置警告（建议处理）：",
            "Model routing warnings (recommended to fix):",
        ));
        egui::ScrollArea::vertical()
            .id_salt("overview_model_routing_warnings_scroll")
            .max_height(120.0)
            .show(ui, |ui| {
                for warning in warnings {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), warning);
                }
            });
    }
}

fn format_running_route_summary(explain: &RoutingExplainResponse) -> Option<String> {
    let selected = explain.selected_route.as_ref()?;
    let path = if selected.route_path.is_empty() {
        "-".to_string()
    } else {
        selected.route_path.join(" > ")
    };
    let compat_station = if selected.compatibility.station_name.is_empty() {
        "-"
    } else {
        selected.compatibility.station_name.as_str()
    };
    Some(format!(
        "provider={}/{} path={} compat_station={}",
        selected.provider_id.as_str(),
        selected.endpoint_id.as_str(),
        path,
        compat_station
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_running_route_summary_prefers_provider_endpoint_identity() {
        let explain = RoutingExplainResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            runtime_loaded_at_ms: Some(123),
            request_model: Some("gpt-5.4".to_string()),
            session_id: Some("sid-1".to_string()),
            request_context: Default::default(),
            selected_route: Some(crate::routing_explain::RoutingExplainCandidate {
                provider_id: "alpha".to_string(),
                provider_alias: Some("Alpha".to_string()),
                endpoint_id: "default".to_string(),
                route_path: vec!["entry".to_string(), "alpha".to_string()],
                compatibility: crate::routing_explain::RoutingExplainCompatibility {
                    station_name: "routing".to_string(),
                    upstream_index: 0,
                },
                station_name: "routing".to_string(),
                upstream_index: 0,
                upstream_base_url: "https://alpha.example/v1".to_string(),
                selected: true,
                skip_reasons: Vec::new(),
            }),
            candidates: Vec::new(),
            conditional_routes: Vec::new(),
        };

        let summary = format_running_route_summary(&explain).expect("route summary");

        assert!(summary.contains("provider=alpha/default"));
        assert!(summary.contains("path=entry > alpha"));
        assert!(summary.contains("compat_station=routing"));
    }
}
