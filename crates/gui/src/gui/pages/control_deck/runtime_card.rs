use super::*;

pub(super) fn render_control_deck_runtime_card(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) {
    let lang = ctx.lang;
    let banner = runtime_card_banner(lang, proxy_kind, render_ctx);
    let target_value = runtime_target_value(lang, render_ctx);
    let target_hint = runtime_target_hint(lang, proxy_kind, render_ctx);
    let retry_value = runtime_retry_value(lang, render_ctx);
    let retry_hint = runtime_retry_hint(lang, proxy_kind, render_ctx);
    let admin_value = runtime_admin_value(lang, render_ctx);
    let admin_hint = runtime_admin_hint(lang, render_ctx);

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.small(pick(lang, "当前运行态控制", "Current runtime control"));
                ui.heading(pick(lang, "Runtime Card", "Runtime Card"));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(proxy_mode_label(lang, proxy_kind))
                        .strong()
                        .color(egui::Color32::from_rgb(76, 114, 176)),
                );
            });
        });

        ui.small(banner);
        ui.add_space(6.0);

        ui.columns(3, |cols| {
            super::render_control_deck_summary_card(
                &mut cols[0],
                pick(lang, "Runtime target", "Runtime target"),
                target_value,
                &target_hint,
            );
            super::render_control_deck_summary_card(
                &mut cols[1],
                pick(lang, "Retry / Failover", "Retry / Failover"),
                retry_value,
                &retry_hint,
            );
            super::render_control_deck_summary_card(
                &mut cols[2],
                pick(lang, "Remote admin", "Remote admin"),
                admin_value,
                &admin_hint,
            );
        });

        if let Some(status_line) = operator_counts_status_line(lang, render_ctx) {
            ui.add_space(6.0);
            ui.small(status_line);
        }

        if let Some(status_line) = operator_health_status_line(lang, render_ctx) {
            ui.small(status_line);
        }

        if let Some(error) = render_ctx.runtime_last_error.as_deref() {
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                format!(
                    "{}: {error}",
                    pick(lang, "当前运行态错误", "Current runtime error")
                ),
            );
        }

        if let Some(message) = runtime_host_local_warning(lang, render_ctx) {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
        }

        if let Some(message) = runtime_admin_message(lang, render_ctx) {
            ui.add_space(6.0);
            let color = if render_ctx
                .runtime_remote_admin_access
                .as_ref()
                .is_some_and(|caps| caps.remote_enabled && remote_admin_token_present())
            {
                egui::Color32::from_rgb(60, 160, 90)
            } else {
                egui::Color32::from_rgb(120, 120, 120)
            };
            ui.colored_label(color, message);
        }
    });
}

pub(super) fn proxy_mode_label(lang: Language, proxy_kind: ProxyModeKind) -> &'static str {
    match proxy_kind {
        ProxyModeKind::Attached => pick(lang, "附着代理", "Attached proxy"),
        ProxyModeKind::Running => pick(lang, "本机运行", "Local runtime"),
        ProxyModeKind::Starting => pick(lang, "启动中", "Starting"),
        ProxyModeKind::Stopped => pick(lang, "本地文件", "Local file"),
    }
}

fn runtime_card_banner(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) -> String {
    if let Some(runtime_service) = render_ctx.runtime_service.as_deref() {
        if !render_ctx.runtime_matches_selected_service {
            return match lang {
                Language::Zh => format!(
                    "当前真正挂着的运行态服务是 {runtime_service}，而这页工作台聚焦的是 {}。顶部 station/profile 摘要代表当前工作台目标，不代表这个运行态本身。",
                    render_ctx.selected_service
                ),
                Language::En => format!(
                    "The live runtime is {runtime_service}, while this workspace is focused on {}. The station/profile summaries above describe the current workspace target, not that runtime itself.",
                    render_ctx.selected_service
                ),
            };
        }

        return match proxy_kind {
            ProxyModeKind::Attached => pick(
                lang,
                "这张卡片解释当前附着代理真正暴露出来的控制面：你是否能直写 retry、远端 admin 是否可用，以及当前 GUI 在 LAN/Tailscale 设备上会受哪些 host-local 限制。",
                "This card explains what the attached proxy actually exposes: whether retry is writable, whether remote admin is available, and which host-local capabilities stay unavailable from this LAN/Tailscale device.",
            )
            .to_string(),
            ProxyModeKind::Running => pick(
                lang,
                "这张卡片解释本机正在运行的代理：当前工作台是否与运行态对齐，retry/failover 怎样生效，以及哪些动作会直接命中本机 control-plane。",
                "This card explains the local runtime: whether the workspace is aligned with it, how retry/failover is currently applied, and which actions hit the local control plane directly.",
            )
            .to_string(),
            ProxyModeKind::Starting => pick(
                lang,
                "代理正在启动。工作台仍可继续编辑设置，但运行态信息会在下一次 refresh 后补齐。",
                "The proxy is starting. You can keep editing settings, and runtime details will fill in after the next refresh.",
            )
            .to_string(),
            ProxyModeKind::Stopped => pick(
                lang,
                "当前没有活动代理。这个工作台仍可整理 station/provider/profile 文稿，但不会直接作用到任何运行态。",
                "There is no active proxy. The workspace can still edit station/provider/profile documents, but nothing is applied to a live runtime yet.",
            )
            .to_string(),
        };
    }

    match proxy_kind {
        ProxyModeKind::Starting => pick(
            lang,
            "代理正在启动。工作台仍可继续编辑设置，但运行态信息会在下一次 refresh 后补齐。",
            "The proxy is starting. You can keep editing settings, and runtime details will fill in after the next refresh.",
        )
        .to_string(),
        ProxyModeKind::Stopped => pick(
            lang,
            "当前没有活动代理。这个工作台仍可整理 station/provider/profile 文稿，但不会直接作用到任何运行态。",
            "There is no active proxy. The workspace can still edit station/provider/profile documents, but nothing is applied to a live runtime yet.",
        )
        .to_string(),
        _ => pick(
            lang,
            "当前运行态信息还未就绪；先 refresh 一次控制面后再看这张卡片。",
            "Runtime details are not ready yet; refresh the control plane once and revisit this card.",
        )
        .to_string(),
    }
}

fn runtime_target_value(lang: Language, render_ctx: &ProxySettingsRenderContext) -> String {
    render_ctx
        .runtime_service
        .clone()
        .unwrap_or_else(|| pick(lang, "<未运行>", "<inactive>").to_string())
}

fn runtime_target_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) -> String {
    let mode = proxy_mode_label(lang, proxy_kind);
    let base_url = render_ctx
        .runtime_base_url
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<未知地址>", "<unknown endpoint>"));

    if let Some(runtime_service) = render_ctx.runtime_service.as_deref() {
        if !render_ctx.runtime_matches_selected_service {
            let base_hint = match lang {
                Language::Zh => format!(
                    "{mode} · {base_url} · 实际运行={runtime_service}，当前工作台={}",
                    render_ctx.selected_service
                ),
                Language::En => format!(
                    "{mode} · {base_url} · runtime={runtime_service}, workspace={}",
                    render_ctx.selected_service
                ),
            };
            return append_operator_runtime_scope(lang, render_ctx, base_hint);
        }

        let base_hint = match lang {
            Language::Zh => format!(
                "{mode} · {base_url} · 已与 {} 工作台对齐",
                render_ctx.selected_service
            ),
            Language::En => format!(
                "{mode} · {base_url} · aligned with {} workspace",
                render_ctx.selected_service
            ),
        };
        return append_operator_runtime_scope(lang, render_ctx, base_hint);
    }

    match lang {
        Language::Zh => format!("{mode} · {base_url} · 当前没有可解释的活动运行态"),
        Language::En => format!("{mode} · {base_url} · no live runtime is currently available"),
    }
}

fn runtime_retry_value(lang: Language, render_ctx: &ProxySettingsRenderContext) -> String {
    if render_ctx.runtime_service.is_none() {
        return pick(lang, "<未加载>", "<not loaded>").to_string();
    }

    if let Some(summary) = render_ctx.operator_retry_summary.as_ref() {
        return retry_profile_display_text(lang, summary.configured_profile);
    }

    if render_ctx.configured_retry.is_some()
        || render_ctx.resolved_retry.is_some()
        || render_ctx.supports_retry_config_api
    {
        return retry_profile_display_text(
            lang,
            render_ctx
                .configured_retry
                .as_ref()
                .and_then(|retry| retry.profile),
        );
    }

    pick(lang, "仅 resolved / 不可写", "Resolved only / read-only").to_string()
}

fn runtime_retry_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
) -> String {
    let supports_write = render_ctx
        .operator_retry_summary
        .as_ref()
        .map(|summary| summary.supports_write)
        .unwrap_or(render_ctx.supports_retry_config_api);
    let write_mode = if supports_write {
        match proxy_kind {
            ProxyModeKind::Attached => pick(lang, "可直写远端", "remote-writable"),
            ProxyModeKind::Running => pick(lang, "可直写本机", "local-writable"),
            ProxyModeKind::Starting | ProxyModeKind::Stopped => {
                pick(lang, "需运行后可写", "writable after runtime")
            }
        }
    } else {
        pick(lang, "只读 policy", "resolved-only")
    };

    if let Some(summary) = render_ctx.operator_retry_summary.as_ref() {
        let failover_scope = if summary.allow_cross_station_before_first_output {
            pick(
                lang,
                "首包前可跨 station",
                "cross-station before first output",
            )
        } else {
            pick(lang, "先耗尽当前 station", "current station first")
        };
        let recent_observations = format!(
            " · {} retry={} · {}={} · {}={} · {}={}",
            pick(lang, "最近", "recent"),
            summary.recent_retried_requests,
            pick(lang, "同站", "same-station"),
            summary.recent_same_station_retries,
            pick(lang, "跨站", "cross-station"),
            summary.recent_cross_station_failovers,
            pick(lang, "fast", "fast"),
            summary.recent_fast_mode_requests,
        );
        return format!(
            "{write_mode} · {}={} · {}={} · {failover_scope}{recent_observations}",
            pick(lang, "上游尝试", "upstream tries"),
            summary.upstream_max_attempts,
            pick(lang, "Provider 尝试", "provider tries"),
            summary.provider_max_attempts,
        );
    }

    if let Some(retry) = render_ctx.resolved_retry.as_ref() {
        let failover_scope = if retry.allow_cross_station_before_first_output {
            pick(
                lang,
                "首包前可跨 station",
                "cross-station before first output",
            )
        } else {
            pick(lang, "先耗尽当前 station", "current station first")
        };
        return format!(
            "{write_mode} · {}={} · {}={} · {failover_scope}",
            pick(lang, "上游尝试", "upstream tries"),
            retry.upstream.max_attempts,
            pick(lang, "Provider 尝试", "provider tries"),
            retry.provider.max_attempts,
        );
    }

    if render_ctx.runtime_service.is_some() {
        return format!(
            "{write_mode} · {}",
            pick(
                lang,
                "运行态还没有返回 resolved retry policy",
                "The runtime has not returned a resolved retry policy yet",
            )
        );
    }

    pick(
        lang,
        "当前没有活动运行态，所以这里还没有 retry/failover 信息。",
        "There is no active runtime yet, so retry/failover details are unavailable.",
    )
    .to_string()
}

fn runtime_admin_value(lang: Language, render_ctx: &ProxySettingsRenderContext) -> String {
    let Some(admin_base_url) = render_ctx.runtime_admin_base_url.as_deref() else {
        return pick(lang, "<未连接>", "<offline>").to_string();
    };
    let Some(caps) = render_ctx.runtime_remote_admin_access.as_ref() else {
        return pick(lang, "状态未知", "Unknown").to_string();
    };

    remote_admin_access_short_label(admin_base_url, caps, lang).unwrap_or_else(|| {
        if management_base_url_is_loopback(admin_base_url) {
            pick(lang, "本机 loopback", "Local loopback").to_string()
        } else {
            pick(lang, "远端已连接", "Remote attached").to_string()
        }
    })
}

fn runtime_admin_hint(lang: Language, render_ctx: &ProxySettingsRenderContext) -> String {
    let Some(admin_base_url) = render_ctx.runtime_admin_base_url.as_deref() else {
        return pick(
            lang,
            "当前没有可解释的 admin 控制面。",
            "No admin control plane is currently available.",
        )
        .to_string();
    };
    let Some(caps) = render_ctx.runtime_remote_admin_access.as_ref() else {
        return format!("admin={admin_base_url}");
    };

    if management_base_url_is_loopback(admin_base_url) {
        return match lang {
            Language::Zh => format!("admin={admin_base_url} · 当前设备可直接访问"),
            Language::En => format!("admin={admin_base_url} · directly reachable from this device"),
        };
    }

    if !caps.remote_enabled {
        return match lang {
            Language::Zh => format!(
                "admin={admin_base_url} · 需在代理主机设置 {} 才能开放 LAN/Tailscale 附着",
                caps.token_env_var
            ),
            Language::En => format!(
                "admin={admin_base_url} · enable {} on the proxy host for LAN/Tailscale attach",
                caps.token_env_var
            ),
        };
    }

    format!(
        "admin={admin_base_url} · header={} · env={}",
        caps.token_header, caps.token_env_var
    )
}

fn runtime_host_local_warning(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
) -> Option<String> {
    let admin_base_url = render_ctx.runtime_admin_base_url.as_deref()?;
    let caps = render_ctx.runtime_host_local_capabilities.as_ref()?;
    remote_local_only_warning_message(
        admin_base_url,
        caps,
        lang,
        &[
            pick(lang, "cwd", "cwd"),
            pick(lang, "transcript", "transcript"),
            pick(lang, "resume", "resume"),
            pick(lang, "open file", "open file"),
        ],
    )
}

fn runtime_admin_message(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
) -> Option<String> {
    let admin_base_url = render_ctx.runtime_admin_base_url.as_deref()?;
    let caps = render_ctx.runtime_remote_admin_access.as_ref()?;
    remote_admin_access_message(admin_base_url, caps, lang)
}

fn operator_counts_status_line(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
) -> Option<String> {
    if let Some(counts) = render_ctx.operator_counts.as_ref() {
        return Some(format!(
            "{}={} · {}={} · {}={} · {}={} · {}={} · {}={}",
            pick(lang, "active", "active"),
            counts.active_requests,
            pick(lang, "recent", "recent"),
            counts.recent_requests,
            pick(lang, "sessions", "sessions"),
            counts.sessions,
            pick(lang, "stations", "stations"),
            counts.stations,
            pick(lang, "profiles", "profiles"),
            counts.profiles,
            pick(lang, "providers", "providers"),
            counts.providers,
        ));
    }

    if render_ctx.runtime_service.is_some() && render_ctx.supports_operator_summary_api {
        return Some(
            pick(
                lang,
                "operator summary 已声明可用，但这次 refresh 还没拿到聚合计数。",
                "operator summary is available, but this refresh has not returned aggregate counts yet.",
            )
            .to_string(),
        );
    }

    None
}

fn operator_health_status_line(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
) -> Option<String> {
    if let Some(summary) = render_ctx.operator_health_summary.as_ref() {
        let is_stable = summary.stations_draining == 0
            && summary.stations_breaker_open == 0
            && summary.stations_half_open == 0
            && summary.stations_with_active_health_checks == 0
            && summary.stations_with_probe_failures == 0
            && summary.stations_with_degraded_passive_health == 0
            && summary.stations_with_failing_passive_health == 0
            && summary.stations_with_cooldown == 0
            && summary.stations_with_usage_exhaustion == 0;
        if is_stable {
            return Some(
                pick(
                    lang,
                    "health=stable · hc=0 · cooldown=0 · quota=0",
                    "health=stable · hc=0 · cooldown=0 · quota=0",
                )
                .to_string(),
            );
        }

        return Some(format!(
            "{}={} · {}={} · {}={} · {}={} · {}={} · {}={}/{} · {}={} · {}={}",
            pick(lang, "drain", "drain"),
            summary.stations_draining,
            pick(lang, "breaker", "breaker"),
            summary.stations_breaker_open,
            pick(lang, "half-open", "half-open"),
            summary.stations_half_open,
            pick(lang, "hc", "hc"),
            summary.stations_with_active_health_checks,
            pick(lang, "probe err", "probe err"),
            summary.stations_with_probe_failures,
            pick(lang, "passive dg/fail", "passive dg/fail"),
            summary.stations_with_degraded_passive_health,
            summary.stations_with_failing_passive_health,
            pick(lang, "cooldown", "cooldown"),
            summary.stations_with_cooldown,
            pick(lang, "quota", "quota"),
            summary.stations_with_usage_exhaustion,
        ));
    }

    if render_ctx.runtime_service.is_some() && render_ctx.supports_operator_summary_api {
        return Some(
            pick(
                lang,
                "operator summary 已声明可用，但这次 refresh 还没拿到 health posture。",
                "operator summary is available, but this refresh has not returned health posture yet.",
            )
            .to_string(),
        );
    }

    None
}

fn append_operator_runtime_scope(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
    base_hint: String,
) -> String {
    let Some(scope_hint) = operator_runtime_scope_hint(lang, render_ctx) else {
        return base_hint;
    };
    format!("{base_hint} · {scope_hint}")
}

fn operator_runtime_scope_hint(
    lang: Language,
    render_ctx: &ProxySettingsRenderContext,
) -> Option<String> {
    let summary = render_ctx.operator_runtime_summary.as_ref()?;
    let mut parts = Vec::new();

    if let Some(station) = summary
        .effective_active_station
        .as_deref()
        .or(summary.configured_active_station.as_deref())
    {
        parts.push(format!(
            "{}={station}",
            pick(lang, "active station", "active station")
        ));
    }

    if let Some(station) = summary.global_station_override.as_deref() {
        parts.push(format!(
            "{}={station}",
            pick(lang, "global override", "global override")
        ));
    }

    if let Some(profile) = summary.default_profile_summary.as_ref() {
        let profile_suffix = if profile.fast_mode {
            pick(lang, " (fast mode)", " (fast mode)")
        } else {
            ""
        };
        parts.push(format!(
            "{}={}{}",
            pick(lang, "default profile", "default profile"),
            profile.name,
            profile_suffix
        ));
    } else if let Some(profile) = summary.default_profile.as_deref() {
        parts.push(format!(
            "{}={profile}",
            pick(lang, "default profile", "default profile")
        ));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}
