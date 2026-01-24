use eframe::egui;

use std::collections::HashMap;

use super::autostart;
use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::proxy_control::{GuiConfigOption, PortInUseAction, ProxyModeKind};
use crate::sessions::{SessionSummary, SessionTranscriptMessage};
use crate::state::{ActiveRequest, FinishedRequest, SessionStats};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Config,
    Sessions,
    Requests,
    History,
    Settings,
}

#[derive(Debug, Default)]
pub struct ViewState {
    pub sessions: SessionsViewState,
    pub requests: RequestsViewState,
    pub config: ConfigViewState,
    pub history: HistoryViewState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigMode {
    Form,
    Raw,
}

impl Default for ConfigMode {
    fn default() -> Self {
        Self::Form
    }
}

#[derive(Debug)]
pub struct ConfigViewState {
    mode: ConfigMode,
    service: crate::config::ServiceKind,
    selected_name: Option<String>,
    working: Option<crate::config::ProxyConfig>,
    load_error: Option<String>,
}

impl Default for ConfigViewState {
    fn default() -> Self {
        Self {
            mode: ConfigMode::Form,
            service: crate::config::ServiceKind::Codex,
            selected_name: None,
            working: None,
            load_error: None,
        }
    }
}

#[derive(Debug)]
pub struct HistoryViewState {
    pub query: String,
    pub sessions: Vec<SessionSummary>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub selected_idx: usize,
    pub selected_id: Option<String>,
    pub auto_load_transcript: bool,
    pub transcript_tail: usize,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
    pub transcript_error: Option<String>,
    loaded_for: Option<(String, usize)>,
}

impl Default for HistoryViewState {
    fn default() -> Self {
        Self {
            query: String::new(),
            sessions: Vec::new(),
            last_error: None,
            loaded_at_ms: None,
            selected_idx: 0,
            selected_id: None,
            auto_load_transcript: true,
            transcript_tail: 80,
            transcript_messages: Vec::new(),
            transcript_error: None,
            loaded_for: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionsViewState {
    pub active_only: bool,
    pub errors_only: bool,
    pub overrides_only: bool,
    pub selected_session_id: Option<String>,
    pub selected_idx: usize,
    editor: SessionOverrideEditor,
}

#[derive(Debug)]
pub struct RequestsViewState {
    pub errors_only: bool,
    pub scope_session: bool,
    pub selected_idx: usize,
}

impl Default for RequestsViewState {
    fn default() -> Self {
        Self {
            errors_only: false,
            scope_session: true,
            selected_idx: 0,
        }
    }
}

#[derive(Debug, Default)]
struct SessionOverrideEditor {
    sid: Option<String>,
    config_override: Option<String>,
    effort_override: Option<String>,
    custom_effort: String,
}

pub struct PageCtx<'a> {
    pub lang: Language,
    pub view: &'a mut ViewState,
    pub gui_cfg: &'a mut GuiConfig,
    pub proxy_config_text: &'a mut String,
    pub proxy_config_path: &'a std::path::Path,
    pub last_error: &'a mut Option<String>,
    pub last_info: &'a mut Option<String>,
    pub rt: &'a tokio::runtime::Runtime,
    pub proxy: &'a mut super::proxy_control::ProxyController,
}

pub fn nav(ui: &mut egui::Ui, lang: Language, current: &mut Page) {
    ui.horizontal(|ui| {
        let items = [
            (Page::Overview, pick(lang, "总览", "Overview")),
            (Page::Config, pick(lang, "配置", "Config")),
            (Page::Sessions, pick(lang, "会话", "Sessions")),
            (Page::Requests, pick(lang, "请求", "Requests")),
            (Page::History, pick(lang, "历史", "History")),
            (Page::Settings, pick(lang, "设置", "Settings")),
        ];
        for (p, label) in items {
            if ui.selectable_label(*current == p, label).clicked() {
                *current = p;
            }
        }
    });
    ui.separator();
}

pub fn render(ui: &mut egui::Ui, page: Page, ctx: &mut PageCtx<'_>) {
    match page {
        Page::Overview => render_overview(ui, ctx),
        Page::Config => render_config(ui, ctx),
        Page::Sessions => render_sessions(ui, ctx),
        Page::Requests => render_requests(ui, ctx),
        Page::History => render_history(ui, ctx),
        Page::Settings => render_settings(ui, ctx),
    }
}

fn render_overview(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "总览", "Overview"));

    ui.separator();

    // Sync defaults from GUI config (so Settings changes take effect without restart).
    // Avoid overriding the UI state while running/attached.
    if matches!(ctx.proxy.kind(), ProxyModeKind::Stopped) {
        ctx.proxy
            .set_defaults(ctx.gui_cfg.proxy.default_port, ctx.gui_cfg.service_kind());
    }

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        let mut svc = ctx.proxy.desired_service();
        egui::ComboBox::from_id_salt("proxy_service")
            .selected_text(match svc {
                crate::config::ServiceKind::Codex => "codex",
                crate::config::ServiceKind::Claude => "claude",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
            });
        if svc != ctx.proxy.desired_service() {
            ctx.proxy.set_desired_service(svc);
            ctx.gui_cfg.set_service_kind(svc);
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }

        ui.add_space(12.0);
        ui.label(pick(ctx.lang, "端口", "Port"));
        let mut port = ctx.proxy.desired_port();
        ui.add(egui::DragValue::new(&mut port).range(1..=65535));
        if port != ctx.proxy.desired_port() {
            ctx.proxy.set_desired_port(port);
            ctx.gui_cfg.proxy.default_port = port;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.add_space(6.0);

    match ctx.proxy.kind() {
        ProxyModeKind::Stopped => {
            if ui
                .button(pick(ctx.lang, "启动代理", "Start proxy"))
                .clicked()
            {
                let action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
                ctx.proxy.request_start_or_prompt(
                    ctx.rt,
                    action,
                    ctx.gui_cfg.attach.remember_choice,
                );

                if let Some(e) = ctx.proxy.last_start_error() {
                    *ctx.last_error = Some(e.to_string());
                }
            }
        }
        ProxyModeKind::Starting => {
            ui.label(pick(ctx.lang, "正在启动…", "Starting..."));
        }
        ProxyModeKind::Running => {
            if let Some(r) = ctx.proxy.running() {
                ui.label(format!(
                    "{}: 127.0.0.1:{} ({})",
                    pick(ctx.lang, "运行中", "Running"),
                    r.port,
                    r.service_name
                ));
                if let Some(err) = r.last_error.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
                }

                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "活跃请求", "Active requests"),
                    r.active.len()
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
                    r.recent.len()
                ));

                let active_name = match r.service_name {
                    "claude" => r.cfg.claude.active.clone(),
                    _ => r.cfg.codex.active.clone(),
                };
                let active_fallback = match r.service_name {
                    "claude" => r.cfg.claude.active_config().map(|c| c.name.clone()),
                    _ => r.cfg.codex.active_config().map(|c| c.name.clone()),
                };
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "当前配置(active)", "Active config"),
                    active_name
                        .or(active_fallback)
                        .unwrap_or_else(|| "-".to_string())
                ));

                let warnings =
                    crate::config::model_routing_warnings(r.cfg.as_ref(), r.service_name);
                if !warnings.is_empty() {
                    ui.add_space(4.0);
                    ui.label(pick(
                        ctx.lang,
                        "模型路由配置警告（建议处理）：",
                        "Model routing warnings (recommended to fix):",
                    ));
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for w in warnings {
                                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), w);
                            }
                        });
                }
            }
            if ui
                .button(pick(ctx.lang, "停止代理", "Stop proxy"))
                .clicked()
            {
                if let Err(e) = ctx.proxy.stop(ctx.rt) {
                    *ctx.last_error = Some(format!("stop failed: {e}"));
                } else {
                    *ctx.last_info = Some(pick(ctx.lang, "已停止", "Stopped").to_string());
                }
            }
        }
        ProxyModeKind::Attached => {
            if let Some(att) = ctx.proxy.attached() {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "已附着", "Attached"),
                    att.base_url
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "活跃请求", "Active requests"),
                    att.active.len()
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
                    att.recent.len()
                ));
                if let Some(v) = att.api_version {
                    ui.label(format!(
                        "{}: v{}",
                        pick(ctx.lang, "API 版本", "API version"),
                        v
                    ));
                }
                if let Some(svc) = att.service_name.as_deref() {
                    ui.label(format!("{}: {svc}", pick(ctx.lang, "服务", "Service")));
                }
                if let Some(ms) = att.runtime_loaded_at_ms {
                    ui.label(format!(
                        "{}: {}",
                        pick(ctx.lang, "运行态配置 loaded_at_ms", "runtime loaded_at_ms"),
                        ms
                    ));
                }
                if let Some(ms) = att.runtime_source_mtime_ms {
                    ui.label(format!(
                        "{}: {}",
                        pick(ctx.lang, "运行态配置 mtime_ms", "runtime mtime_ms"),
                        ms
                    ));
                }
                if let Some(err) = att.last_error.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
                }
            }
            ui.horizontal(|ui| {
                if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
                    ctx.proxy
                        .refresh_attached_if_due(ctx.rt, std::time::Duration::from_secs(0));
                }
                if ui.button(pick(ctx.lang, "取消附着", "Detach")).clicked() {
                    ctx.proxy.clear_port_in_use_modal();
                    ctx.proxy.detach();
                    *ctx.last_info = Some(pick(ctx.lang, "已取消附着", "Detached").to_string());
                }
            });
        }
    }

    // Port-in-use modal (only shown when action is Ask).
    if ctx.proxy.show_port_in_use_modal() {
        let mut open = true;
        egui::Window::new(pick(ctx.lang, "端口已被占用", "Port is in use"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let port = ctx.proxy.desired_port();
                ui.label(format!(
                    "{}: 127.0.0.1:{}",
                    pick(ctx.lang, "监听端口冲突", "Bind conflict"),
                    port
                ));
                ui.add_space(8.0);

                let mut remember = ctx.proxy.port_in_use_modal_remember();
                ui.checkbox(
                    &mut remember,
                    pick(
                        ctx.lang,
                        "记住我的选择（下次不再弹窗）",
                        "Remember my choice",
                    ),
                );
                ctx.proxy.set_port_in_use_modal_remember(remember);

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "附着到现有代理", "Attach"))
                        .clicked()
                    {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::Attach.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_attach();
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "换端口启动", "Start on another port"));
                    let mut p = ctx
                        .proxy
                        .port_in_use_modal_suggested_port()
                        .unwrap_or(port.saturating_add(1));
                    ui.add(egui::DragValue::new(&mut p).range(1..=65535));
                    ctx.proxy.set_port_in_use_modal_new_port(p);
                    if ui.button(pick(ctx.lang, "启动", "Start")).clicked() {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::StartNewPort.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_new_port(ctx.rt);
                    }
                });

                ui.horizontal(|ui| {
                    if ui.button(pick(ctx.lang, "退出", "Exit")).clicked() {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::Exit.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_exit();
                    }
                });
            });

        if !open {
            ctx.proxy.clear_port_in_use_modal();
        }
    }
}

fn render_sessions(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "会话", "Sessions"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看会话。",
            "No proxy is running or attached. Start or attach on Overview to view sessions.",
        ));
        return;
    };

    let last_error = snapshot.last_error.clone();
    let active = snapshot.active.clone();
    let recent = snapshot.recent.clone();
    let global_override = snapshot.global_override.clone();
    let session_effort_overrides = snapshot.session_effort_overrides.clone();
    let session_config_overrides = snapshot.session_config_overrides.clone();
    let session_stats = snapshot.session_stats.clone();

    if let Some(err) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.sessions.active_only,
            pick(ctx.lang, "仅活跃", "Active only"),
        );
        ui.checkbox(
            &mut ctx.view.sessions.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        ui.checkbox(
            &mut ctx.view.sessions.overrides_only,
            pick(ctx.lang, "仅覆盖", "Overrides only"),
        );
    });

    ui.add_space(6.0);

    let rows = build_session_rows(
        active,
        &recent,
        &session_effort_overrides,
        &session_config_overrides,
        &session_stats,
    );

    let filtered = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            if ctx.view.sessions.active_only && row.active_count == 0 {
                return false;
            }
            if ctx.view.sessions.errors_only && row.last_status.is_some_and(|s| s < 400) {
                return false;
            }
            if ctx.view.sessions.overrides_only
                && row.override_effort.is_none()
                && row.override_config_name.is_none()
            {
                return false;
            }
            true
        })
        .take(400)
        .collect::<Vec<_>>();

    // Stable selection: prefer session_id match, else keep previous index.
    let selected_idx_in_filtered = ctx
        .view
        .sessions
        .selected_session_id
        .as_deref()
        .and_then(|sid| {
            filtered
                .iter()
                .position(|(_, row)| row.session_id.as_deref() == Some(sid))
        })
        .unwrap_or(
            ctx.view
                .sessions
                .selected_idx
                .min(filtered.len().saturating_sub(1)),
        );

    ctx.view.sessions.selected_idx = selected_idx_in_filtered;
    let selected = filtered
        .get(ctx.view.sessions.selected_idx)
        .map(|(_, row)| *row);
    ctx.view.sessions.selected_session_id = selected.and_then(|r| r.session_id.clone());

    // Sync editor to the selected session, but do not clobber while editing the same session.
    if ctx.view.sessions.editor.sid != ctx.view.sessions.selected_session_id {
        ctx.view.sessions.editor.sid = ctx.view.sessions.selected_session_id.clone();
        ctx.view.sessions.editor.config_override =
            selected.and_then(|r| r.override_config_name.clone());
        ctx.view.sessions.editor.effort_override = selected.and_then(|r| r.override_effort.clone());
        ctx.view.sessions.editor.custom_effort = selected
            .and_then(|r| r.override_effort.clone())
            .unwrap_or_default();
    }

    let mut force_refresh = false;
    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "列表", "List"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                let now = now_ms();
                for (pos, (_, row)) in filtered.iter().enumerate() {
                    let selected = pos == ctx.view.sessions.selected_idx;
                    let sid = row
                        .session_id
                        .as_deref()
                        .map(|s| short_sid(s, 16))
                        .unwrap_or_else(|| {
                            pick(ctx.lang, "<全部/未知>", "<all/unknown>").to_string()
                        });
                    let cwd = row
                        .cwd
                        .as_deref()
                        .map(|s| shorten(basename(s), 18))
                        .unwrap_or_else(|| "-".to_string());
                    let active = if row.active_count > 0 {
                        format!("a={}", row.active_count)
                    } else {
                        "a=0".to_string()
                    };
                    let st = row
                        .last_status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let last = format_age(now, row.last_ended_at_ms);
                    let pin = row
                        .override_config_name
                        .as_deref()
                        .map(|s| shorten(s, 12))
                        .unwrap_or_else(|| "-".to_string());
                    let label = format!("{sid}  {cwd}  {active}  st={st}  last={last}  pin={pin}");
                    if ui.selectable_label(selected, label).clicked() {
                        ctx.view.sessions.selected_idx = pos;
                        ctx.view.sessions.selected_session_id = row.session_id.clone();
                    }
                }
            });

        cols[1].heading(pick(ctx.lang, "详情", "Details"));
        cols[1].add_space(4.0);

        let Some(row) = selected else {
            cols[1].label(pick(ctx.lang, "无会话数据。", "No session data."));
            return;
        };

        let sid_full = row.session_id.as_deref().unwrap_or("-");
        let cwd_full = row.cwd.as_deref().unwrap_or("-");
        let model = row.last_model.as_deref().unwrap_or("-");
        let provider = row.last_provider_id.as_deref().unwrap_or("-");
        let last_cfg = row.last_config_name.as_deref().unwrap_or("-");
        let effort_last = row.last_reasoning_effort.as_deref().unwrap_or("-");

        cols[1].label(format!("session: {sid_full}"));
        cols[1].label(format!("cwd: {cwd_full}"));
        cols[1].label(format!("model: {model}"));
        cols[1].label(format!("provider: {provider}"));
        cols[1].label(format!("config(last): {last_cfg}"));
        cols[1].label(format!("effort(last): {effort_last}"));

        if let Some(status) = row.last_status {
            cols[1].label(format!("status(last): {status}"));
        }
        if let Some(ms) = row.last_duration_ms {
            cols[1].label(format!("duration(last): {ms} ms"));
        }
        if let Some(u) = row.last_usage.as_ref() {
            cols[1].label(format!("usage(last): {}", usage_line(u)));
        }
        if let Some(u) = row.total_usage.as_ref() {
            cols[1].label(format!("usage(total): {}", usage_line(u)));
        }

        cols[1].separator();

        let override_cfg = row.override_config_name.as_deref().unwrap_or("-");
        let override_eff = row.override_effort.as_deref().unwrap_or("-");
        let global_cfg = global_override.as_deref().unwrap_or("-");
        cols[1].label(format!(
            "{}: effort={override_eff}, cfg={override_cfg}, global={global_cfg}",
            pick(ctx.lang, "覆盖", "Overrides")
        ));

        let Some(sid) = row.session_id.clone() else {
            cols[1].label(pick(
                ctx.lang,
                "该条目没有 session_id，暂不支持编辑覆盖。",
                "This entry has no session_id; overrides editing is disabled.",
            ));
            return;
        };

        let cfg_options = config_options_from_gui_configs(&snapshot.configs);

        cols[1].add_space(6.0);
        cols[1].label(pick(ctx.lang, "会话覆盖设置", "Session overrides"));

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "固定配置", "Pinned config"));

            let mut selected_name = ctx.view.sessions.editor.config_override.clone();
            egui::ComboBox::from_id_salt(("session_cfg_override", sid.as_str()))
                .selected_text(match selected_name.as_deref() {
                    Some(v) => v.to_string(),
                    None => pick(ctx.lang, "<自动>", "<auto>").to_string(),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_name,
                        None,
                        pick(ctx.lang, "<自动>", "<auto>"),
                    );
                    for (name, label) in cfg_options.iter() {
                        ui.selectable_value(&mut selected_name, Some(name.clone()), label);
                    }
                });
            if selected_name != ctx.view.sessions.editor.config_override {
                ctx.view.sessions.editor.config_override = selected_name;
            }

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
                let desired = ctx.view.sessions.editor.config_override.clone();
                if !snapshot.supports_v1 {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "附着到的代理不支持会话固定配置（需要 API v1）。",
                            "Attached proxy does not support pinned session config (need API v1).",
                        )
                        .to_string(),
                    );
                } else {
                    match ctx
                        .proxy
                        .apply_session_config_override(ctx.rt, sid, desired)
                    {
                        Ok(()) => {
                            force_refresh = true;
                            *ctx.last_info =
                                Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply override failed: {e}"));
                        }
                    }
                }
            }
        });

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "推理强度", "Reasoning effort"));

            let mut choice = match ctx.view.sessions.editor.effort_override.as_deref() {
                None => "auto",
                Some("low") => "low",
                Some("medium") => "medium",
                Some("high") => "high",
                Some("xhigh") => "xhigh",
                Some(_) => "custom",
            };

            egui::ComboBox::from_id_salt(("session_effort_choice", sid.as_str()))
                .selected_text(choice)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, "auto", "auto");
                    ui.selectable_value(&mut choice, "low", "low");
                    ui.selectable_value(&mut choice, "medium", "medium");
                    ui.selectable_value(&mut choice, "high", "high");
                    ui.selectable_value(&mut choice, "xhigh", "xhigh");
                    ui.selectable_value(&mut choice, "custom", "custom");
                });

            if choice == "auto" {
                ctx.view.sessions.editor.effort_override = None;
            } else if choice != "custom" {
                ctx.view.sessions.editor.effort_override = Some(choice.to_string());
                ctx.view.sessions.editor.custom_effort = choice.to_string();
            } else if ctx.view.sessions.editor.effort_override.is_none() {
                ctx.view.sessions.editor.effort_override =
                    Some(ctx.view.sessions.editor.custom_effort.clone());
            }

            ui.add(
                egui::TextEdit::singleline(&mut ctx.view.sessions.editor.custom_effort)
                    .desired_width(90.0)
                    .hint_text(pick(ctx.lang, "自定义", "custom")),
            );

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
                let desired = match choice {
                    "auto" => None,
                    "custom" => {
                        let v = ctx.view.sessions.editor.custom_effort.trim().to_string();
                        if v.is_empty() { None } else { Some(v) }
                    }
                    v => Some(v.to_string()),
                };
                match ctx
                    .proxy
                    .apply_session_effort_override(ctx.rt, sid, desired)
                {
                    Ok(()) => {
                        force_refresh = true;
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply override failed: {e}"));
                    }
                }
            }
        });
    });

    if force_refresh {
        ctx.proxy
            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
    }
}

fn render_requests(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "请求", "Requests"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看请求。",
            "No proxy is running or attached. Start or attach on Overview to view requests.",
        ));
        return;
    };

    let last_error = snapshot.last_error.clone();
    let recent = snapshot.recent.clone();

    if let Some(err) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    let selected_sid = ctx.view.sessions.selected_session_id.as_deref();

    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.requests.scope_session,
            pick(ctx.lang, "跟随所选会话", "Scope to selected session"),
        );
        ui.checkbox(
            &mut ctx.view.requests.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
        }
    });

    ui.add_space(6.0);

    let filtered = recent
        .iter()
        .filter(|r| {
            if ctx.view.requests.errors_only && r.status_code < 400 {
                return false;
            }
            if ctx.view.requests.scope_session {
                match (selected_sid, r.session_id.as_deref()) {
                    (Some(sid), Some(rid)) => sid == rid,
                    (Some(_), None) => false,
                    (None, _) => true,
                }
            } else {
                true
            }
        })
        .take(600)
        .collect::<Vec<_>>();

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
        cols[0].heading(pick(ctx.lang, "列表", "List"));
        cols[0].add_space(4.0);

        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                let now = now_ms();
                for (pos, r) in filtered.iter().enumerate() {
                    let selected = pos == ctx.view.requests.selected_idx;
                    let age = format_age(now, Some(r.ended_at_ms));
                    let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
                    let model = r.model.as_deref().unwrap_or("-");
                    let cfg = r.config_name.as_deref().unwrap_or("-");
                    let pid = r.provider_id.as_deref().unwrap_or("-");
                    let path = shorten_middle(&r.path, 60);
                    let label = format!(
                        "{age}  st={}  {}ms  att={}  {}  {}  {}  {}",
                        r.status_code,
                        r.duration_ms,
                        attempts,
                        shorten(model, 18),
                        shorten(cfg, 14),
                        shorten(pid, 10),
                        path
                    );
                    if ui.selectable_label(selected, label).clicked() {
                        ctx.view.requests.selected_idx = pos;
                    }
                }
            });

        cols[1].heading(pick(ctx.lang, "详情", "Details"));
        cols[1].add_space(4.0);

        let Some(r) = filtered.get(ctx.view.requests.selected_idx).copied() else {
            cols[1].label(pick(
                ctx.lang,
                "无请求数据。",
                "No requests match current filters.",
            ));
            return;
        };

        cols[1].label(format!("id: {}", r.id));
        cols[1].label(format!("service: {}", r.service));
        cols[1].label(format!("method: {}", r.method));
        cols[1].label(format!("path: {}", r.path));
        cols[1].label(format!("status: {}", r.status_code));
        cols[1].label(format!("duration: {} ms", r.duration_ms));
        if let Some(ttfb) = r.ttfb_ms.filter(|v| *v > 0) {
            cols[1].label(format!("ttfb: {ttfb} ms"));
        }

        if let Some(sid) = r.session_id.as_deref() {
            cols[1].label(format!("session: {sid}"));
        }
        if let Some(cwd) = r.cwd.as_deref() {
            cols[1].label(format!("cwd: {cwd}"));
        }

        cols[1].label(format!("model: {}", r.model.as_deref().unwrap_or("-")));
        cols[1].label(format!(
            "effort: {}",
            r.reasoning_effort.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "config: {}",
            r.config_name.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "provider: {}",
            r.provider_id.as_deref().unwrap_or("-")
        ));
        if let Some(u) = r.upstream_base_url.as_deref() {
            cols[1].label(format!("upstream: {}", shorten_middle(u, 80)));
        }

        if let Some(u) = r.usage.as_ref().filter(|u| u.total_tokens > 0) {
            cols[1].label(format!("usage: {}", usage_line(u)));

            let ttfb_ms = r.ttfb_ms.unwrap_or(0);
            let gen_ms = if ttfb_ms > 0 && ttfb_ms < r.duration_ms {
                r.duration_ms.saturating_sub(ttfb_ms)
            } else {
                r.duration_ms
            };
            if gen_ms > 0 && u.output_tokens > 0 {
                let out_tok_s = (u.output_tokens as f64) / (gen_ms as f64 / 1000.0);
                if out_tok_s.is_finite() && out_tok_s > 0.0 {
                    cols[1].label(format!("out_tok/s: {:.1}", out_tok_s));
                }
            }
        }

        cols[1].separator();
        cols[1].label(pick(ctx.lang, "重试 / 路由链", "Retry / route chain"));
        if let Some(retry) = r.retry.as_ref() {
            cols[1].label(format!("attempts: {}", retry.attempts));
            let max = 12usize;
            for (idx, entry) in retry.upstream_chain.iter().take(max).enumerate() {
                cols[1].label(format!("{:>2}. {}", idx + 1, shorten_middle(entry, 120)));
            }
            if retry.upstream_chain.len() > max {
                cols[1].label(format!("… +{} more", retry.upstream_chain.len() - max));
            }
        } else {
            cols[1].label(pick(ctx.lang, "(无重试)", "(no retries)"));
        }
    });
}

fn render_history(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "历史会话", "History"));
    ui.label(pick(
        ctx.lang,
        "读取 Codex 的本地 sessions（~/.codex/sessions）。",
        "Reads local Codex sessions (~/.codex/sessions).",
    ));

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(240.0)
                .hint_text(pick(
                    ctx.lang,
                    "输入关键词（匹配首条用户消息）",
                    "keyword (first user message)",
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let query = ctx.view.history.query.trim().to_string();
            let fut = async move {
                if query.is_empty() {
                    crate::sessions::find_codex_sessions_for_current_dir(200).await
                } else {
                    crate::sessions::search_codex_sessions_for_current_dir(&query, 200).await
                }
            };
            match ctx.rt.block_on(fut) {
                Ok(list) => {
                    ctx.view.history.sessions = list;
                    ctx.view.history.loaded_at_ms = Some(now_ms());
                    ctx.view.history.last_error = None;
                    if ctx.view.history.sessions.is_empty() {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id = None;
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = None;
                    } else if ctx
                        .view
                        .history
                        .selected_id
                        .as_deref()
                        .is_none_or(|id| !ctx.view.history.sessions.iter().any(|s| s.id == id))
                    {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id =
                            Some(ctx.view.history.sessions[0].id.clone());
                        ctx.view.history.loaded_for = None;
                    }
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        ui.checkbox(
            &mut ctx.view.history.auto_load_transcript,
            pick(ctx.lang, "自动加载对话", "Auto load transcript"),
        );
    });

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    if ctx.view.history.sessions.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无会话。点击“刷新”加载。",
            "No sessions loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selection stable.
    let selected_idx = ctx
        .view
        .history
        .selected_id
        .as_deref()
        .and_then(|id| ctx.view.history.sessions.iter().position(|s| s.id == id))
        .unwrap_or(
            ctx.view
                .history
                .selected_idx
                .min(ctx.view.history.sessions.len().saturating_sub(1)),
        );
    ctx.view.history.selected_idx = selected_idx;
    ctx.view.history.selected_id = Some(ctx.view.history.sessions[selected_idx].id.clone());

    // Auto-load transcript when selection/tail changes.
    if ctx.view.history.auto_load_transcript {
        if let Some(id) = ctx.view.history.selected_id.clone() {
            let key = (id.clone(), ctx.view.history.transcript_tail);
            if ctx.view.history.loaded_for.as_ref() != Some(&key) {
                let path = ctx.view.history.sessions[selected_idx].path.clone();
                let tail = ctx.view.history.transcript_tail;
                match ctx
                    .rt
                    .block_on(crate::sessions::read_codex_session_transcript(
                        &path,
                        Some(tail),
                    )) {
                    Ok(msgs) => {
                        ctx.view.history.transcript_messages = msgs;
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = Some(key);
                    }
                    Err(e) => {
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = Some(e.to_string());
                        ctx.view.history.loaded_for = None;
                    }
                }
            }
        }
    }

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "会话列表", "Sessions"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    let selected = idx == ctx.view.history.selected_idx;
                    let id_short = short_sid(&s.id, 16);
                    let cwd = s
                        .cwd
                        .as_deref()
                        .map(|v| shorten(basename(v), 22))
                        .unwrap_or_else(|| "-".to_string());
                    let rounds = s.rounds;
                    let last = s
                        .last_response_at
                        .as_deref()
                        .or(s.updated_at.as_deref())
                        .unwrap_or("-");
                    let first = s.first_user_message.as_deref().unwrap_or("-");
                    let label = format!(
                        "{id_short}  r={rounds}  {cwd}  {last}  {}",
                        shorten(first, 40)
                    );
                    if ui.selectable_label(selected, label).clicked() {
                        ctx.view.history.selected_idx = idx;
                        ctx.view.history.selected_id = Some(s.id.clone());
                        ctx.view.history.loaded_for = None;
                    }
                }
            });

        cols[1].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[1].add_space(4.0);

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "尾部条数", "Tail"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                    .range(10..=500)
                    .speed(1),
            );

            if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
                ctx.view.history.loaded_for = None;
                // Next frame will load (auto_load_transcript=true) or user can click again.
                if !ctx.view.history.auto_load_transcript {
                    ctx.view.history.auto_load_transcript = true;
                }
            }
        });

        if let Some(err) = ctx.view.history.transcript_error.as_deref() {
            cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        egui::ScrollArea::vertical()
            .max_height(480.0)
            .show(&mut cols[1], |ui| {
                if ctx.view.history.transcript_messages.is_empty() {
                    ui.label(pick(ctx.lang, "（无内容）", "(empty)"));
                    return;
                }
                for msg in ctx.view.history.transcript_messages.iter_mut() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    ui.label(format!("[{ts}] {role}:"));
                    ui.add(
                        egui::TextEdit::multiline(&mut msg.text)
                            .desired_rows(3)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                    ui.add_space(6.0);
                }
            });
    });
}

fn render_placeholder(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, name: &str) {
    ui.heading(pick(ctx.lang, "开发中", "In progress"));
    ui.label(format!("{}: {}", pick(ctx.lang, "页面", "Page"), name));
    ui.label(pick(
        ctx.lang,
        "后续阶段会把 TUI 的 Sessions/Requests/History 等能力逐步迁移到这里。",
        "Next milestones will port TUI Sessions/Requests/History features here.",
    ));
}

fn render_settings(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "设置", "Settings"));

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "语言", "Language"));
        let mut lang = ctx.gui_cfg.language_enum();
        egui::ComboBox::from_id_salt("gui_lang")
            .selected_text(match lang {
                Language::Zh => "中文",
                Language::En => "English",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut lang, Language::Zh, "中文");
                ui.selectable_value(&mut lang, Language::En, "English");
            });
        if lang != ctx.gui_cfg.language_enum() {
            ctx.gui_cfg.set_language_enum(lang);
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(lang, "已保存语言设置", "Language saved").to_string());
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "刷新间隔(ms)", "Refresh (ms)"));
        let mut refresh_ms = ctx.gui_cfg.ui.refresh_ms;
        ui.add(egui::DragValue::new(&mut refresh_ms).range(100..=5000));
        if refresh_ms != ctx.gui_cfg.ui.refresh_ms {
            ctx.gui_cfg.ui.refresh_ms = refresh_ms;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.separator();

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "端口占用时", "On port in use"));
        let mut action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
        egui::ComboBox::from_id_salt("attach_port_in_use_action")
            .selected_text(match action {
                PortInUseAction::Ask => pick(ctx.lang, "每次询问", "Ask"),
                PortInUseAction::Attach => pick(ctx.lang, "默认附着", "Attach"),
                PortInUseAction::StartNewPort => pick(ctx.lang, "自动换端口", "Start new port"),
                PortInUseAction::Exit => pick(ctx.lang, "退出", "Exit"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Ask,
                    pick(ctx.lang, "每次询问", "Ask"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Attach,
                    pick(ctx.lang, "默认附着", "Attach"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::StartNewPort,
                    pick(ctx.lang, "自动换端口", "Start new port"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Exit,
                    pick(ctx.lang, "退出", "Exit"),
                );
            });
        if action.as_str() != ctx.gui_cfg.attach.on_port_in_use {
            ctx.gui_cfg.attach.on_port_in_use = action.as_str().to_string();
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        let mut remember = ctx.gui_cfg.attach.remember_choice;
        ui.checkbox(
            &mut remember,
            pick(
                ctx.lang,
                "记住选择（不再弹窗）",
                "Remember choice (no prompt)",
            ),
        );
        if remember != ctx.gui_cfg.attach.remember_choice {
            ctx.gui_cfg.attach.remember_choice = remember;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "关闭窗口行为", "Close behavior"));

        let mut behavior = ctx.gui_cfg.window.close_behavior.clone();
        egui::ComboBox::from_id_salt("window_close_behavior")
            .selected_text(behavior.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut behavior,
                    "minimize_to_tray".to_string(),
                    "minimize_to_tray",
                );
                ui.selectable_value(&mut behavior, "exit".to_string(), "exit");
            });
        if behavior != ctx.gui_cfg.window.close_behavior {
            ctx.gui_cfg.window.close_behavior = behavior;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        let mut enabled = ctx.gui_cfg.tray.enabled;
        ui.checkbox(&mut enabled, pick(ctx.lang, "启用托盘", "Enable tray"));
        if enabled != ctx.gui_cfg.tray.enabled {
            ctx.gui_cfg.tray.enabled = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info =
                    Some(pick(ctx.lang, "已保存托盘设置", "Tray setting saved").to_string());
            }
        }
        ui.label(pick(
            ctx.lang,
            "(托盘菜单：Show/Hide、Start/Stop、Quit)",
            "(Tray menu: Show/Hide, Start/Stop, Quit)",
        ));
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "开机启动", "Autostart"));

        let reg_enabled = autostart::is_enabled().unwrap_or(false);
        let mut desired = ctx.gui_cfg.autostart.enabled;
        ui.checkbox(&mut desired, pick(ctx.lang, "启用", "Enabled"));

        if desired != ctx.gui_cfg.autostart.enabled {
            match autostart::set_enabled(desired) {
                Ok(()) => {
                    ctx.gui_cfg.autostart.enabled = desired;
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    } else {
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已更新开机启动", "Autostart updated").to_string());
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("set autostart failed: {e}"));
                }
            }
        }

        if ctx.gui_cfg.autostart.enabled != reg_enabled {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(ctx.lang, "（未应用到系统）", "(not applied)"),
            );
        }

        ui.label(pick(ctx.lang, "（Windows）", "(Windows)"));
    });
}

fn render_config(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "配置", "Config"));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前文件", "Current file"),
        ctx.proxy_config_path.display()
    ));

    ui.separator();

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "视图", "View"));
        egui::ComboBox::from_id_salt("config_view_mode")
            .selected_text(match ctx.view.config.mode {
                ConfigMode::Form => pick(ctx.lang, "表单", "Form"),
                ConfigMode::Raw => pick(ctx.lang, "原始", "Raw"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.config.mode,
                    ConfigMode::Form,
                    pick(ctx.lang, "表单", "Form"),
                );
                ui.selectable_value(
                    &mut ctx.view.config.mode,
                    ConfigMode::Raw,
                    pick(ctx.lang, "原始", "Raw"),
                );
            });
    });

    ui.add_space(6.0);
    match ctx.view.config.mode {
        ConfigMode::Form => render_config_form(ui, ctx),
        ConfigMode::Raw => render_config_raw(ui, ctx),
    }
}

fn render_config_form(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(
        ctx.lang,
        "表单视图：优先做常用项（active / enabled / level）。复杂字段仍建议用“原始”视图。",
        "Form view: focuses on common fields (active / enabled / level). Use Raw view for advanced edits.",
    ));

    let mut needs_load = ctx.view.config.working.is_none();
    if let Some(err) = ctx.view.config.load_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        needs_load = true;
    }

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "从磁盘加载", "Load from disk"))
            .clicked()
        {
            needs_load = true;
        }

        if ui
            .button(pick(ctx.lang, "重载代理运行态", "Reload proxy runtime"))
            .clicked()
        {
            if let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt) {
                *ctx.last_error = Some(format!("reload runtime failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
            }
        }
    });

    if needs_load {
        match std::fs::read_to_string(ctx.proxy_config_path) {
            Ok(t) => match parse_proxy_config(&t) {
                Ok(cfg) => {
                    ctx.view.config.working = Some(cfg);
                    ctx.view.config.load_error = None;
                }
                Err(e) => {
                    ctx.view.config.working = None;
                    ctx.view.config.load_error = Some(format!("parse failed: {e}"));
                }
            },
            Err(e) => {
                ctx.view.config.working = None;
                ctx.view.config.load_error = Some(format!("read config failed: {e}"));
            }
        }
    }

    if ctx.view.config.working.is_none() {
        ui.add_space(6.0);
        ui.label(pick(
            ctx.lang,
            "未加载配置。你可以切换到“原始”视图，或点击“从磁盘加载”。",
            "Config not loaded. Switch to Raw view, or click Load from disk.",
        ));
        return;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        let mut svc = ctx.view.config.service;
        egui::ComboBox::from_id_salt("config_form_service")
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

    let (active_name, names) = {
        let cfg = ctx.view.config.working.as_ref().expect("checked above");
        let mgr = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &cfg.claude,
            crate::config::ServiceKind::Codex => &cfg.codex,
        };
        let mut v = mgr.configs.keys().cloned().collect::<Vec<_>>();
        v.sort_by(|a, b| {
            let la = mgr.configs.get(a).map(|c| c.level).unwrap_or(1);
            let lb = mgr.configs.get(b).map(|c| c.level).unwrap_or(1);
            la.cmp(&lb).then_with(|| a.cmp(b))
        });
        (mgr.active.clone(), v)
    };

    if names.is_empty() {
        ui.add_space(6.0);
        ui.label(pick(
            ctx.lang,
            "该服务下没有任何 config（configs 为空）。请先在“原始”视图或文件中添加。",
            "No configs found for this service. Add one via Raw view or by editing the file.",
        ));
        return;
    }

    if ctx
        .view
        .config
        .selected_name
        .as_ref()
        .is_none_or(|n| !names.iter().any(|x| x == n))
    {
        ctx.view.config.selected_name = names.first().cloned();
    }

    let selected_name = ctx.view.config.selected_name.clone();
    let mut action_set_active: Option<String> = None;
    let mut action_save_apply = false;

    {
        let cfg = ctx.view.config.working.as_mut().expect("checked above");
        ui.columns(2, |cols| {
            cols[0].heading(pick(ctx.lang, "配置列表", "Configs"));
            cols[0].add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(520.0)
                .show(&mut cols[0], |ui| {
                    for name in names.iter() {
                        let is_active = active_name.as_deref() == Some(name.as_str());
                        let is_selected = selected_name.as_deref() == Some(name.as_str());

                        let svc = match ctx.view.config.service {
                            crate::config::ServiceKind::Claude => cfg.claude.configs.get(name),
                            crate::config::ServiceKind::Codex => cfg.codex.configs.get(name),
                        };

                        let (enabled, level, alias, upstreams) = svc
                            .map(|s| {
                                (
                                    s.enabled,
                                    s.level.clamp(1, 10),
                                    s.alias.as_deref().unwrap_or(""),
                                    s.upstreams.len(),
                                )
                            })
                            .unwrap_or((false, 1, "", 0));

                        let mut label = format!("L{level} {name}");
                        if !alias.trim().is_empty() {
                            label.push_str(&format!(" ({alias})"));
                        }
                        label.push_str(&format!("  up={upstreams}"));
                        if !enabled {
                            label.push_str("  [off]");
                        }
                        if is_active {
                            label = format!("★ {label}");
                        }

                        if ui.selectable_label(is_selected, label).clicked() {
                            ctx.view.config.selected_name = Some(name.clone());
                        }
                    }
                });

            cols[1].heading(pick(ctx.lang, "详情", "Details"));
            cols[1].add_space(4.0);

            let Some(name) = ctx.view.config.selected_name.clone() else {
                cols[1].label(pick(ctx.lang, "未选择配置。", "No config selected."));
                return;
            };

            let mgr = match ctx.view.config.service {
                crate::config::ServiceKind::Claude => &mut cfg.claude,
                crate::config::ServiceKind::Codex => &mut cfg.codex,
            };
            let Some(svc) = mgr.configs.get_mut(&name) else {
                cols[1].label(pick(
                    ctx.lang,
                    "配置不存在（可能已被删除）。",
                    "Config missing.",
                ));
                return;
            };

            cols[1].label(format!("name: {}", svc.name));
            cols[1].label(format!("alias: {}", svc.alias.as_deref().unwrap_or("-")));
            cols[1].label(format!("upstreams: {}", svc.upstreams.len()));
            cols[1].add_space(6.0);

            cols[1].horizontal(|ui| {
                ui.checkbox(&mut svc.enabled, pick(ctx.lang, "启用", "Enabled"));
                ui.label(pick(ctx.lang, "等级", "Level"));
                ui.add(egui::DragValue::new(&mut svc.level).range(1..=10));
            });

            cols[1].add_space(6.0);
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "设为 active", "Set active"))
                    .clicked()
                {
                    action_set_active = Some(name.clone());
                }

                if ui
                    .button(pick(ctx.lang, "保存并应用", "Save & apply"))
                    .clicked()
                {
                    action_save_apply = true;
                }
            });
        });
    }

    if let Some(name) = action_set_active {
        let cfg = ctx.view.config.working.as_mut().expect("checked above");
        let mgr = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        mgr.active = Some(name);
        *ctx.last_info = Some(pick(ctx.lang, "已设置 active", "Active set").to_string());
    }

    if action_save_apply {
        let save_res = {
            let cfg = ctx.view.config.working.as_ref().expect("checked above");
            ctx.rt.block_on(crate::config::save_config(cfg))
        };
        match save_res {
            Ok(()) => {
                let new_path = crate::config::config_file_path();
                if let Ok(t) = std::fs::read_to_string(&new_path) {
                    *ctx.proxy_config_text = t;
                }
                if let Ok(t) = std::fs::read_to_string(&new_path)
                    && let Ok(parsed) = parse_proxy_config(&t)
                {
                    ctx.view.config.working = Some(parsed);
                }

                if matches!(
                    ctx.proxy.kind(),
                    super::proxy_control::ProxyModeKind::Running
                        | super::proxy_control::ProxyModeKind::Attached
                ) {
                    if let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt) {
                        *ctx.last_error = Some(format!("reload runtime failed: {e}"));
                    }
                }

                *ctx.last_info = Some(pick(ctx.lang, "已保存", "Saved").to_string());
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!("save failed: {e}"));
            }
        }
    }
}

fn render_config_raw(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "原始编辑", "Raw editor"));

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "从磁盘重载", "Reload from disk"))
            .clicked()
        {
            match std::fs::read_to_string(ctx.proxy_config_path) {
                Ok(t) => {
                    *ctx.proxy_config_text = t;
                    *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("read config failed: {e}"));
                }
            }
        }

        if ui.button(pick(ctx.lang, "校验", "Validate")).clicked() {
            match parse_proxy_config(ctx.proxy_config_text) {
                Ok(_) => {
                    *ctx.last_info = Some(pick(ctx.lang, "校验通过", "Valid").to_string());
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("parse failed: {e}"));
                }
            }
        }

        if ui
            .button(pick(ctx.lang, "保存并应用", "Save & apply"))
            .clicked()
        {
            match parse_proxy_config(ctx.proxy_config_text) {
                Ok(cfg) => {
                    let save = ctx.rt.block_on(crate::config::save_config(&cfg));
                    match save {
                        Ok(()) => {
                            let new_path = crate::config::config_file_path();
                            match std::fs::read_to_string(&new_path) {
                                Ok(t) => {
                                    *ctx.proxy_config_text = t;
                                    *ctx.last_info =
                                        Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                    *ctx.last_error = None;
                                }
                                Err(e) => {
                                    *ctx.last_info =
                                        Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                    *ctx.last_error = Some(format!("re-read failed: {e}"));
                                }
                            }

                            if matches!(
                                ctx.proxy.kind(),
                                super::proxy_control::ProxyModeKind::Running
                                    | super::proxy_control::ProxyModeKind::Attached
                            ) {
                                if let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt) {
                                    *ctx.last_error = Some(format!("reload runtime failed: {e}"));
                                }
                            }
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("save failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("parse failed: {e}"));
                }
            }
        }
    });

    ui.separator();
    let editor = egui::TextEdit::multiline(ctx.proxy_config_text)
        .font(egui::TextStyle::Monospace)
        .code_editor()
        .desired_rows(28)
        .lock_focus(true);
    ui.add(editor);
}

fn parse_proxy_config(text: &str) -> anyhow::Result<crate::config::ProxyConfig> {
    // Be forgiving: try TOML first, then JSON.
    if let Ok(v) = toml::from_str::<crate::config::ProxyConfig>(text) {
        return Ok(v);
    }
    let v = serde_json::from_str::<crate::config::ProxyConfig>(text)?;
    Ok(v)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRow {
    session_id: Option<String>,
    cwd: Option<String>,
    active_count: u64,
    active_started_at_ms_min: Option<u64>,
    last_status: Option<u16>,
    last_duration_ms: Option<u64>,
    last_ended_at_ms: Option<u64>,
    last_model: Option<String>,
    last_reasoning_effort: Option<String>,
    last_provider_id: Option<String>,
    last_config_name: Option<String>,
    last_usage: Option<UsageMetrics>,
    total_usage: Option<UsageMetrics>,
    turns_total: Option<u64>,
    turns_with_usage: Option<u64>,
    override_effort: Option<String>,
    override_config_name: Option<String>,
}

fn build_session_rows(
    active: Vec<ActiveRequest>,
    recent: &[FinishedRequest],
    overrides: &HashMap<String, String>,
    config_overrides: &HashMap<String, String>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionRow> {
    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionRow> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: req.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: Some(req.started_at_ms),
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: req.model.clone(),
            last_reasoning_effort: req.reasoning_effort.clone(),
            last_provider_id: req.provider_id.clone(),
            last_config_name: req.config_name.clone(),
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });

        entry.active_count = entry.active_count.saturating_add(1);
        entry.active_started_at_ms_min = Some(
            entry
                .active_started_at_ms_min
                .unwrap_or(req.started_at_ms)
                .min(req.started_at_ms),
        );
        if entry.cwd.is_none() {
            entry.cwd = req.cwd;
        }
        if let Some(effort) = req.reasoning_effort {
            entry.last_reasoning_effort = Some(effort);
        }
        if entry.last_model.is_none() {
            entry.last_model = req.model;
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = req.provider_id;
        }
        if entry.last_config_name.is_none() {
            entry.last_config_name = req.config_name;
        }
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: r.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: r.model.clone(),
            last_reasoning_effort: r.reasoning_effort.clone(),
            last_provider_id: r.provider_id.clone(),
            last_config_name: r.config_name.clone(),
            last_usage: r.usage.clone(),
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });

        let should_update = entry
            .last_ended_at_ms
            .map_or(true, |prev| r.ended_at_ms >= prev);
        if should_update {
            entry.last_status = Some(r.status_code);
            entry.last_duration_ms = Some(r.duration_ms);
            entry.last_ended_at_ms = Some(r.ended_at_ms);
            entry.last_model = r.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = r
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_provider_id = r.provider_id.clone().or(entry.last_provider_id.clone());
            entry.last_config_name = r.config_name.clone().or(entry.last_config_name.clone());
            entry.last_usage = r.usage.clone().or(entry.last_usage.clone());
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
    }

    for (sid, st) in stats.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_provider_id: None,
            last_config_name: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });

        if entry.turns_total.is_none() {
            entry.turns_total = Some(st.turns_total);
        }
        if entry.last_status.is_none() {
            entry.last_status = st.last_status;
        }
        if entry.last_duration_ms.is_none() {
            entry.last_duration_ms = st.last_duration_ms;
        }
        if entry.last_ended_at_ms.is_none() {
            entry.last_ended_at_ms = st.last_ended_at_ms;
        }
        if entry.last_model.is_none() {
            entry.last_model = st.last_model.clone();
        }
        if entry.last_reasoning_effort.is_none() {
            entry.last_reasoning_effort = st.last_reasoning_effort.clone();
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = st.last_provider_id.clone();
        }
        if entry.last_config_name.is_none() {
            entry.last_config_name = st.last_config_name.clone();
        }
        if entry.last_usage.is_none() {
            entry.last_usage = st.last_usage.clone();
        }
        if entry.total_usage.is_none() {
            entry.total_usage = Some(st.total_usage.clone());
        }
        if entry.turns_with_usage.is_none() {
            entry.turns_with_usage = Some(st.turns_with_usage);
        }
    }

    for (sid, eff) in overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_provider_id: None,
            last_config_name: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });
        entry.override_effort = Some(eff.clone());
    }

    for (sid, cfg_name) in config_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_provider_id: None,
            last_config_name: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });
        entry.override_config_name = Some(cfg_name.clone());
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_age(now_ms: u64, ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return "-".to_string();
    };
    if now_ms <= ts {
        return "0s".to_string();
    }
    let mut secs = (now_ms - ts) / 1000;
    let days = secs / 86400;
    secs %= 86400;
    let hours = secs / 3600;
    secs %= 3600;
    let mins = secs / 60;
    secs %= 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(|c| c == '/' || c == '\\');
    trimmed
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(trimmed)
}

fn shorten(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn short_sid(s: &str, max_chars: usize) -> String {
    shorten(s, max_chars)
}

fn shorten_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let left = max_chars / 2;
    let right = max_chars.saturating_sub(left).saturating_sub(1);
    let mut out = String::new();
    for ch in chars.iter().take(left) {
        out.push(*ch);
    }
    out.push('…');
    for ch in chars.iter().skip(chars.len().saturating_sub(right)) {
        out.push(*ch);
    }
    out
}

fn tokens_short(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1_000_000.0 {
        format!("{:.1}m", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{:.0}", n)
    }
}

fn usage_line(usage: &UsageMetrics) -> String {
    format!(
        "tok in/out/rsn/ttl: {}/{}/{}/{}",
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_tokens),
        tokens_short(usage.total_tokens)
    )
}

fn config_options_from_gui_configs(configs: &[GuiConfigOption]) -> Vec<(String, String)> {
    let mut out = configs
        .iter()
        .map(|c| {
            let label = match c.alias.as_deref() {
                Some(a) if !a.trim().is_empty() => format!("{} ({a})", c.name),
                _ => c.name.clone(),
            };
            (c.name.clone(), label, c.level.clamp(1, 10))
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    out.into_iter().map(|(n, l, _)| (n, l)).collect()
}
