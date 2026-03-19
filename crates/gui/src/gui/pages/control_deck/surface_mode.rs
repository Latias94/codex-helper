use super::*;

pub(super) fn control_surface_mode(
    proxy_kind: ProxyModeKind,
    direct_available: bool,
    local_edit_available: bool,
) -> ControlSurfaceMode {
    if direct_available {
        match proxy_kind {
            ProxyModeKind::Attached => ControlSurfaceMode::DirectRemote,
            ProxyModeKind::Running | ProxyModeKind::Starting | ProxyModeKind::Stopped => {
                ControlSurfaceMode::DirectLocal
            }
        }
    } else if local_edit_available {
        ControlSurfaceMode::LocalDraft
    } else {
        ControlSurfaceMode::Unavailable
    }
}

pub(super) fn control_scope_label(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) -> String {
    match control_surface_mode(
        proxy_kind,
        render_ctx.profile_control_plane_enabled || render_ctx.station_control_plane_enabled,
        !render_ctx.attached_mode,
    ) {
        ControlSurfaceMode::DirectRemote => pick(lang, "直写远端", "Remote control-plane").into(),
        ControlSurfaceMode::DirectLocal => pick(lang, "直写本机", "Local control-plane").into(),
        ControlSurfaceMode::LocalDraft => pick(lang, "本地文稿", "Local settings draft").into(),
        ControlSurfaceMode::Unavailable => pick(lang, "远端受限", "Remote-limited").into(),
    }
}

pub(super) fn control_scope_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) -> String {
    match control_surface_mode(
        proxy_kind,
        render_ctx.profile_control_plane_enabled || render_ctx.station_control_plane_enabled,
        !render_ctx.attached_mode,
    ) {
        ControlSurfaceMode::DirectRemote => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "变更会直接写到附着代理并刷新远端运行态",
                "Writes go straight to the attached proxy runtime",
            )
        ),
        ControlSurfaceMode::DirectLocal => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "变更通过本机 control-plane 落盘并刷新运行态",
                "Writes go through the local control plane and refresh runtime",
            )
        ),
        ControlSurfaceMode::LocalDraft => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "当前编辑本机设置文档，需重新运行或附着后生效",
                "Editing the local settings document; run or attach to apply",
            )
        ),
        ControlSurfaceMode::Unavailable => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "当前附着目标未暴露对应写接口，只能查看或切回本机文稿",
                "The attached target does not expose write APIs for this surface",
            )
        ),
    }
}

pub(super) fn station_summary_hint(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
    station_count: usize,
) -> String {
    let configured = render_ctx
        .configured_active_name
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<无>", "<none>"));
    let effective = render_ctx
        .effective_active_name
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"));
    format!(
        "{} {} · {} {} · {} {}",
        pick(lang, "配置", "cfg"),
        configured,
        pick(lang, "生效", "eff"),
        effective,
        pick(lang, "总数", "total"),
        station_count
    )
}

pub(super) fn profile_summary_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
    profile_count: usize,
) -> String {
    let surface = surface_mode_short_label(
        lang,
        control_surface_mode(
            proxy_kind,
            render_ctx.profile_control_plane_enabled,
            !render_ctx.attached_mode,
        ),
    );
    format!(
        "{} · {} {}",
        surface,
        pick(lang, "模板", "profiles"),
        profile_count
    )
}

pub(super) fn provider_summary_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
    provider_count: usize,
) -> String {
    let surface = surface_mode_short_label(
        lang,
        control_surface_mode(
            proxy_kind,
            render_ctx.provider_structure_control_plane_enabled,
            render_ctx.provider_structure_edit_enabled,
        ),
    );
    let lead = render_ctx
        .provider_display_names
        .first()
        .cloned()
        .unwrap_or_else(|| pick(lang, "未配置", "empty").to_string());
    format!(
        "{} · {} {} · {}",
        surface,
        pick(lang, "总数", "total"),
        provider_count,
        lead
    )
}

pub(super) fn render_surface_chip(
    ui: &mut egui::Ui,
    lang: Language,
    title: &str,
    mode: ControlSurfaceMode,
) {
    let (status, color) = surface_mode_chip(lang, mode);
    let text = format!("{title}: {status}");
    ui.label(egui::RichText::new(text).color(color));
}

fn surface_mode_short_label(lang: Language, mode: ControlSurfaceMode) -> &'static str {
    match mode {
        ControlSurfaceMode::DirectLocal => pick(lang, "本机直写", "local-direct"),
        ControlSurfaceMode::DirectRemote => pick(lang, "远端直写", "remote-direct"),
        ControlSurfaceMode::LocalDraft => pick(lang, "本地文稿", "local-draft"),
        ControlSurfaceMode::Unavailable => pick(lang, "不可用", "unavailable"),
    }
}

fn surface_mode_chip(lang: Language, mode: ControlSurfaceMode) -> (&'static str, egui::Color32) {
    match mode {
        ControlSurfaceMode::DirectLocal => (
            pick(lang, "本机直写", "local-direct"),
            egui::Color32::from_rgb(63, 120, 191),
        ),
        ControlSurfaceMode::DirectRemote => (
            pick(lang, "远端直写", "remote-direct"),
            egui::Color32::from_rgb(54, 153, 94),
        ),
        ControlSurfaceMode::LocalDraft => (
            pick(lang, "本地文稿", "local-draft"),
            egui::Color32::from_rgb(196, 140, 70),
        ),
        ControlSurfaceMode::Unavailable => (
            pick(lang, "不可用", "unavailable"),
            egui::Color32::from_rgb(160, 84, 84),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_surface_mode_prefers_direct_over_local_draft() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, true, true),
            ControlSurfaceMode::DirectRemote
        );
        assert_eq!(
            control_surface_mode(ProxyModeKind::Running, true, true),
            ControlSurfaceMode::DirectLocal
        );
    }

    #[test]
    fn control_surface_mode_falls_back_to_local_draft_when_editable() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, false, true),
            ControlSurfaceMode::LocalDraft
        );
    }

    #[test]
    fn control_surface_mode_marks_unavailable_when_no_surface_exists() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, false, false),
            ControlSurfaceMode::Unavailable
        );
    }
}
