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
    let route_graph_routing = running
        .cfg
        .version
        .is_some_and(crate::config::is_supported_route_graph_config_version);
    ui.label(format!(
        "{}: {}",
        if route_graph_routing {
            pick(ctx.lang, "全局 route target", "Global route target")
        } else {
            pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)")
        },
        if route_graph_routing {
            running.global_route_target_override.as_deref()
        } else {
            running.global_station_override.as_deref()
        }
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
                "运行时 route target / 兼容 active_station / drain / breaker 已移到 Stations 页集中操作。",
                "Runtime route target / compatible active_station / drain / breaker now live in the Stations page.",
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
    let compatibility = selected
        .compatibility
        .as_ref()
        .map(|compatibility| {
            format!(
                "compat_station={} upstream#{}",
                compatibility.station_name, compatibility.upstream_index
            )
        })
        .unwrap_or_else(|| "compatibility=-".to_string());
    Some(format!(
        "provider={}/{} path={} {}",
        selected.provider_id.as_str(),
        selected.endpoint_id.as_str(),
        path,
        compatibility
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
                provider_endpoint_key: "codex/alpha/default".to_string(),
                route_path: vec!["entry".to_string(), "alpha".to_string()],
                preference_group: 0,
                compatibility: Some(crate::routing_explain::RoutingExplainCompatibility {
                    station_name: "routing".to_string(),
                    upstream_index: 0,
                }),
                upstream_base_url: "https://alpha.example/v1".to_string(),
                selected: true,
                skip_reasons: Vec::new(),
            }),
            candidates: Vec::new(),
            affinity_policy: "none".to_string(),
            affinity: None,
            conditional_routes: Vec::new(),
        };

        let summary = format_running_route_summary(&explain).expect("route summary");

        assert!(summary.contains("provider=alpha/default"));
        assert!(summary.contains("path=entry > alpha"));
        assert!(summary.contains("compat_station=routing upstream#0"));
    }

    #[test]
    fn format_running_route_summary_marks_absent_legacy_compatibility() {
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
                provider_endpoint_key: "codex/alpha/default".to_string(),
                route_path: vec!["entry".to_string(), "alpha".to_string()],
                preference_group: 0,
                compatibility: None,
                upstream_base_url: "https://alpha.example/v1".to_string(),
                selected: true,
                skip_reasons: Vec::new(),
            }),
            candidates: Vec::new(),
            affinity_policy: "none".to_string(),
            affinity: None,
            conditional_routes: Vec::new(),
        };

        let summary = format_running_route_summary(&explain).expect("route summary");

        assert!(summary.contains("provider=alpha/default"));
        assert!(summary.contains("compatibility=-"));
        assert!(!summary.contains("compat_station=-"));
    }
}
