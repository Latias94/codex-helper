use super::config_v2_header::render_config_v2_workspace_header;
use super::view_state::ConfigV2Section;
use super::*;

mod actions;
pub(super) mod context;
mod editors;
mod state;

use actions::*;
use context::*;
use editors::*;
use state::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);
    ui.label(pick(
        ctx.lang,
        "当前文件是 v2 station/provider 布局。这个工作台更适合个人中转站的日常管理：station 管路由，provider 管来源，profile 管 fast mode / 模型 / 思考模式。",
        "This page uses the v2 station/provider schema. The workspace is optimized for relay management: stations route, providers source traffic, profiles control fast mode, model, and reasoning.",
    ));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        let mut svc = ctx.view.config.service;
        egui::ComboBox::from_id_salt("config_form_v2_service")
            .selected_text(match svc {
                crate::config::ServiceKind::Codex => "codex",
                crate::config::ServiceKind::Claude => "claude",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
            });
        ctx.view.config.service = svc;
    });

    let Some(render_ctx) = ConfigV2RenderContext::build(ctx) else {
        return;
    };

    render_config_v2_workspace_header(
        ui,
        ctx.lang,
        ctx.proxy.kind(),
        &render_ctx,
        &mut ctx.view.config.v2_section,
    );
    ui.add_space(10.0);

    let mut actions = ConfigV2PendingActions::default();
    let mut draft = ConfigV2EditorDraft::from_view(&ctx.view.config);
    render_ctx.sync_draft(&mut draft);

    {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        let preview_station_specs = render_ctx.preview_station_specs();
        let preview_provider_catalog = render_ctx.preview_provider_catalog();
        let preview_runtime_station_catalog = render_ctx.preview_runtime_station_catalog();

        match ctx.view.config.v2_section {
            ConfigV2Section::Stations => {
                render_config_v2_stations_section(
                    ui,
                    StationsSectionArgs {
                        lang: ctx.lang,
                        proxy_kind: ctx.proxy.kind(),
                        last_error: ctx.last_error,
                        last_info: ctx.last_info,
                        view,
                        selected_service: render_ctx.selected_service,
                        schema_version: render_ctx.schema_version,
                        station_display_names: &render_ctx.station_display_names,
                        selected_name: &mut ctx.view.config.selected_name,
                        station_control_plane_enabled: render_ctx.station_control_plane_enabled,
                        station_structure_control_plane_enabled: render_ctx
                            .station_structure_control_plane_enabled,
                        station_structure_edit_enabled: render_ctx.station_structure_edit_enabled,
                        station_control_plane_catalog: &render_ctx.station_control_plane_catalog,
                        configured_active_name: render_ctx.configured_active_name.clone(),
                        effective_active_name: render_ctx.effective_active_name.clone(),
                        station_default_profile: render_ctx.station_default_profile.clone(),
                        attached_station_specs: render_ctx.attached_station_specs.as_ref(),
                        local_station_spec_catalog: &render_ctx.local_station_spec_catalog,
                        local_provider_ref_catalog: &render_ctx.local_provider_ref_catalog,
                        provider_catalog: &render_ctx.provider_catalog,
                        profile_catalog: &render_ctx.profile_catalog,
                        runtime_service: render_ctx.runtime_service.as_deref(),
                        supports_v1: render_ctx.supports_v1,
                        cfg_health: render_ctx.cfg_health.as_ref(),
                        hc_status: render_ctx.hc_status.as_ref(),
                        action_set_active: &mut actions.set_active,
                        action_clear_active: &mut actions.clear_active,
                        action_set_active_remote: &mut actions.set_active_remote,
                        action_save_apply: &mut actions.save_apply,
                        action_save_apply_remote: &mut actions.save_apply_remote,
                        action_upsert_station_spec_remote: &mut actions.upsert_station_spec_remote,
                        action_delete_station_spec_remote: &mut actions.delete_station_spec_remote,
                        action_probe_selected: &mut actions.probe_selected,
                        action_health_start: &mut actions.health_start,
                        action_health_cancel: &mut actions.health_cancel,
                        new_station_name: &mut draft.new_station_name,
                        station_editor_name: &mut draft.station_editor_name,
                        station_editor_alias: &mut draft.station_editor_alias,
                        station_editor_enabled: &mut draft.station_editor_enabled,
                        station_editor_level: &mut draft.station_editor_level,
                        station_editor_members: &mut draft.station_editor_members,
                    },
                );
            }
            ConfigV2Section::Providers => {
                render_config_v2_providers_section(
                    ui,
                    ctx.lang,
                    ctx.proxy.kind(),
                    ctx.last_error,
                    ctx.last_info,
                    view,
                    render_ctx.selected_service,
                    render_ctx.provider_structure_control_plane_enabled,
                    render_ctx.provider_structure_edit_enabled,
                    render_ctx.attached_provider_specs.as_ref(),
                    render_ctx.attached_station_specs.as_ref(),
                    &render_ctx.local_provider_spec_catalog,
                    &render_ctx.provider_display_names,
                    &mut draft.selected_provider_name,
                    &mut draft.new_provider_name,
                    &mut draft.provider_editor_name,
                    &mut draft.provider_editor_alias,
                    &mut draft.provider_editor_enabled,
                    &mut draft.provider_editor_auth_token_env,
                    &mut draft.provider_editor_api_key_env,
                    &mut draft.provider_editor_endpoints,
                    &mut actions.upsert_provider_spec_remote,
                    &mut actions.delete_provider_spec_remote,
                    &mut actions.save_apply,
                );
            }
            ConfigV2Section::Profiles => {
                ui.group(|ui| {
                    ui.heading(pick(ctx.lang, "Profiles", "Profiles"));
                    ui.label(pick(
                        ctx.lang,
                        "Profile 用于把 station / model / reasoning_effort / service_tier 组合成可复用控制模板；更适合表达 fast mode、模型切换和思考模式。",
                        "Profiles bundle station / model / reasoning_effort / service_tier into reusable control templates for fast mode, model switching, and reasoning mode.",
                    ));
                    if render_ctx.profile_control_plane_enabled {
                        render_config_v2_profiles_control_plane(
                            ui,
                            ctx.lang,
                            render_ctx.selected_service,
                            &render_ctx.profile_control_plane_catalog,
                            render_ctx.profile_control_plane_default.as_deref(),
                            &render_ctx.profile_control_plane_station_names,
                            &mut draft.selected_profile_name,
                            &mut draft.new_profile_name,
                            &mut draft.profile_editor_name,
                            &mut draft.profile_editor_extends,
                            &mut draft.profile_editor_station,
                            &mut draft.profile_editor_model,
                            &mut draft.profile_editor_reasoning_effort,
                            &mut draft.profile_editor_service_tier,
                            &mut draft.profile_error,
                            &mut actions.profile_upsert_remote,
                            &mut actions.profile_delete_remote,
                            &mut actions.profile_set_persisted_default_remote,
                            render_ctx.attached_mode,
                            render_ctx.station_control_plane_enabled,
                            render_ctx
                                .station_control_plane_configured_active
                                .as_deref(),
                            render_ctx
                                .station_control_plane_effective_active
                                .as_deref(),
                            preview_station_specs,
                            preview_provider_catalog,
                            preview_runtime_station_catalog,
                        );
                    } else {
                        render_config_v2_profiles_local(
                            ui,
                            LocalProfilesSectionArgs {
                                lang: ctx.lang,
                                selected_service: render_ctx.selected_service,
                                view,
                                station_names: &render_ctx.station_names,
                                selected_profile_name: &mut draft.selected_profile_name,
                                new_profile_name: &mut draft.new_profile_name,
                                profile_info: &mut draft.profile_info,
                                profile_error: &mut draft.profile_error,
                                action_save_apply: &mut actions.save_apply,
                                configured_active_name: render_ctx.configured_active_name.as_deref(),
                                effective_active_name: render_ctx.effective_active_name.as_deref(),
                                preview_station_specs,
                                preview_provider_catalog,
                                preview_runtime_station_catalog,
                            },
                        );
                    }
                });
            }
        }
    }

    let (profile_info, profile_error) = draft.persist_into_view(&mut ctx.view.config);
    if let Some(message) = profile_info {
        *ctx.last_info = Some(message);
    }
    if let Some(message) = profile_error {
        *ctx.last_error = Some(message);
    }
    actions.apply(ctx);
}
