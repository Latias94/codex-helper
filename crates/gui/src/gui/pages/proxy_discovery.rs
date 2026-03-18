use super::view_state::DiscoveryViewState;
use super::*;

#[derive(Debug, Default)]
pub(super) struct ProxyDiscoveryActions {
    pub scan_local_proxies: bool,
    pub attach_discovered: Option<DiscoveredProxy>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProxyDiscoveryListOptions<'a> {
    pub scroll_id: &'a str,
    pub grid_id: &'a str,
    pub max_height: f32,
    pub empty_text: &'a str,
    pub attach_enabled: bool,
    pub show_port_hover_details: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProxyDiscoveryApplyOptions<'a> {
    pub scan_done_none: &'a str,
    pub scan_done_found: &'a str,
    pub attach_success: &'a str,
    pub sync_desired_port: bool,
    pub sync_default_port: bool,
}

pub(super) fn render_proxy_discovery_list(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    options: ProxyDiscoveryListOptions<'_>,
    actions: &mut ProxyDiscoveryActions,
) {
    let discovered_all = ctx.proxy.discovered_proxies().to_vec();
    if discovered_all.is_empty() {
        ui.label(options.empty_text);
        return;
    }

    render_proxy_discovery_filters(ui, ctx, discovered_all.len());
    let discovered = discovered_all
        .into_iter()
        .filter(|proxy| discovery_matches_filters(proxy, &ctx.view.discovery))
        .collect::<Vec<_>>();
    if discovered.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(pick(
                ctx.lang,
                "当前筛选下没有匹配的代理。",
                "No proxies match the current filters.",
            ));
            if ui
                .button(pick(ctx.lang, "清除筛选", "Clear filters"))
                .clicked()
            {
                ctx.view.discovery = DiscoveryViewState::default();
            }
        });
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt(options.scroll_id)
        .max_height(options.max_height)
        .show(ui, |ui| {
            egui::Grid::new(options.grid_id)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "端口", "Port"));
                    ui.label(pick(ctx.lang, "服务", "Service"));
                    ui.label(pick(ctx.lang, "API", "API"));
                    ui.label(pick(ctx.lang, "控制面概览", "Control-plane overview"));
                    ui.label(pick(ctx.lang, "操作", "Action"));
                    ui.end_row();

                    let now = now_ms();
                    for proxy in discovered {
                        render_proxy_discovery_row(
                            ui,
                            ctx,
                            &proxy,
                            options.attach_enabled,
                            options.show_port_hover_details,
                            now,
                            actions,
                        );
                    }
                });
        });
}

fn render_proxy_discovery_filters(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, total: usize) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} {}", pick(ctx.lang, "发现", "Found"), total));
        ui.separator();
        ui.toggle_value(
            &mut ctx.view.discovery.recommended_only,
            pick(ctx.lang, "仅推荐", "Recommended"),
        );
        ui.toggle_value(
            &mut ctx.view.discovery.station_control_only,
            pick(ctx.lang, "站点控制", "Station control"),
        );
        ui.toggle_value(
            &mut ctx.view.discovery.session_control_only,
            pick(ctx.lang, "会话控制", "Session control"),
        );
        ui.toggle_value(
            &mut ctx.view.discovery.retry_write_only,
            pick(ctx.lang, "重试写回", "Retry write"),
        );
        ui.toggle_value(
            &mut ctx.view.discovery.remote_admin_only,
            pick(ctx.lang, "远端管理", "Remote admin"),
        );
        if ui.small_button(pick(ctx.lang, "重置", "Reset")).clicked() {
            ctx.view.discovery = DiscoveryViewState::default();
        }
    });
    ui.add_space(4.0);
}

pub(super) fn apply_proxy_discovery_actions(
    ctx: &mut PageCtx<'_>,
    actions: ProxyDiscoveryActions,
    options: ProxyDiscoveryApplyOptions<'_>,
) {
    if actions.scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info = Some(options.scan_done_none.to_string());
        } else {
            *ctx.last_info = Some(options.scan_done_found.to_string());
        }
    }

    if let Some(proxy) = actions.attach_discovered {
        let warning = remote_local_only_warning_message(
            proxy.admin_base_url.as_str(),
            &proxy.host_local_capabilities,
            ctx.lang,
            &[
                pick(ctx.lang, "cwd", "cwd"),
                pick(ctx.lang, "transcript", "transcript"),
                pick(ctx.lang, "resume", "resume"),
                pick(ctx.lang, "open file", "open file"),
            ],
        );
        let admin_message = remote_admin_access_message(
            proxy.admin_base_url.as_str(),
            &proxy.remote_admin_access,
            ctx.lang,
        );
        ctx.proxy
            .request_attach_with_admin_base(proxy.port, Some(proxy.admin_base_url.clone()));
        if options.sync_desired_port {
            ctx.proxy.set_desired_port(proxy.port);
        }
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        if options.sync_default_port {
            ctx.gui_cfg.proxy.default_port = proxy.port;
        }
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                options.attach_success.to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
        }
    }
}

fn render_proxy_discovery_row(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    proxy: &DiscoveredProxy,
    attach_enabled: bool,
    show_port_hover_details: bool,
    now: u64,
    actions: &mut ProxyDiscoveryActions,
) {
    if show_port_hover_details {
        let mut hover = format!("base_url: {}", proxy.base_url);
        hover.push_str(&format!("\nadmin_base_url: {}", proxy.admin_base_url));
        if !proxy.endpoints.is_empty() {
            hover.push_str(&format!("\nendpoints: {}", proxy.endpoints.len()));
        }
        if let Some(ms) = proxy.runtime_loaded_at_ms {
            hover.push_str(&format!("\nruntime_loaded: {}", format_age(now, Some(ms))));
        }
        if let Some(summary) = discovery_catalog_summary(proxy, ctx.lang) {
            hover.push('\n');
            hover.push_str(&summary);
        }
        if let Some(summary) = discovery_retry_summary(proxy, ctx.lang) {
            hover.push('\n');
            hover.push_str(&summary);
        }
        ui.label(proxy.port.to_string()).on_hover_text(hover);
    } else {
        ui.label(proxy.port.to_string());
    }

    ui.label(
        proxy
            .service_name
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "未知", "unknown")),
    );
    ui.label(match proxy.api_version {
        Some(v) => format!("v{v}"),
        None => "-".to_string(),
    });
    ui.vertical(|ui| {
        if let Some(err) = proxy.last_error.as_deref() {
            ui.label(err);
        } else {
            ui.label(pick(ctx.lang, "可用", "OK"));
        }
        if let Some(summary) = discovery_runtime_summary(proxy, ctx.lang) {
            ui.small(summary);
        }
        if let Some(summary) = discovery_catalog_summary(proxy, ctx.lang) {
            ui.small(summary);
        }
        if let Some(summary) = discovery_retry_summary(proxy, ctx.lang) {
            ui.small(summary);
        }
        if let Some(summary) = discovery_health_summary(proxy, ctx.lang) {
            ui.small(summary);
        }
        if let Some(summary) = discovery_control_summary(proxy, ctx.lang) {
            ui.small(summary);
        }
        render_discovery_tags(ui, proxy, ctx.lang);
        if let Some(warning) = remote_local_only_warning_message(
            proxy.admin_base_url.as_str(),
            &proxy.host_local_capabilities,
            ctx.lang,
            &[
                pick(ctx.lang, "cwd", "cwd"),
                pick(ctx.lang, "transcript", "transcript"),
                pick(ctx.lang, "resume", "resume"),
            ],
        ) {
            ui.small(pick(
                ctx.lang,
                "远端附着：本机禁用 host-local 功能",
                "Remote attach: no host-local access here",
            ))
            .on_hover_text(warning);
        }
        if let Some(label) = remote_admin_access_short_label(
            proxy.admin_base_url.as_str(),
            &proxy.remote_admin_access,
            ctx.lang,
        ) {
            let color = if proxy.remote_admin_access.remote_enabled && remote_admin_token_present()
            {
                egui::Color32::from_rgb(60, 160, 90)
            } else {
                egui::Color32::from_rgb(200, 120, 40)
            };
            let response = ui.colored_label(color, label);
            if let Some(message) = remote_admin_access_message(
                proxy.admin_base_url.as_str(),
                &proxy.remote_admin_access,
                ctx.lang,
            ) {
                response.on_hover_text(message);
            }
        }
    });

    if ui
        .add_enabled(
            attach_enabled,
            egui::Button::new(pick(ctx.lang, "附着", "Attach")),
        )
        .clicked()
    {
        actions.attach_discovered = Some(proxy.clone());
    }
    ui.end_row();
}

fn discovery_has_operator_home(proxy: &DiscoveredProxy) -> bool {
    proxy.operator_runtime_summary.is_some()
        || proxy.operator_retry_summary.is_some()
        || proxy.operator_health_summary.is_some()
        || proxy.operator_counts.is_some()
}

fn discovery_has_station_control(proxy: &DiscoveredProxy) -> bool {
    let surface = &proxy.surface_capabilities;
    surface.stations
        || surface.station_runtime
        || surface.station_persisted_settings
        || surface.station_specs
        || surface.station_probe
}

fn discovery_has_profile_control(proxy: &DiscoveredProxy) -> bool {
    let surface = &proxy.surface_capabilities;
    surface.profiles
        || surface.default_profile_override
        || surface.persisted_default_profile
        || surface.profile_mutation
}

fn discovery_has_session_control(proxy: &DiscoveredProxy) -> bool {
    let surface = &proxy.surface_capabilities;
    surface.session_overrides
        || surface.session_profile_override
        || surface.session_model_override
        || surface.session_reasoning_effort_override
        || surface.session_station_override
        || surface.session_service_tier_override
        || surface.session_override_reset
}

fn discovery_has_provider_control(proxy: &DiscoveredProxy) -> bool {
    let surface = &proxy.surface_capabilities;
    surface.providers || surface.provider_runtime || surface.provider_specs
}

fn discovery_has_retry_write(proxy: &DiscoveredProxy) -> bool {
    proxy
        .operator_retry_summary
        .as_ref()
        .map(|retry| retry.supports_write)
        .unwrap_or(proxy.surface_capabilities.retry_config)
}

fn discovery_has_remote_admin(proxy: &DiscoveredProxy) -> bool {
    proxy.remote_admin_access.remote_enabled
}

fn discovery_has_management_surface(proxy: &DiscoveredProxy) -> bool {
    discovery_has_station_control(proxy)
        || discovery_has_profile_control(proxy)
        || discovery_has_session_control(proxy)
        || discovery_has_provider_control(proxy)
        || discovery_has_retry_write(proxy)
}

fn discovery_is_recommended(proxy: &DiscoveredProxy) -> bool {
    proxy.last_error.is_none()
        && discovery_has_operator_home(proxy)
        && proxy.surface_capabilities.operator_summary
        && discovery_has_management_surface(proxy)
}

fn discovery_matches_filters(proxy: &DiscoveredProxy, filters: &DiscoveryViewState) -> bool {
    (!filters.recommended_only || discovery_is_recommended(proxy))
        && (!filters.station_control_only || discovery_has_station_control(proxy))
        && (!filters.session_control_only || discovery_has_session_control(proxy))
        && (!filters.retry_write_only || discovery_has_retry_write(proxy))
        && (!filters.remote_admin_only || discovery_has_remote_admin(proxy))
}

fn render_discovery_tags(ui: &mut egui::Ui, proxy: &DiscoveredProxy, lang: Language) {
    let mut tags = Vec::new();
    if discovery_is_recommended(proxy) {
        tags.push((
            pick(lang, "推荐", "recommended"),
            egui::Color32::from_rgb(60, 160, 90),
        ));
    }
    if discovery_has_station_control(proxy) {
        tags.push((
            pick(lang, "站点", "station"),
            egui::Color32::from_rgb(80, 130, 210),
        ));
    }
    if discovery_has_profile_control(proxy) {
        tags.push((
            pick(lang, "配置档", "profile"),
            egui::Color32::from_rgb(110, 110, 190),
        ));
    }
    if discovery_has_session_control(proxy) {
        tags.push((
            pick(lang, "会话", "session"),
            egui::Color32::from_rgb(190, 120, 50),
        ));
    }
    if discovery_has_provider_control(proxy) {
        tags.push((
            pick(lang, "提供商", "provider"),
            egui::Color32::from_rgb(60, 145, 150),
        ));
    }
    if discovery_has_retry_write(proxy) {
        tags.push((
            pick(lang, "重试", "retry"),
            egui::Color32::from_rgb(150, 90, 170),
        ));
    }
    if discovery_has_remote_admin(proxy) {
        tags.push((
            pick(lang, "远端管理", "remote-admin"),
            egui::Color32::from_rgb(180, 90, 90),
        ));
    }
    if tags.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        for (label, color) in tags {
            ui.colored_label(color, format!("#{label}"));
        }
    });
}

fn discovery_runtime_summary(proxy: &DiscoveredProxy, lang: Language) -> Option<String> {
    let runtime = proxy.operator_runtime_summary.as_ref()?;
    let station = runtime
        .effective_active_station
        .as_deref()
        .or(runtime.configured_active_station.as_deref());
    let profile = runtime
        .default_profile
        .as_deref()
        .or(runtime.configured_default_profile.as_deref());

    match (station, profile) {
        (Some(station), Some(profile)) => Some(match lang {
            Language::Zh => format!("当前: 站点 {station} · 配置档 {profile}"),
            Language::En => format!("Current: station={station} · profile={profile}"),
        }),
        (Some(station), None) => Some(match lang {
            Language::Zh => format!("当前: 站点 {station}"),
            Language::En => format!("Current: station={station}"),
        }),
        (None, Some(profile)) => Some(match lang {
            Language::Zh => format!("当前: 配置档 {profile}"),
            Language::En => format!("Current: profile={profile}"),
        }),
        (None, None) => None,
    }
}

fn discovery_catalog_summary(proxy: &DiscoveredProxy, lang: Language) -> Option<String> {
    let counts = proxy.operator_counts.as_ref()?;
    Some(match lang {
        Language::Zh => format!(
            "目录: 会话 {} · 站点 {} · 配置档 {}",
            counts.sessions, counts.stations, counts.profiles
        ),
        Language::En => format!(
            "Catalog: sessions {} · stations {} · profiles {}",
            counts.sessions, counts.stations, counts.profiles
        ),
    })
}

fn discovery_health_summary(proxy: &DiscoveredProxy, lang: Language) -> Option<String> {
    let health = proxy.operator_health_summary.as_ref()?;
    let mut parts = Vec::new();
    if health.stations_draining > 0 {
        parts.push(match lang {
            Language::Zh => format!("排空 {}", health.stations_draining),
            Language::En => format!("draining {}", health.stations_draining),
        });
    }
    if health.stations_breaker_open > 0 {
        parts.push(match lang {
            Language::Zh => format!("熔断 {}", health.stations_breaker_open),
            Language::En => format!("breaker {}", health.stations_breaker_open),
        });
    }
    if health.stations_half_open > 0 {
        parts.push(match lang {
            Language::Zh => format!("半开 {}", health.stations_half_open),
            Language::En => format!("half-open {}", health.stations_half_open),
        });
    }
    if health.stations_with_active_health_checks > 0 {
        parts.push(match lang {
            Language::Zh => format!("探测 {}", health.stations_with_active_health_checks),
            Language::En => format!("probes {}", health.stations_with_active_health_checks),
        });
    }
    if health.stations_with_probe_failures > 0 {
        parts.push(match lang {
            Language::Zh => format!("失败探测 {}", health.stations_with_probe_failures),
            Language::En => format!("probe-fail {}", health.stations_with_probe_failures),
        });
    }
    if health.stations_with_degraded_passive_health > 0 {
        parts.push(match lang {
            Language::Zh => format!("降级 {}", health.stations_with_degraded_passive_health),
            Language::En => format!("degraded {}", health.stations_with_degraded_passive_health),
        });
    }
    if health.stations_with_failing_passive_health > 0 {
        parts.push(match lang {
            Language::Zh => format!("失败 {}", health.stations_with_failing_passive_health),
            Language::En => format!("failing {}", health.stations_with_failing_passive_health),
        });
    }

    if parts.is_empty() {
        Some(pick(lang, "健康: 正常", "Health: OK").to_string())
    } else {
        Some(format!(
            "{} {}",
            pick(lang, "健康:", "Health:"),
            parts.join(" · ")
        ))
    }
}

fn retry_profile_label(profile: crate::config::RetryProfileName) -> &'static str {
    match profile {
        crate::config::RetryProfileName::Balanced => "balanced",
        crate::config::RetryProfileName::SameUpstream => "same-upstream",
        crate::config::RetryProfileName::AggressiveFailover => "aggressive-failover",
        crate::config::RetryProfileName::CostPrimary => "cost-primary",
    }
}

fn discovery_retry_summary(proxy: &DiscoveredProxy, lang: Language) -> Option<String> {
    let retry = proxy.operator_retry_summary.as_ref()?;
    let mut parts = Vec::new();
    if let Some(profile) = retry.configured_profile {
        parts.push(retry_profile_label(profile).to_string());
    }
    parts.push(match lang {
        Language::Zh => format!("上游 {}", retry.upstream_max_attempts),
        Language::En => format!("upstream {}", retry.upstream_max_attempts),
    });
    parts.push(match lang {
        Language::Zh => format!("提供商 {}", retry.provider_max_attempts),
        Language::En => format!("provider {}", retry.provider_max_attempts),
    });
    if retry.allow_cross_station_before_first_output {
        parts.push(pick(lang, "首包前跨站", "pre-output cross-station").to_string());
    }
    if retry.supports_write {
        parts.push(pick(lang, "可写回", "writable").to_string());
    }

    Some(format!(
        "{} {}",
        pick(lang, "重试:", "Retry:"),
        parts.join(" · ")
    ))
}

fn discovery_control_summary(proxy: &DiscoveredProxy, lang: Language) -> Option<String> {
    let mut parts = Vec::new();

    if discovery_has_station_control(proxy) {
        parts.push(pick(lang, "站点控制", "station control"));
    }
    if discovery_has_profile_control(proxy) {
        parts.push(pick(lang, "配置档控制", "profile control"));
    }
    if discovery_has_session_control(proxy) {
        parts.push(pick(lang, "会话控制", "session control"));
    }
    if discovery_has_provider_control(proxy) {
        parts.push(pick(lang, "提供商目录", "provider catalog"));
    }
    if discovery_has_retry_write(proxy) {
        parts.push(pick(lang, "重试策略", "retry policy"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!(
            "{} {}",
            pick(lang, "能力:", "Controls:"),
            parts.join(" / ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_discovered_proxy() -> DiscoveredProxy {
        DiscoveredProxy {
            port: 4321,
            base_url: "http://127.0.0.1:4321".to_string(),
            admin_base_url: "http://127.0.0.1:5321".to_string(),
            api_version: Some(1),
            service_name: Some("codex".to_string()),
            endpoints: Vec::new(),
            surface_capabilities: crate::dashboard_core::ControlPlaneSurfaceCapabilities {
                operator_summary: true,
                stations: true,
                station_runtime: true,
                station_persisted_settings: true,
                station_specs: true,
                profiles: true,
                default_profile_override: true,
                session_overrides: true,
                session_model_override: true,
                provider_specs: true,
                ..Default::default()
            },
            runtime_loaded_at_ms: Some(100),
            operator_runtime_summary: Some(crate::dashboard_core::OperatorRuntimeSummary {
                runtime_loaded_at_ms: Some(100),
                runtime_source_mtime_ms: Some(101),
                configured_active_station: Some("right".to_string()),
                effective_active_station: Some("vibe".to_string()),
                global_station_override: None,
                configured_default_profile: Some("balanced".to_string()),
                default_profile: Some("fast".to_string()),
                default_profile_summary: None,
            }),
            operator_retry_summary: Some(crate::dashboard_core::OperatorRetrySummary {
                configured_profile: Some(crate::config::RetryProfileName::Balanced),
                supports_write: true,
                upstream_max_attempts: 2,
                provider_max_attempts: 3,
                allow_cross_station_before_first_output: true,
            }),
            operator_health_summary: Some(crate::dashboard_core::OperatorHealthSummary {
                stations_draining: 1,
                stations_breaker_open: 2,
                stations_half_open: 0,
                stations_with_active_health_checks: 3,
                stations_with_probe_failures: 0,
                stations_with_degraded_passive_health: 0,
                stations_with_failing_passive_health: 0,
                stations_with_cooldown: 0,
                stations_with_usage_exhaustion: 0,
            }),
            operator_counts: Some(crate::dashboard_core::OperatorSummaryCounts {
                active_requests: 1,
                recent_requests: 2,
                sessions: 3,
                stations: 4,
                profiles: 5,
            }),
            last_error: None,
            shared_capabilities: Default::default(),
            host_local_capabilities: Default::default(),
            remote_admin_access: Default::default(),
        }
    }

    #[test]
    fn discovery_runtime_catalog_and_health_summaries_are_semantic() {
        let proxy = sample_discovered_proxy();

        assert_eq!(
            discovery_runtime_summary(&proxy, Language::Zh).as_deref(),
            Some("当前: 站点 vibe · 配置档 fast")
        );
        assert_eq!(
            discovery_catalog_summary(&proxy, Language::Zh).as_deref(),
            Some("目录: 会话 3 · 站点 4 · 配置档 5")
        );
        assert_eq!(
            discovery_retry_summary(&proxy, Language::Zh).as_deref(),
            Some("重试: balanced · 上游 2 · 提供商 3 · 首包前跨站 · 可写回")
        );
        assert_eq!(
            discovery_health_summary(&proxy, Language::Zh).as_deref(),
            Some("健康: 排空 1 · 熔断 2 · 探测 3")
        );
    }

    #[test]
    fn discovery_control_summary_groups_surface_capabilities() {
        let proxy = sample_discovered_proxy();

        assert_eq!(
            discovery_control_summary(&proxy, Language::En).as_deref(),
            Some(
                "Controls: station control / profile control / session control / provider catalog / retry policy"
            )
        );
    }

    #[test]
    fn discovery_filters_match_expected_capabilities() {
        let proxy = sample_discovered_proxy();

        assert!(discovery_matches_filters(
            &proxy,
            &DiscoveryViewState {
                recommended_only: true,
                ..Default::default()
            }
        ));
        assert!(discovery_matches_filters(
            &proxy,
            &DiscoveryViewState {
                station_control_only: true,
                ..Default::default()
            }
        ));
        assert!(discovery_matches_filters(
            &proxy,
            &DiscoveryViewState {
                session_control_only: true,
                ..Default::default()
            }
        ));
        assert!(discovery_matches_filters(
            &proxy,
            &DiscoveryViewState {
                retry_write_only: true,
                ..Default::default()
            }
        ));
        assert!(!discovery_matches_filters(
            &proxy,
            &DiscoveryViewState {
                remote_admin_only: true,
                ..Default::default()
            }
        ));

        let mut remote_proxy = proxy.clone();
        remote_proxy.remote_admin_access.remote_enabled = true;
        assert!(discovery_matches_filters(
            &remote_proxy,
            &DiscoveryViewState {
                remote_admin_only: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn discovery_is_recommended_requires_operator_home_and_management_surface() {
        let proxy = sample_discovered_proxy();
        assert!(discovery_is_recommended(&proxy));

        let mut no_operator_summary = proxy.clone();
        no_operator_summary.surface_capabilities.operator_summary = false;
        assert!(!discovery_is_recommended(&no_operator_summary));

        let mut no_management_surface = proxy.clone();
        no_management_surface.surface_capabilities.stations = false;
        no_management_surface.surface_capabilities.station_runtime = false;
        no_management_surface
            .surface_capabilities
            .station_persisted_settings = false;
        no_management_surface.surface_capabilities.station_specs = false;
        no_management_surface.surface_capabilities.profiles = false;
        no_management_surface
            .surface_capabilities
            .default_profile_override = false;
        no_management_surface.surface_capabilities.session_overrides = false;
        no_management_surface
            .surface_capabilities
            .session_model_override = false;
        no_management_surface.surface_capabilities.provider_specs = false;
        no_management_surface.operator_retry_summary = None;
        assert!(!discovery_is_recommended(&no_management_surface));

        let mut with_error = proxy.clone();
        with_error.last_error = Some("boom".to_string());
        assert!(!discovery_is_recommended(&with_error));
    }
}
