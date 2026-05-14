use crate::dashboard_core::{ProviderEndpointOption, ProviderOption};

use super::*;

pub(super) fn render_attached_proxy_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let (
        base_url,
        active_len,
        recent_len,
        api_version,
        service_name,
        runtime_loaded_at_ms,
        runtime_source_mtime_ms,
        last_error,
        global_station_override,
        global_route_target_override,
        supports_global_route_target_override,
        admin_base_url,
        host_local_capabilities,
        remote_admin_access,
        providers,
    ) = {
        let Some(attached) = ctx.proxy.attached() else {
            return;
        };

        (
            attached.base_url.clone(),
            attached.active.len(),
            attached.recent.len(),
            attached.api_version,
            attached.service_name.clone(),
            attached.runtime_loaded_at_ms,
            attached.runtime_source_mtime_ms,
            attached.last_error.clone(),
            attached.global_station_override.clone(),
            attached.global_route_target_override.clone(),
            attached.supports_global_route_target_override,
            attached.admin_base_url.clone(),
            attached.host_local_capabilities.clone(),
            attached.remote_admin_access.clone(),
            attached.providers.clone(),
        )
    };

    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "已附着", "Attached"),
        base_url
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "活跃请求", "Active requests"),
        active_len
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
        recent_len
    ));
    if let Some(version) = api_version {
        ui.label(format!(
            "{}: v{}",
            pick(ctx.lang, "API 版本", "API version"),
            version
        ));
    }
    if let Some(service) = service_name.as_deref() {
        ui.label(format!("{}: {service}", pick(ctx.lang, "服务", "Service")));
    }
    if let Some(loaded_at_ms) = runtime_loaded_at_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行态配置 loaded_at_ms", "runtime loaded_at_ms"),
            loaded_at_ms
        ));
    }
    if let Some(mtime_ms) = runtime_source_mtime_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行态配置 mtime_ms", "runtime mtime_ms"),
            mtime_ms
        ));
    }
    if let Some(error) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
    }
    ui.label(format!(
        "{}: {}",
        if supports_global_route_target_override {
            pick(ctx.lang, "全局 route target", "Global route target")
        } else {
            pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)")
        },
        if supports_global_route_target_override {
            global_route_target_override.as_deref()
        } else {
            global_station_override.as_deref()
        }
        .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));
    render_attached_provider_runtime_section(ui, ctx, providers.as_slice());
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "提示：附着模式下不会改你的本机配置文件，但如果远端代理支持 API v1 扩展，上方的运行时控制仍可直接作用于该代理进程。",
            "Tip: attached mode won't change your local config file, but runtime controls above can still act on the remote proxy process when supported.",
        ),
    );
    if let Some(warning) = remote_local_only_warning_message(
        admin_base_url.as_str(),
        &host_local_capabilities,
        ctx.lang,
        &[
            pick(ctx.lang, "cwd", "cwd"),
            pick(ctx.lang, "transcript", "transcript"),
            pick(ctx.lang, "resume", "resume"),
            pick(ctx.lang, "open file", "open file"),
        ],
    ) {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), warning);
    }
    if let Some(label) =
        remote_admin_access_short_label(admin_base_url.as_str(), &remote_admin_access, ctx.lang)
    {
        let color = if remote_admin_access.remote_enabled && remote_admin_token_present() {
            egui::Color32::from_rgb(60, 160, 90)
        } else {
            egui::Color32::from_rgb(200, 120, 40)
        };
        let response = ui.colored_label(color, label);
        let remote_admin_message =
            remote_admin_access_message(admin_base_url.as_str(), &remote_admin_access, ctx.lang);
        if let Some(message) = remote_admin_message.clone() {
            response.on_hover_text(message.clone());
            if !remote_admin_access.remote_enabled || !remote_admin_token_present() {
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
            }
        }
    }
}

fn render_attached_provider_runtime_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    providers: &[ProviderOption],
) {
    if providers.is_empty() {
        return;
    }

    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "Provider runtime", "Provider runtime"));
    egui::ScrollArea::vertical()
        .id_salt("attached_provider_runtime_scroll")
        .max_height(220.0)
        .show(ui, |ui| {
            for provider in providers {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format_provider_display(provider));
                        ui.small(format!(
                            "{}: {}",
                            pick(ctx.lang, "routable", "routable"),
                            provider.routable_endpoints
                        ));
                        ui.small(format!(
                            "{}: {}",
                            pick(ctx.lang, "effective", "effective"),
                            if provider.effective_enabled {
                                pick(ctx.lang, "on", "on")
                            } else {
                                pick(ctx.lang, "off", "off")
                            }
                        ));
                    });
                    ui.add_space(4.0);
                    for endpoint in &provider.endpoints {
                        render_attached_provider_endpoint_row(ui, ctx, provider, endpoint);
                    }
                });
                ui.add_space(4.0);
            }
        });
}

fn render_attached_provider_endpoint_row(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    provider: &ProviderOption,
    endpoint: &ProviderEndpointOption,
) {
    ui.horizontal_wrapped(|ui| {
        ui.monospace(format_attached_provider_endpoint_identity(
            provider.name.as_str(),
            endpoint.name.as_str(),
        ));
        ui.small(format!("base={}", shorten_middle(&endpoint.base_url, 56)));
        if !endpoint.routable {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(ctx.lang, "不可路由", "Not routable"),
            );
        }
        if endpoint.runtime_enabled_override.is_some() || endpoint.runtime_state_override.is_some()
        {
            let mut overrides = Vec::new();
            if let Some(enabled) = endpoint.runtime_enabled_override {
                overrides.push(format!(
                    "{}={}",
                    pick(ctx.lang, "enabled", "enabled"),
                    enabled
                ));
            }
            if let Some(state) = endpoint.runtime_state_override {
                overrides.push(format!(
                    "{}={}",
                    pick(ctx.lang, "state", "state"),
                    runtime_config_state_label(ctx.lang, state)
                ));
            }
            ui.small(format!(
                "{}: {}",
                pick(ctx.lang, "覆盖", "Override"),
                overrides.join(", ")
            ));
        }
    });

    ui.horizontal(|ui| {
        let mut enabled = endpoint.effective_enabled;
        if ui
            .checkbox(&mut enabled, pick(ctx.lang, "启用", "Enabled"))
            .changed()
        {
            match ctx.proxy.set_provider_runtime_meta(
                ctx.rt,
                provider.name.clone(),
                Some(endpoint.name.clone()),
                Some(Some(enabled)),
                None,
            ) {
                Ok(()) => {
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已更新 provider runtime 覆盖",
                            "Provider runtime override updated",
                        )
                        .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error =
                        Some(format!("apply provider runtime override failed: {error}"));
                }
            }
        }

        let mut runtime_state = endpoint.runtime_state;
        egui::ComboBox::from_id_salt((
            "attached_provider_runtime_state",
            provider.name.as_str(),
            endpoint.name.as_str(),
        ))
        .selected_text(runtime_config_state_label(ctx.lang, runtime_state))
        .show_ui(ui, |ui| {
            for candidate in [
                RuntimeConfigState::Normal,
                RuntimeConfigState::Draining,
                RuntimeConfigState::BreakerOpen,
                RuntimeConfigState::HalfOpen,
            ] {
                ui.selectable_value(
                    &mut runtime_state,
                    candidate,
                    runtime_config_state_label(ctx.lang, candidate),
                );
            }
        });
        if runtime_state != endpoint.runtime_state {
            match ctx.proxy.set_provider_runtime_meta(
                ctx.rt,
                provider.name.clone(),
                Some(endpoint.name.clone()),
                None,
                Some(Some(runtime_state)),
            ) {
                Ok(()) => {
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已更新 provider runtime 状态",
                            "Provider runtime state updated",
                        )
                        .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error = Some(format!("apply provider runtime state failed: {error}"));
                }
            }
        }

        let can_clear = endpoint.runtime_enabled_override.is_some()
            || endpoint.runtime_state_override.is_some();
        if ui
            .add_enabled(
                can_clear,
                egui::Button::new(pick(ctx.lang, "清除覆盖", "Clear override")),
            )
            .clicked()
        {
            match ctx.proxy.set_provider_runtime_meta(
                ctx.rt,
                provider.name.clone(),
                Some(endpoint.name.clone()),
                endpoint.runtime_enabled_override.map(|_| None),
                endpoint.runtime_state_override.map(|_| None),
            ) {
                Ok(()) => {
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已清除 provider runtime 覆盖",
                            "Provider runtime override cleared",
                        )
                        .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error =
                        Some(format!("clear provider runtime override failed: {error}"));
                }
            }
        }
    });
}

fn format_provider_display(provider: &ProviderOption) -> String {
    provider
        .alias
        .as_deref()
        .filter(|alias| !alias.trim().is_empty())
        .map(|alias| format!("{} ({alias})", provider.name))
        .unwrap_or_else(|| provider.name.clone())
}

fn format_attached_provider_endpoint_identity(provider_name: &str, endpoint_name: &str) -> String {
    format_route_decision_provider_endpoint(Some(provider_name), Some(endpoint_name))
        .unwrap_or_else(|| format!("{provider_name}/{endpoint_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_attached_provider_endpoint_identity_prefers_provider_endpoint_identity() {
        assert_eq!(
            format_attached_provider_endpoint_identity("alpha", "default"),
            "alpha/default"
        );
    }
}
