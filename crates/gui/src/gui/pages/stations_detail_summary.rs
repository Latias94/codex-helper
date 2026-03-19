use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_station_identity_summary(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
    health: Option<&StationHealth>,
    health_status: Option<&HealthCheckStatus>,
    lb: Option<&LbConfigView>,
    referencing_profiles: &[String],
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
) {
    ui.label(format!("name: {}", cfg.name));
    ui.label(format!(
        "alias: {}",
        cfg.alias
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "-", "-"))
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "路由角色", "Routing role"),
        if effective_active_station == Some(cfg.name.as_str()) {
            pick(ctx.lang, "当前 active_station", "current active_station")
        } else if snapshot.global_station_override.as_deref() == Some(cfg.name.as_str()) {
            pick(ctx.lang, "当前 global pin", "current global pin")
        } else {
            pick(ctx.lang, "普通候选", "normal candidate")
        }
    ));
    if configured_active_station == Some(cfg.name.as_str())
        && effective_active_station != Some(cfg.name.as_str())
    {
        ui.small(pick(
            ctx.lang,
            "该站点是配置 active_station，但当前生效路由已被 fallback / pin / runtime 状态改变。",
            "This station is the configured active_station, but the effective route currently differs because of fallback, pin, or runtime state.",
        ));
    }
    ui.label(format!(
        "enabled: {}  (configured: {})",
        cfg.enabled, cfg.configured_enabled
    ));
    ui.label(format!(
        "level: L{}  (configured: L{})",
        cfg.level.clamp(1, 10),
        cfg.configured_level.clamp(1, 10)
    ));
    ui.label(format!(
        "state: {}",
        runtime_config_state_label(ctx.lang, cfg.runtime_state)
    ));
    ui.label(format!(
        "source: {}",
        format_runtime_station_source(ctx.lang, cfg)
    ));
    ui.label(format!(
        "health: {}",
        format_runtime_station_health_status(health, health_status)
    ));
    ui.label(format!("breaker: {}", format_runtime_lb_summary(lb)));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "Profiles", "Profiles"),
        if referencing_profiles.is_empty() {
            pick(ctx.lang, "<无>", "<none>").to_string()
        } else {
            referencing_profiles.join(", ")
        }
    ));
    ui.small(format_runtime_config_capability_label(
        ctx.lang,
        &cfg.capabilities,
    ))
    .on_hover_text(runtime_config_capability_hover_text(
        ctx.lang,
        &cfg.capabilities,
    ));

    if cfg.capabilities.model_catalog_kind == ModelCatalogKind::Declared
        && !cfg.capabilities.supported_models.is_empty()
    {
        let preview = cfg
            .capabilities
            .supported_models
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>();
        let suffix = if cfg.capabilities.supported_models.len() > preview.len() {
            format!(
                " … +{}",
                cfg.capabilities.supported_models.len() - preview.len()
            )
        } else {
            String::new()
        };
        ui.small(format!("models: {}{suffix}", preview.join(", ")));
    }
}
