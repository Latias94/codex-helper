use eframe::egui;

use std::collections::{HashMap, HashSet};

use super::autostart;
use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::proxy_control::{PortInUseAction, ProxyModeKind};
use super::util::{open_in_file_manager, spawn_windows_terminal_wt_new_tab};
use crate::dashboard_core::ConfigOption;
use crate::sessions::{SessionDayDir, SessionIndexItem, SessionSummary, SessionTranscriptMessage};
use crate::state::{ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, SessionStats};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Overview,
    Config,
    Sessions,
    Requests,
    Stats,
    History,
    Settings,
}

#[derive(Debug, Default)]
pub struct ViewState {
    pub requested_page: Option<Page>,
    pub setup: SetupViewState,
    pub sessions: SessionsViewState,
    pub requests: RequestsViewState,
    pub config: ConfigViewState,
    pub history: HistoryViewState,
}

#[derive(Debug)]
pub struct SetupViewState {
    pub import_codex_on_init: bool,
}

impl Default for SetupViewState {
    fn default() -> Self {
        Self {
            import_codex_on_init: true,
        }
    }
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
    import_codex: ImportCodexModalState,
}

impl Default for ConfigViewState {
    fn default() -> Self {
        Self {
            mode: ConfigMode::Form,
            service: crate::config::ServiceKind::Codex,
            selected_name: None,
            working: None,
            load_error: None,
            import_codex: ImportCodexModalState::default(),
        }
    }
}

#[derive(Debug)]
struct ImportCodexModalState {
    open: bool,
    add_missing: bool,
    set_active: bool,
    force: bool,
    preview: Option<crate::config::SyncCodexAuthFromCodexReport>,
    last_error: Option<String>,
}

impl Default for ImportCodexModalState {
    fn default() -> Self {
        Self {
            open: false,
            add_missing: true,
            set_active: true,
            force: false,
            preview: None,
            last_error: None,
        }
    }
}

#[derive(Debug)]
pub struct HistoryViewState {
    pub scope: HistoryScope,
    pub query: String,
    pub sessions: Vec<SessionSummary>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub selected_idx: usize,
    pub selected_id: Option<String>,
    pub recent_since_hours: u32,
    pub recent_limit: usize,
    pub infer_git_root: bool,
    pub resume_cmd: String,
    pub shell: String,
    pub keep_open: bool,
    pub wt_window: i32,
    pub all_days_limit: usize,
    pub all_dates: Vec<SessionDayDir>,
    pub all_selected_date: Option<String>,
    pub all_day_limit: usize,
    pub all_day_sessions: Vec<SessionIndexItem>,
    loaded_day_for: Option<String>,
    pub hide_tool_calls: bool,
    pub transcript_view: TranscriptViewMode,
    pub transcript_selected_msg_idx: usize,
    transcript_plain_key: Option<(String, Option<usize>, bool)>,
    transcript_plain_text: String,
    transcript_load_seq: u64,
    transcript_load: Option<TranscriptLoad>,
    pub auto_load_transcript: bool,
    pub transcript_full: bool,
    pub transcript_tail: usize,
    pub transcript_raw_messages: Vec<SessionTranscriptMessage>,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
    pub transcript_error: Option<String>,
    loaded_for: Option<(String, Option<usize>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    CurrentProject,
    GlobalRecent,
    AllByDate,
}

impl Default for HistoryViewState {
    fn default() -> Self {
        Self {
            scope: HistoryScope::CurrentProject,
            query: String::new(),
            sessions: Vec::new(),
            last_error: None,
            loaded_at_ms: None,
            selected_idx: 0,
            selected_id: None,
            recent_since_hours: 12,
            recent_limit: 50,
            infer_git_root: true,
            resume_cmd: "codex resume {id}".to_string(),
            shell: "pwsh".to_string(),
            keep_open: true,
            wt_window: -1,
            all_days_limit: 120,
            all_dates: Vec::new(),
            all_selected_date: None,
            all_day_limit: 500,
            all_day_sessions: Vec::new(),
            loaded_day_for: None,
            hide_tool_calls: true,
            transcript_view: TranscriptViewMode::Messages,
            transcript_selected_msg_idx: 0,
            transcript_plain_key: None,
            transcript_plain_text: String::new(),
            transcript_load_seq: 0,
            transcript_load: None,
            auto_load_transcript: true,
            transcript_full: false,
            transcript_tail: 80,
            transcript_raw_messages: Vec::new(),
            transcript_messages: Vec::new(),
            transcript_error: None,
            loaded_for: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptViewMode {
    Messages,
    PlainText,
}

#[derive(Debug)]
struct TranscriptLoad {
    seq: u64,
    key: (String, Option<usize>),
    rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Vec<SessionTranscriptMessage>>)>,
    join: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Default)]
pub struct SessionsViewState {
    pub active_only: bool,
    pub errors_only: bool,
    pub overrides_only: bool,
    pub lock_order: bool,
    pub search: String,
    pub selected_session_id: Option<String>,
    pub selected_idx: usize,
    ordered_session_ids: Vec<Option<String>>,
    last_active_set: HashSet<Option<String>>,
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
            (Page::Setup, pick(lang, "快速设置", "Setup")),
            (Page::Overview, pick(lang, "总览", "Overview")),
            (Page::Config, pick(lang, "配置", "Config")),
            (Page::Sessions, pick(lang, "会话", "Sessions")),
            (Page::Requests, pick(lang, "请求", "Requests")),
            (Page::Stats, pick(lang, "统计", "Stats")),
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
        Page::Setup => render_setup(ui, ctx),
        Page::Overview => render_overview(ui, ctx),
        Page::Config => render_config(ui, ctx),
        Page::Sessions => render_sessions(ui, ctx),
        Page::Requests => render_requests(ui, ctx),
        Page::Stats => render_stats(ui, ctx),
        Page::History => render_history(ui, ctx),
        Page::Settings => render_settings(ui, ctx),
    }
}

fn render_setup(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "快速设置", "Setup"));
    ui.label(pick(
        ctx.lang,
        "目标：让 Codex/Claude 走本地 codex-helper 代理（常驻后台），并完成基础配置。",
        "Goal: route Codex/Claude through the local codex-helper proxy (resident) and complete basic setup.",
    ));
    ui.separator();

    // Step 1: proxy config
    let cfg_path = ctx.proxy_config_path.to_path_buf();
    let cfg_exists = cfg_path.exists() && !ctx.proxy_config_text.trim().is_empty();

    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "1) 生成/导入配置",
            "1) Create/import config",
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "配置文件", "Config file"),
            cfg_path.display()
        ));

        if cfg_exists {
            ui.colored_label(
                egui::Color32::from_rgb(60, 160, 90),
                pick(ctx.lang, "已就绪", "Ready"),
            );
            if ui
                .button(pick(ctx.lang, "打开配置文件", "Open config file"))
                .clicked()
            {
                if let Err(e) = open_in_file_manager(&cfg_path, true) {
                    *ctx.last_error = Some(format!("open config failed: {e}"));
                }
            }
            if ui
                .button(pick(ctx.lang, "前往配置页", "Go to Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "未检测到有效配置（建议先创建）",
                    "Config not found (create one first)",
                ),
            );
            ui.checkbox(
                &mut ctx.view.setup.import_codex_on_init,
                pick(
                    ctx.lang,
                    "自动从 ~/.codex/config.toml + auth.json 导入 Codex upstream",
                    "Auto-import Codex upstreams from ~/.codex/config.toml + auth.json",
                ),
            );

            if ui
                .button(pick(ctx.lang, "创建 config.toml", "Create config.toml"))
                .clicked()
            {
                match ctx.rt.block_on(crate::config::init_config_toml(
                    false,
                    ctx.view.setup.import_codex_on_init,
                )) {
                    Ok(path) => {
                        *ctx.last_info = Some(format!(
                            "{}: {}",
                            pick(ctx.lang, "已写入配置", "Wrote config"),
                            path.display()
                        ));
                        *ctx.proxy_config_text =
                            std::fs::read_to_string(ctx.proxy_config_path).unwrap_or_default();
                    }
                    Err(e) => *ctx.last_error = Some(format!("init config failed: {e}")),
                }
            }
        }
    });

    ui.add_space(10.0);

    // Step 2: start proxy
    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "2) 启动本地代理", "2) Start local proxy"));

        let kind = ctx.proxy.kind();
        let status_text = match kind {
            ProxyModeKind::Running => pick(ctx.lang, "运行中", "Running"),
            ProxyModeKind::Attached => pick(ctx.lang, "已附着", "Attached"),
            ProxyModeKind::Starting => pick(ctx.lang, "启动中", "Starting"),
            ProxyModeKind::Stopped => pick(ctx.lang, "未运行", "Stopped"),
        };
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "状态", "Status"),
            status_text
        ));

        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "服务", "Service"));
            let mut svc = ctx.proxy.desired_service();
            egui::ComboBox::from_id_salt("setup_service")
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

        ui.horizontal(|ui| {
            let can_start = matches!(ctx.proxy.kind(), ProxyModeKind::Stopped);
            if ui
                .add_enabled(
                    can_start,
                    egui::Button::new(pick(ctx.lang, "启动代理", "Start proxy")),
                )
                .clicked()
            {
                let action = super::proxy_control::PortInUseAction::parse(
                    &ctx.gui_cfg.attach.on_port_in_use,
                );
                ctx.proxy.request_start_or_prompt(
                    ctx.rt,
                    action,
                    ctx.gui_cfg.attach.remember_choice,
                );
            }

            let can_stop = matches!(
                ctx.proxy.kind(),
                ProxyModeKind::Running | ProxyModeKind::Attached
            );
            if ui
                .add_enabled(
                    can_stop,
                    egui::Button::new(pick(ctx.lang, "停止代理", "Stop proxy")),
                )
                .clicked()
            {
                if let Err(e) = ctx.proxy.stop(ctx.rt) {
                    *ctx.last_error = Some(format!("stop failed: {e}"));
                } else {
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已停止代理", "Proxy stopped").to_string());
                }
            }
        });
    });

    ui.add_space(10.0);

    // Step 3: switch client
    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "3) 让客户端走本地代理", "3) Point client to local proxy"));

        let svc = ctx.proxy.desired_service();
        let port = ctx
            .proxy
            .snapshot()
            .and_then(|s| s.port)
            .unwrap_or(ctx.proxy.desired_port());

        match svc {
            crate::config::ServiceKind::Claude => {
                let st = crate::codex_integration::claude_switch_status();
                match st {
                    Ok(st) => {
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "Claude settings", "Claude settings"),
                            st.settings_path.display()
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 ANTHROPIC_BASE_URL", "Current ANTHROPIC_BASE_URL"),
                            st.base_url.as_deref().unwrap_or("-")
                        ));
                        if st.enabled {
                            ui.colored_label(
                                egui::Color32::from_rgb(60, 160, 90),
                                pick(ctx.lang, "已启用（本地代理）", "Enabled (local proxy)"),
                            );
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(ctx.lang, "未启用", "Not enabled"),
                            );
                        }

                        ui.horizontal(|ui| {
                            let enable_label = match ctx.lang {
                                Language::Zh => format!("启用（端口 {port}）"),
                                Language::En => format!("Enable (port {port})"),
                            };
                            if ui
                                .add_enabled(
                                    !st.enabled,
                                    egui::Button::new(enable_label),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::claude_switch_on(port) {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已更新 Claude settings 指向本地代理",
                                            "Updated Claude settings to local proxy",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                                }
                            }

                            if ui
                                .add_enabled(
                                    st.has_backup,
                                    egui::Button::new(pick(ctx.lang, "恢复（从备份）", "Restore (from backup)")),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::claude_switch_off() {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已从备份恢复 Claude settings",
                                            "Restored Claude settings from backup",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                                }
                            }
                        });
                    }
                    Err(e) => *ctx.last_error = Some(format!("read claude switch status failed: {e}")),
                }
            }
            _ => {
                let st = crate::codex_integration::codex_switch_status();
                match st {
                    Ok(st) => {
                        ui.label(pick(
                            ctx.lang,
                            "Codex 将通过 ~/.codex/config.toml 的 model_provider 指向本地代理。",
                            "Codex will route through ~/.codex/config.toml (model_provider).",
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 model_provider", "Current model_provider"),
                            st.model_provider.as_deref().unwrap_or("-")
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 base_url", "Current base_url"),
                            st.base_url.as_deref().unwrap_or("-")
                        ));
                        if st.enabled {
                            ui.colored_label(
                                egui::Color32::from_rgb(60, 160, 90),
                                pick(ctx.lang, "已启用（本地代理）", "Enabled (local proxy)"),
                            );
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(ctx.lang, "未启用", "Not enabled"),
                            );
                        }

                        ui.horizontal(|ui| {
                            let enable_label = match ctx.lang {
                                Language::Zh => format!("启用（端口 {port}）"),
                                Language::En => format!("Enable (port {port})"),
                            };
                            if ui
                                .add_enabled(
                                    !st.enabled,
                                    egui::Button::new(enable_label),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::switch_on(port) {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已更新 ~/.codex/config.toml 指向本地代理",
                                            "Updated ~/.codex/config.toml to local proxy",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                                }
                            }

                            if ui
                                .add_enabled(
                                    st.has_backup,
                                    egui::Button::new(pick(ctx.lang, "恢复（从备份）", "Restore (from backup)")),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::switch_off() {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已从备份恢复 ~/.codex/config.toml",
                                            "Restored ~/.codex/config.toml from backup",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                                }
                            }
                        });

                        if !st.has_backup {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(
                                    ctx.lang,
                                    "提示：未检测到备份文件（首次 switch on 时会自动创建备份）。",
                                    "Tip: no backup detected (a backup is created on first switch on).",
                                ),
                            );
                        }
                    }
                    Err(e) => *ctx.last_error = Some(format!("read codex switch status failed: {e}")),
                }
            }
        }
    });

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "我已完成，前往总览", "Done, go to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
    });
}

fn render_overview(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "总览", "Overview"));

    ui.separator();

    let mut action_scan_local_proxies = false;
    let mut action_attach_discovered: Option<u16> = None;

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

            ui.add_space(10.0);
            ui.separator();
            ui.label(pick(
                ctx.lang,
                "手动附着到已运行的代理",
                "Attach to an existing proxy",
            ));
            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "端口", "Port"));
                let mut attach_port = ctx
                    .gui_cfg
                    .attach
                    .last_port
                    .unwrap_or(ctx.gui_cfg.proxy.default_port);
                ui.add(egui::DragValue::new(&mut attach_port).range(1..=65535));
                if Some(attach_port) != ctx.gui_cfg.attach.last_port {
                    ctx.gui_cfg.attach.last_port = Some(attach_port);
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    }
                }

                if ui.button(pick(ctx.lang, "附着", "Attach")).clicked() {
                    ctx.proxy.request_attach(attach_port);
                    ctx.gui_cfg.attach.last_port = Some(attach_port);
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    } else {
                        *ctx.last_info = Some(pick(ctx.lang, "正在附着…", "Attaching...").into());
                    }
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(pick(
                ctx.lang,
                "自动发现本机已运行的代理（端口 3210-3220）",
                "Discover running local proxies (ports 3210-3220)",
            ));
            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "扫描 3210-3220", "Scan 3210-3220"))
                    .clicked()
                {
                    action_scan_local_proxies = true;
                }
                if let Some(t) = ctx.proxy.last_discovery_scan() {
                    ui.label(format!(
                        "{}: {}s",
                        pick(ctx.lang, "上次扫描", "Last scan"),
                        t.elapsed().as_secs()
                    ));
                }
            });

            let discovered = ctx.proxy.discovered_proxies().to_vec();
            if discovered.is_empty() {
                ui.label(pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"));
            } else {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        egui::Grid::new("discovered_proxies_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(pick(ctx.lang, "端口", "Port"));
                                ui.label(pick(ctx.lang, "服务", "Service"));
                                ui.label(pick(ctx.lang, "API", "API"));
                                ui.label(pick(ctx.lang, "状态", "Status"));
                                ui.end_row();

                                for p in discovered {
                                    ui.label(p.port.to_string());
                                    ui.label(
                                        p.service_name
                                            .as_deref()
                                            .unwrap_or_else(|| pick(ctx.lang, "未知", "unknown")),
                                    );
                                    ui.label(match p.api_version {
                                        Some(v) => format!("v{v}"),
                                        None => "-".to_string(),
                                    });
                                    if let Some(err) = p.last_error.as_deref() {
                                        ui.label(err);
                                    } else {
                                        ui.label(pick(ctx.lang, "可用", "OK"));
                                    }

                                    if ui.button(pick(ctx.lang, "附着", "Attach")).clicked() {
                                        action_attach_discovered = Some(p.port);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        }
        ProxyModeKind::Starting => {
            ui.label(pick(ctx.lang, "正在启动…", "Starting..."));
        }
        ProxyModeKind::Running => {
            let mut global_override_ui: Option<(Option<String>, Vec<ConfigOption>)> = None;
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
                if let Some(snapshot) = ctx.proxy.snapshot() {
                    global_override_ui = Some((snapshot.global_override, snapshot.configs));
                }
            }

            if let Some((global, configs)) = global_override_ui {
                ui.add_space(10.0);
                ui.separator();
                ui.label(pick(
                    ctx.lang,
                    "全局覆盖（Pinned）",
                    "Global override (pinned)",
                ));
                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "固定配置", "Pinned config"));

                    let mut selected = global.clone();
                    egui::ComboBox::from_id_salt("global_cfg_override")
                        .selected_text(match selected.as_deref() {
                            Some(v) => v.to_string(),
                            None => pick(ctx.lang, "<自动>", "<auto>").to_string(),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut selected,
                                None,
                                pick(ctx.lang, "<自动>", "<auto>"),
                            );
                            for cfg in configs.iter().filter(|c| c.enabled) {
                                let label = match cfg.alias.as_deref() {
                                    Some(a) if !a.trim().is_empty() => {
                                        format!("{} ({a})", cfg.name)
                                    }
                                    _ => cfg.name.clone(),
                                };
                                ui.selectable_value(&mut selected, Some(cfg.name.clone()), label);
                            }
                        });

                    if selected != global {
                        match ctx.proxy.apply_global_config_override(ctx.rt, selected) {
                            Ok(()) => {
                                *ctx.last_info =
                                    Some(pick(ctx.lang, "已应用全局覆盖", "Applied").to_string());
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("apply global override failed: {e}"));
                            }
                        }
                    }

                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        if let Err(e) = ctx.proxy.apply_global_config_override(ctx.rt, None) {
                            *ctx.last_error = Some(format!("clear failed: {e}"));
                        } else {
                            *ctx.last_info =
                                Some(pick(ctx.lang, "已清除全局覆盖", "Cleared").to_string());
                        }
                    }
                });
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
            let mut global_override_ui: Option<(Option<String>, Vec<ConfigOption>, bool)> = None;
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

                global_override_ui = Some((
                    att.global_override.clone(),
                    att.configs.clone(),
                    att.api_version == Some(1),
                ));
            }

            if let Some((global, configs, supports_v1)) = global_override_ui {
                if supports_v1 && !configs.is_empty() {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(pick(
                        ctx.lang,
                        "全局覆盖（Pinned）",
                        "Global override (pinned)",
                    ));
                    ui.horizontal(|ui| {
                        ui.label(pick(ctx.lang, "固定配置", "Pinned config"));
                        let mut selected = global.clone();
                        egui::ComboBox::from_id_salt("global_cfg_override_attached")
                            .selected_text(match selected.as_deref() {
                                Some(v) => v.to_string(),
                                None => pick(ctx.lang, "<自动>", "<auto>").to_string(),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut selected,
                                    None,
                                    pick(ctx.lang, "<自动>", "<auto>"),
                                );
                                for cfg in configs.iter().filter(|c| c.enabled) {
                                    let label = match cfg.alias.as_deref() {
                                        Some(a) if !a.trim().is_empty() => {
                                            format!("{} ({a})", cfg.name)
                                        }
                                        _ => cfg.name.clone(),
                                    };
                                    ui.selectable_value(
                                        &mut selected,
                                        Some(cfg.name.clone()),
                                        label,
                                    );
                                }
                            });

                        if selected != global {
                            match ctx.proxy.apply_global_config_override(ctx.rt, selected) {
                                Ok(()) => {
                                    *ctx.last_info = Some(
                                        pick(ctx.lang, "已应用全局覆盖", "Applied").to_string(),
                                    );
                                }
                                Err(e) => {
                                    *ctx.last_error =
                                        Some(format!("apply global override failed: {e}"));
                                }
                            }
                        }

                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            if let Err(e) = ctx.proxy.apply_global_config_override(ctx.rt, None) {
                                *ctx.last_error = Some(format!("clear failed: {e}"));
                            } else {
                                *ctx.last_info =
                                    Some(pick(ctx.lang, "已清除全局覆盖", "Cleared").to_string());
                            }
                        }
                    });
                } else if !supports_v1 {
                    ui.add_space(6.0);
                    ui.label(pick(
                        ctx.lang,
                        "附着代理未启用 API v1：全局覆盖不可用。",
                        "Attached proxy has no API v1: global override disabled.",
                    ));
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

    if action_scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found").to_string());
        } else {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：已列出可用代理", "Scan done").to_string());
        }
    }

    if let Some(port) = action_attach_discovered {
        ctx.proxy.request_attach(port);
        ctx.gui_cfg.attach.last_port = Some(port);
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "正在附着…", "Attaching...").into());
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
        ui.checkbox(
            &mut ctx.view.sessions.lock_order,
            pick(ctx.lang, "锁定顺序", "Lock order"),
        )
        .on_hover_text(pick(
            ctx.lang,
            "暂停自动重排（活跃/最近分区与新会话插入也会暂停）",
            "Pause auto reordering (active partitioning and new-session insertion are paused too).",
        ));
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add_sized(
            [320.0, 20.0],
            egui::TextEdit::singleline(&mut ctx.view.sessions.search).hint_text(pick(
                ctx.lang,
                "按 session_id / cwd / model / config 过滤…",
                "Filter by session_id / cwd / model / config...",
            )),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.sessions.search.clear();
        }
    });

    ui.add_space(6.0);

    let rows = build_session_rows(
        active,
        &recent,
        &session_effort_overrides,
        &session_config_overrides,
        &session_stats,
    );

    let mut row_index_by_id = HashMap::new();
    for (idx, row) in rows.iter().enumerate() {
        row_index_by_id.insert(row.session_id.clone(), idx);
    }

    sync_session_order(&mut ctx.view.sessions, &rows);

    let q = ctx.view.sessions.search.trim().to_lowercase();
    let filtered = ctx
        .view
        .sessions
        .ordered_session_ids
        .iter()
        .filter_map(|id| row_index_by_id.get(id).copied().map(|idx| &rows[idx]))
        .filter(|row| {
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
            session_row_matches_query(row, &q)
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
                .position(|row| row.session_id.as_deref() == Some(sid))
        })
        .unwrap_or(
            ctx.view
                .sessions
                .selected_idx
                .min(filtered.len().saturating_sub(1)),
        );

    ctx.view.sessions.selected_idx = selected_idx_in_filtered;
    let selected = filtered.get(ctx.view.sessions.selected_idx).copied();
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
                for (pos, row) in filtered.iter().enumerate() {
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

        cols[1].horizontal(|ui| {
            let can_copy = row.session_id.is_some();
            if ui
                .add_enabled(
                    can_copy,
                    egui::Button::new(pick(ctx.lang, "复制 session_id", "Copy session_id")),
                )
                .clicked()
            {
                if let Some(sid) = row.session_id.as_deref() {
                    ui.ctx().copy_text(sid.to_string());
                    *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
                }
            }

            let can_open_cwd = row.cwd.is_some();
            if ui
                .add_enabled(can_open_cwd, egui::Button::new(pick(ctx.lang, "打开 cwd", "Open cwd")))
                .clicked()
            {
                if let Some(cwd) = row.cwd.as_deref() {
                    let path = std::path::PathBuf::from(cwd);
                    if let Err(e) = open_in_file_manager(&path, false) {
                        *ctx.last_error = Some(format!("open cwd failed: {e}"));
                    }
                }
            }

            let can_open_transcript = row.session_id.is_some();
            if ui
                .add_enabled(
                    can_open_transcript,
                    egui::Button::new(pick(ctx.lang, "打开对话记录", "Open transcript")),
                )
                .clicked()
            {
                let Some(sid) = row.session_id.clone() else {
                    return;
                };
                match ctx
                    .rt
                    .block_on(crate::sessions::find_codex_session_file_by_id(&sid))
                {
                    Ok(Some(path)) => {
                        let pos = ctx.view.history.sessions.iter().position(|s| s.id == sid);
                        let selected_idx = if let Some(pos) = pos {
                            ctx.view.history.sessions[pos].path = path;
                            pos
                        } else {
                            ctx.view.history.sessions.insert(
                                0,
                                SessionSummary {
                                    id: sid.clone(),
                                    path,
                                    cwd: row.cwd.clone(),
                                    created_at: None,
                                    updated_at: None,
                                    last_response_at: None,
                                    user_turns: 0,
                                    assistant_turns: 0,
                                    rounds: 0,
                                    first_user_message: Some(
                                        pick(ctx.lang, "（来自 Sessions）", "(from Sessions)")
                                            .to_string(),
                                    ),
                                },
                            );
                            0
                        };

                        ctx.view.history.selected_idx = selected_idx;
                        ctx.view.history.selected_id = Some(sid.clone());
                        ctx.view.history.loaded_for = None;
                        ctx.view.history.auto_load_transcript = true;
                        ctx.view.requested_page = Some(Page::History);
                    }
                    Ok(None) => {
                        *ctx.last_error = Some(pick(
                            ctx.lang,
                            "未找到该 session_id 的本地 Codex 会话文件（~/.codex/sessions）。",
                            "No local Codex session file found for this session_id (~/.codex/sessions).",
                        ).to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("find session file failed: {e}"));
                    }
                }
            }
        });

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

fn session_row_matches_query(row: &SessionRow, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    for v in [
        row.session_id.as_deref(),
        row.cwd.as_deref(),
        row.last_model.as_deref(),
        row.last_provider_id.as_deref(),
        row.last_config_name.as_deref(),
    ] {
        if let Some(s) = v {
            if s.to_lowercase().contains(q) {
                return true;
            }
        }
    }
    false
}

fn sync_session_order(state: &mut SessionsViewState, rows: &[SessionRow]) {
    let mut current_set: HashSet<Option<String>> = HashSet::new();
    let mut active_set: HashSet<Option<String>> = HashSet::new();
    for row in rows {
        current_set.insert(row.session_id.clone());
        if row.active_count > 0 {
            active_set.insert(row.session_id.clone());
        }
    }

    if state.ordered_session_ids.is_empty() {
        state.ordered_session_ids = rows.iter().map(|r| r.session_id.clone()).collect();
        state.last_active_set = active_set;
        return;
    }

    // Always prune sessions that no longer exist in the current snapshot.
    state
        .ordered_session_ids
        .retain(|id| current_set.contains(id));

    // Ensure new sessions show up in the list. When auto reordering is enabled, insert them
    // just after the active partition (newest first, based on current snapshot ordering).
    let mut known: HashSet<Option<String>> = state.ordered_session_ids.iter().cloned().collect();
    let mut missing_active: Vec<Option<String>> = Vec::new();
    let mut missing_inactive: Vec<Option<String>> = Vec::new();
    for row in rows {
        if known.contains(&row.session_id) {
            continue;
        }
        known.insert(row.session_id.clone());
        if active_set.contains(&row.session_id) {
            missing_active.push(row.session_id.clone());
        } else {
            missing_inactive.push(row.session_id.clone());
        }
    }

    if state.lock_order {
        state.ordered_session_ids.extend(missing_active);
        state.ordered_session_ids.extend(missing_inactive);
        state.last_active_set = active_set;
        return;
    }

    // Partition active sessions to the top, without reshuffling within each partition.
    let mut active_ids: Vec<Option<String>> = Vec::new();
    let mut inactive_ids: Vec<Option<String>> = Vec::new();
    for id in state.ordered_session_ids.drain(..) {
        if active_set.contains(&id) {
            active_ids.push(id);
        } else {
            inactive_ids.push(id);
        }
    }
    state.ordered_session_ids.extend(active_ids);
    state.ordered_session_ids.extend(inactive_ids);

    let insert_at = state
        .ordered_session_ids
        .iter()
        .take_while(|id| active_set.contains(*id))
        .count();
    let active_missing_len = missing_active.len();
    state
        .ordered_session_ids
        .splice(insert_at..insert_at, missing_active);
    let insert_at2 = insert_at + active_missing_len;
    state
        .ordered_session_ids
        .splice(insert_at2..insert_at2, missing_inactive);

    state.last_active_set = active_set;
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

fn render_stats(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "统计", "Stats"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看统计。",
            "No proxy is running or attached. Start or attach on Overview to view stats.",
        ));
        return;
    };

    fn tokens_short(v: i64) -> String {
        let v = v.max(0) as u64;
        if v >= 1_000_000_000 {
            format!("{:.1}b", (v as f64) / 1_000_000_000.0)
        } else if v >= 1_000_000 {
            format!("{:.1}m", (v as f64) / 1_000_000.0)
        } else if v >= 1_000 {
            format!("{:.1}k", (v as f64) / 1_000.0)
        } else {
            v.to_string()
        }
    }

    fn fmt_pct(ok: usize, total: usize) -> String {
        if total == 0 {
            return "-".to_string();
        }
        format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
    }

    fn pricing_per_1k_usd() -> Option<(f64, f64)> {
        let input = std::env::var("CODEX_HELPER_PRICE_INPUT_PER_1K_USD")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())?;
        let output = std::env::var("CODEX_HELPER_PRICE_OUTPUT_PER_1K_USD")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())?;
        if input.is_finite() && output.is_finite() && input >= 0.0 && output >= 0.0 {
            Some((input, output))
        } else {
            None
        }
    }

    fn estimate_cost_usd(input_tokens: i64, output_tokens: i64) -> Option<f64> {
        let (in_price, out_price) = pricing_per_1k_usd()?;
        let input = (input_tokens.max(0) as f64) / 1000.0;
        let output = (output_tokens.max(0) as f64) / 1000.0;
        Some(input * in_price + output * out_price)
    }

    let rollup = &snapshot.usage_rollup;
    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;

    ui.separator();

    ui.label(format!(
        "{}: {}  {}: {}",
        pick(ctx.lang, "模式", "Mode"),
        match snapshot.kind {
            ProxyModeKind::Running => pick(ctx.lang, "运行中", "Running"),
            ProxyModeKind::Attached => pick(ctx.lang, "已附着", "Attached"),
            _ => pick(ctx.lang, "未知", "Unknown"),
        },
        pick(ctx.lang, "服务", "Service"),
        snapshot.service_name.as_deref().unwrap_or("-")
    ));

    ui.add_space(8.0);

    egui::Grid::new("stats_kpis_grid")
        .striped(true)
        .show(ui, |ui| {
            let since = &rollup.since_start;
            ui.label(pick(ctx.lang, "请求(累计)", "Requests (since start)"));
            ui.label(format!(
                "total={}  errors={}  err%={}",
                since.requests_total,
                since.requests_error,
                if since.requests_total == 0 {
                    "-".to_string()
                } else {
                    format!(
                        "{:.1}%",
                        (since.requests_error as f64) * 100.0 / (since.requests_total as f64)
                    )
                }
            ));
            ui.end_row();

            ui.label(pick(ctx.lang, "Tokens(累计)", "Tokens (since start)"));
            ui.label(format!(
                "in={}  out={}  rsn={}  ttl={}",
                tokens_short(since.usage.input_tokens),
                tokens_short(since.usage.output_tokens),
                tokens_short(since.usage.reasoning_tokens),
                tokens_short(since.usage.total_tokens)
            ));
            ui.end_row();

            ui.label(pick(ctx.lang, "成本(估算)", "Cost (estimated)"));
            let cost_hint = if pricing_per_1k_usd().is_some() {
                estimate_cost_usd(since.usage.input_tokens, since.usage.output_tokens)
                    .map(|v| format!("${v:.2}"))
                    .unwrap_or_else(|| "-".to_string())
            } else {
                pick(
                    ctx.lang,
                    "（设置 CODEX_HELPER_PRICE_* env）",
                    "(set CODEX_HELPER_PRICE_* env)",
                )
                .to_string()
            };
            ui.label(cost_hint);
            ui.end_row();

            ui.label(pick(ctx.lang, "窗口(5m)", "Window (5m)"));
            ui.label(format!(
                "ok={}  p95={}ms  att={}  429={}  5xx={}  n={}",
                fmt_pct(s5.ok_2xx, s5.total),
                s5.p95_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                s5.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.err_429,
                s5.err_5xx,
                s5.total
            ));
            ui.end_row();

            ui.label(pick(ctx.lang, "窗口(1h)", "Window (1h)"));
            ui.label(format!(
                "ok={}  p95={}ms  att={}  429={}  5xx={}  n={}",
                fmt_pct(s1.ok_2xx, s1.total),
                s1.p95_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                s1.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.err_429,
                s1.err_5xx,
                s1.total
            ));
            ui.end_row();
        });

    ui.add_space(10.0);
    ui.separator();
    ui.label(pick(
        ctx.lang,
        "Tokens / day（最近 14 天）",
        "Tokens / day (last 14 days)",
    ));

    let now_day = (now_ms() / 86_400_000) as i32;
    let mut by_day = rollup.by_day.clone();
    if by_day.len() > 14 {
        by_day = by_day[by_day.len().saturating_sub(14)..].to_vec();
    }
    let max_tok = by_day
        .iter()
        .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
        .max()
        .unwrap_or(0);

    egui::Grid::new("stats_by_day_grid")
        .striped(true)
        .show(ui, |ui| {
            ui.label(pick(ctx.lang, "天", "Day"));
            ui.label(pick(ctx.lang, "Tokens", "Tokens"));
            ui.label(pick(ctx.lang, "条", "Requests"));
            ui.end_row();

            for (day, b) in by_day.iter() {
                let delta = day - now_day;
                let label = if delta == 0 {
                    "d+0".to_string()
                } else if delta > 0 {
                    format!("d+{delta}")
                } else {
                    format!("d{delta}")
                };
                let tok = b.usage.total_tokens.max(0) as u64;
                let bar_len = if max_tok == 0 {
                    0
                } else {
                    ((tok as f64) * 24.0 / (max_tok as f64)).round() as usize
                };
                let bar = "▮".repeat(bar_len);
                ui.label(label);
                ui.label(format!("{}  {}", tokens_short(b.usage.total_tokens), bar));
                ui.label(b.requests_total.to_string());
                ui.end_row();
            }
        });

    ui.add_space(10.0);
    ui.separator();
    ui.label(pick(
        ctx.lang,
        "Top configs/providers（累计）",
        "Top configs/providers (since start)",
    ));

    ui.columns(2, |cols| {
        cols[0].label(pick(ctx.lang, "Configs", "Configs"));
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(&mut cols[0], |ui| {
                for (name, b) in rollup.by_config.iter().take(30) {
                    ui.label(format!(
                        "{}  tok={}  n={}  err={}",
                        shorten(name, 28),
                        tokens_short(b.usage.total_tokens),
                        b.requests_total,
                        b.requests_error
                    ));
                }
            });

        cols[1].label(pick(ctx.lang, "Providers", "Providers"));
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(&mut cols[1], |ui| {
                for (name, b) in rollup.by_provider.iter().take(30) {
                    ui.label(format!(
                        "{}  tok={}  n={}  err={}",
                        shorten(name, 28),
                        tokens_short(b.usage.total_tokens),
                        b.requests_total,
                        b.requests_error
                    ));
                }
            });
    });
}

fn render_history(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    poll_transcript_loader(ctx);

    ui.heading(pick(ctx.lang, "历史会话", "History"));
    ui.label(pick(
        ctx.lang,
        "读取 Codex 的本地 sessions（~/.codex/sessions）。",
        "Reads local Codex sessions (~/.codex/sessions).",
    ));

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "范围", "Scope"));
        egui::ComboBox::from_id_salt("history_scope")
            .selected_text(match ctx.view.history.scope {
                HistoryScope::CurrentProject => pick(ctx.lang, "当前项目", "Current project"),
                HistoryScope::GlobalRecent => pick(ctx.lang, "全局最近", "Global recent"),
                HistoryScope::AllByDate => pick(ctx.lang, "全部(按日期)", "All (by date)"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::CurrentProject,
                    pick(ctx.lang, "当前项目", "Current project"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::GlobalRecent,
                    pick(ctx.lang, "全局最近", "Global recent"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::AllByDate,
                    pick(ctx.lang, "全部(按日期)", "All (by date)"),
                );
            });

        if ctx.view.history.scope == HistoryScope::GlobalRecent {
            ui.label(pick(ctx.lang, "最近(小时)", "Since (hours)"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_since_hours)
                    .range(1..=168)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "条数", "Limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_limit)
                    .range(1..=500)
                    .speed(1),
            );
            ui.checkbox(
                &mut ctx.view.history.infer_git_root,
                pick(ctx.lang, "git 根目录", "git root"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "在 cwd 上向上查找 .git 作为项目根目录（用于复制/打开）",
                "Find .git upward from cwd as the project root (for copy/open).",
            ));
        } else if ctx.view.history.scope == HistoryScope::AllByDate {
            ui.label(pick(ctx.lang, "最近天数", "Recent days"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_days_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "当日上限", "Day limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_day_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
        }
    });

    if ctx.view.history.scope == HistoryScope::AllByDate {
        render_history_all_by_date(ui, ctx);
        return;
    }

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(240.0)
                .hint_text(pick(
                    ctx.lang,
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "输入关键词（匹配 cwd 或首条用户消息）"
                    } else {
                        "输入关键词（匹配首条用户消息）"
                    },
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "keyword (cwd or first user message)"
                    } else {
                        "keyword (first user message)"
                    },
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let query = ctx.view.history.query.trim().to_string();
            let scope = ctx.view.history.scope;
            let recent_since_hours = ctx.view.history.recent_since_hours;
            let recent_limit = ctx.view.history.recent_limit;
            let fut = async move {
                match scope {
                    HistoryScope::CurrentProject => {
                        if query.is_empty() {
                            crate::sessions::find_codex_sessions_for_current_dir(200).await
                        } else {
                            crate::sessions::search_codex_sessions_for_current_dir(&query, 200)
                                .await
                        }
                    }
                    HistoryScope::GlobalRecent => {
                        let since = std::time::Duration::from_secs(
                            (recent_since_hours as u64).saturating_mul(3600),
                        );
                        let mut list = crate::sessions::find_recent_codex_session_summaries(
                            since,
                            recent_limit,
                        )
                        .await?;

                        let q = query.trim().to_lowercase();
                        if !q.is_empty() {
                            list.retain(|s| {
                                s.cwd
                                    .as_deref()
                                    .is_some_and(|cwd| cwd.to_lowercase().contains(q.as_str()))
                                    || s.first_user_message
                                        .as_deref()
                                        .is_some_and(|msg| msg.to_lowercase().contains(q.as_str()))
                            });
                        }
                        Ok(list)
                    }
                    HistoryScope::AllByDate => Ok(Vec::new()),
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
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
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
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.transcript_plain_key = None;
                        ctx.view.history.transcript_plain_text.clear();
                    }
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ctx.view.history.scope == HistoryScope::GlobalRecent
            && ui
                .button(pick(ctx.lang, "复制 root+id 列表", "Copy root+id list"))
                .clicked()
        {
            let mut out = String::new();
            for s in ctx.view.history.sessions.iter() {
                let cwd = s.cwd.as_deref().unwrap_or("-");
                if cwd == "-" {
                    continue;
                }
                let root = if ctx.view.history.infer_git_root {
                    crate::sessions::infer_project_root_from_cwd(cwd)
                } else {
                    cwd.to_string()
                };
                out.push_str(root.trim());
                out.push(' ');
                out.push_str(s.id.as_str());
                out.push('\n');
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
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

    if ctx.view.history.auto_load_transcript
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (id.clone(), tail);
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        ensure_transcript_loading(ctx, path, key);
    }

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "会话列表", "Sessions"));
        cols[0].add_space(4.0);
        let mut pending_select: Option<(usize, String)> = None;
        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    let selected = idx == ctx.view.history.selected_idx;
                    let id_short = short_sid(&s.id, 16);
                    let rounds = s.rounds;
                    let last = s
                        .last_response_at
                        .as_deref()
                        .or(s.updated_at.as_deref())
                        .unwrap_or("-");
                    let first = s.first_user_message.as_deref().unwrap_or("-");
                    let label = match ctx.view.history.scope {
                        HistoryScope::CurrentProject => {
                            let cwd = s
                                .cwd
                                .as_deref()
                                .map(|v| shorten(basename(v), 22))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{id_short}  r={rounds}  {cwd}  {last}  {}",
                                shorten(first, 40)
                            )
                        }
                        HistoryScope::GlobalRecent => {
                            let root = s
                                .cwd
                                .as_deref()
                                .map(|cwd| {
                                    if ctx.view.history.infer_git_root {
                                        crate::sessions::infer_project_root_from_cwd(cwd)
                                    } else {
                                        cwd.to_string()
                                    }
                                })
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{}  {id_short}  r={rounds}  {last}  {}",
                                shorten(&root, 44),
                                shorten(first, 36)
                            )
                        }
                        HistoryScope::AllByDate => {
                            let root = s
                                .cwd
                                .as_deref()
                                .map(|cwd| {
                                    if ctx.view.history.infer_git_root {
                                        crate::sessions::infer_project_root_from_cwd(cwd)
                                    } else {
                                        cwd.to_string()
                                    }
                                })
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{}  {id_short}  r={rounds}  {last}  {}",
                                shorten(&root, 44),
                                shorten(first, 36)
                            )
                        }
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        pending_select = Some((idx, s.id.clone()));
                    }
                }
            });

        if let Some((idx, id)) = pending_select.take() {
            ctx.view.history.selected_idx = idx;
            ctx.view.history.selected_id = Some(id);
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
            ctx.view.history.transcript_error = None;
            ctx.view.history.transcript_plain_key = None;
            ctx.view.history.transcript_plain_text.clear();
            ctx.view.history.transcript_selected_msg_idx = 0;
        }

        cols[1].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[1].add_space(4.0);

        let selected = &ctx.view.history.sessions[selected_idx];
        let selected_id = selected.id.clone();
        let selected_cwd = selected.cwd.clone().unwrap_or_else(|| "-".to_string());
        let workdir = if selected_cwd != "-" && ctx.view.history.infer_git_root {
            crate::sessions::infer_project_root_from_cwd(&selected_cwd)
        } else {
            selected_cwd.clone()
        };
        let resume_cmd = {
            let t = ctx.view.history.resume_cmd.trim();
            if t.is_empty() {
                format!("codex resume {selected_id}")
            } else if t.contains("{id}") {
                t.replace("{id}", &selected_id)
            } else {
                format!("{t} {selected_id}")
            }
        };

        cols[1].group(|ui| {
            ui.label(pick(ctx.lang, "恢复", "Resume"));

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "命令模板", "Template"));
                ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.resume_cmd)
                        .desired_width(260.0),
                );
                if ui
                    .button(pick(ctx.lang, "用 bypass", "Use bypass"))
                    .clicked()
                {
                    ctx.view.history.resume_cmd =
                        "codex --dangerously-bypass-approvals-and-sandbox resume {id}".to_string();
                }
            });

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "Shell", "Shell"));
                ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.shell).desired_width(140.0),
                );
                ui.checkbox(
                    &mut ctx.view.history.keep_open,
                    pick(ctx.lang, "保持打开", "Keep open"),
                );
            });

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "复制 root+id", "Copy root+id"))
                    .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else {
                        ui.ctx()
                            .copy_text(format!("{} {}", workdir.trim(), selected_id));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                    }
                }

                if ui
                    .button(pick(ctx.lang, "复制 resume", "Copy resume"))
                    .clicked()
                {
                    ui.ctx().copy_text(resume_cmd.clone());
                    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                }

                if cfg!(windows)
                    && ui
                        .button(pick(ctx.lang, "在 wt 中恢复", "Open in wt"))
                        .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else if !std::path::Path::new(workdir.trim()).exists() {
                        *ctx.last_error = Some(format!(
                            "{}: {}",
                            pick(ctx.lang, "目录不存在", "Directory not found"),
                            workdir.trim()
                        ));
                    } else if let Err(e) = spawn_windows_terminal_wt_new_tab(
                        ctx.view.history.wt_window,
                        workdir.trim(),
                        ctx.view.history.shell.trim(),
                        ctx.view.history.keep_open,
                        &resume_cmd,
                    ) {
                        *ctx.last_error = Some(format!("spawn wt failed: {e}"));
                    }
                }
            });

            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        cols[1].horizontal(|ui| {
            let mut hide = ctx.view.history.hide_tool_calls;
            ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
            if hide != ctx.view.history.hide_tool_calls {
                ctx.view.history.hide_tool_calls = hide;
                ctx.view.history.transcript_messages =
                    filter_tool_calls(ctx.view.history.transcript_raw_messages.clone(), hide);
                ctx.view.history.transcript_plain_key = None;
                ctx.view.history.transcript_plain_text.clear();
            }

            ui.label(pick(ctx.lang, "显示", "View"));
            egui::ComboBox::from_id_salt("history_transcript_view")
                .selected_text(match ctx.view.history.transcript_view {
                    TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                    TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::Messages,
                        pick(ctx.lang, "消息列表", "Messages"),
                    );
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::PlainText,
                        pick(ctx.lang, "纯文本", "Plain text"),
                    );
                });

            let mut full = ctx.view.history.transcript_full;
            ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
            if full != ctx.view.history.transcript_full {
                ctx.view.history.transcript_full = full;
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
            }

            ui.label(pick(ctx.lang, "尾部条数", "Tail"));
            ui.add_enabled(
                !ctx.view.history.transcript_full,
                egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                    .range(10..=500)
                    .speed(1),
            );
            if ctx.view.history.transcript_full {
                ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
            }

            if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
                // Next frame will load (auto_load_transcript=true) or user can click again.
                if !ctx.view.history.auto_load_transcript {
                    ctx.view.history.auto_load_transcript = true;
                }
            }

            if ui.button(pick(ctx.lang, "打开文件", "Open file")).clicked() {
                let path = ctx.view.history.sessions[selected_idx].path.clone();
                if let Err(e) = open_in_file_manager(&path, true) {
                    *ctx.last_error = Some(format!("open session failed: {e}"));
                }
            }

            if ui.button(pick(ctx.lang, "复制", "Copy")).clicked() {
                let mut out = String::new();
                for msg in ctx.view.history.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    out.push_str(&format!("[{ts}] {role}:\n"));
                    out.push_str(msg.text.as_str());
                    out.push_str("\n\n");
                }
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }
        });

        if let Some(err) = ctx.view.history.transcript_error.as_deref() {
            cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        render_transcript_body(&mut cols[1], ctx, 480.0);
    });
}

fn render_history_all_by_date(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(260.0)
                .hint_text(pick(
                    ctx.lang,
                    "关键词（匹配 cwd 或首条用户消息）",
                    "keyword (cwd or first user message)",
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    ctx.view.history.loaded_day_for = None;
                    ctx.view.history.all_day_sessions.clear();
                    ctx.view.history.selected_id = None;
                    cancel_transcript_load(&mut ctx.view.history);
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ui
            .button(pick(ctx.lang, "加载更多天", "Load more days"))
            .clicked()
        {
            ctx.view.history.all_days_limit = ctx.view.history.all_days_limit.saturating_add(120);
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
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

    if ctx.view.history.all_dates.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无日期索引。点击“刷新”加载。",
            "No date index loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selected date stable.
    if ctx
        .view
        .history
        .all_selected_date
        .as_deref()
        .is_none_or(|d| !ctx.view.history.all_dates.iter().any(|x| x.date == d))
    {
        ctx.view.history.all_selected_date = Some(ctx.view.history.all_dates[0].date.clone());
        ctx.view.history.loaded_day_for = None;
    }

    // Auto-load day sessions when date changes.
    if let Some(date) = ctx.view.history.all_selected_date.clone() {
        if ctx.view.history.loaded_day_for.as_deref() != Some(date.as_str()) {
            let limit = ctx.view.history.all_day_limit;
            let day_dir = ctx
                .view
                .history
                .all_dates
                .iter()
                .find(|x| x.date == date)
                .map(|x| x.path.clone());
            if let Some(day_dir) = day_dir {
                match ctx
                    .rt
                    .block_on(crate::sessions::list_codex_sessions_in_day_dir(
                        &day_dir, limit,
                    )) {
                    Ok(list) => {
                        ctx.view.history.all_day_sessions = list;
                        ctx.view.history.loaded_day_for = Some(date.clone());
                        ctx.view.history.selected_id = None;
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = None;
                    }
                    Err(e) => {
                        ctx.view.history.last_error = Some(e.to_string());
                    }
                }
            }
        }
    }

    let q = ctx.view.history.query.trim().to_lowercase();

    ui.add_space(6.0);
    ui.columns(3, |cols| {
        cols[0].heading(pick(ctx.lang, "日期", "Dates"));
        cols[0].add_space(4.0);
        {
            let total = ctx.view.history.all_dates.len();
            let row_h = 22.0;
            egui::ScrollArea::vertical().max_height(520.0).show_rows(
                &mut cols[0],
                row_h,
                total,
                |ui, range| {
                    for row in range {
                        let d = &ctx.view.history.all_dates[row];
                        let selected = ctx
                            .view
                            .history
                            .all_selected_date
                            .as_deref()
                            .is_some_and(|x| x == d.date);
                        if ui.selectable_label(selected, d.date.as_str()).clicked() {
                            ctx.view.history.all_selected_date = Some(d.date.clone());
                            ctx.view.history.loaded_day_for = None;
                        }
                    }
                },
            );
        }

        cols[1].heading(pick(ctx.lang, "会话", "Sessions"));
        cols[1].add_space(4.0);

        let mut visible_indices: Vec<usize> = Vec::new();
        for (idx, s) in ctx.view.history.all_day_sessions.iter().enumerate() {
            if q.is_empty() {
                visible_indices.push(idx);
                continue;
            }
            let mut matched = false;
            if let Some(cwd) = s.cwd.as_deref() {
                matched |= cwd.to_lowercase().contains(q.as_str());
            }
            if let Some(msg) = s.first_user_message.as_deref() {
                matched |= msg.to_lowercase().contains(q.as_str());
            }
            if matched {
                visible_indices.push(idx);
            }
        }

        {
            let total = visible_indices.len();
            let row_h = 22.0;
            egui::ScrollArea::vertical().max_height(520.0).show_rows(
                &mut cols[1],
                row_h,
                total,
                |ui, range| {
                    for row in range {
                        let idx = visible_indices[row];
                        let s = &ctx.view.history.all_day_sessions[idx];
                        let selected = ctx
                            .view
                            .history
                            .selected_id
                            .as_deref()
                            .is_some_and(|id| id == s.id);

                        let id_short = short_sid(&s.id, 16);
                        let t = s
                            .updated_hint
                            .as_deref()
                            .or(s.created_at.as_deref())
                            .unwrap_or("-");
                        let root_or_cwd = s
                            .cwd
                            .as_deref()
                            .map(|cwd| {
                                if ctx.view.history.infer_git_root {
                                    crate::sessions::infer_project_root_from_cwd(cwd)
                                } else {
                                    cwd.to_string()
                                }
                            })
                            .unwrap_or_else(|| "-".to_string());
                        let first = s.first_user_message.as_deref().unwrap_or("-");
                        let label = format!(
                            "{}  {}  {}  {}",
                            shorten(&root_or_cwd, 36),
                            id_short,
                            shorten(t, 19),
                            shorten(first, 40)
                        );
                        if ui.selectable_label(selected, label).clicked() {
                            ctx.view.history.selected_id = Some(s.id.clone());
                            ctx.view.history.selected_idx = idx;
                            ctx.view.history.loaded_for = None;
                            cancel_transcript_load(&mut ctx.view.history);
                            ctx.view.history.transcript_raw_messages.clear();
                            ctx.view.history.transcript_messages.clear();
                            ctx.view.history.transcript_error = None;
                            ctx.view.history.transcript_plain_key = None;
                            ctx.view.history.transcript_plain_text.clear();
                        }
                    }
                },
            );
        }

        cols[2].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[2].add_space(4.0);

        let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
            ctx.view
                .history
                .all_day_sessions
                .iter()
                .position(|s| s.id == id)
        });
        let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx));

        if selected.is_none() {
            cols[2].label(pick(
                ctx.lang,
                "选择一个会话以预览对话。",
                "Select a session to preview.",
            ));
            return;
        }
        let selected = selected.unwrap();
        let selected_id = selected.id.clone();
        let selected_cwd = selected.cwd.clone().unwrap_or_else(|| "-".to_string());

        let workdir = if selected_cwd != "-" && ctx.view.history.infer_git_root {
            crate::sessions::infer_project_root_from_cwd(&selected_cwd)
        } else {
            selected_cwd.clone()
        };

        let resume_cmd = {
            let t = ctx.view.history.resume_cmd.trim();
            if t.is_empty() {
                format!("codex resume {selected_id}")
            } else if t.contains("{id}") {
                t.replace("{id}", &selected_id)
            } else {
                format!("{t} {selected_id}")
            }
        };

        cols[2].group(|ui| {
            ui.label(pick(ctx.lang, "恢复", "Resume"));

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "命令模板", "Template"));
                ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.resume_cmd)
                        .desired_width(260.0),
                );
                if ui
                    .button(pick(ctx.lang, "用 bypass", "Use bypass"))
                    .clicked()
                {
                    ctx.view.history.resume_cmd =
                        "codex --dangerously-bypass-approvals-and-sandbox resume {id}".to_string();
                }
            });

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "Shell", "Shell"));
                ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.shell).desired_width(140.0),
                );
                ui.checkbox(
                    &mut ctx.view.history.keep_open,
                    pick(ctx.lang, "保持打开", "Keep open"),
                );
            });

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "复制 root+id", "Copy root+id"))
                    .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else {
                        ui.ctx()
                            .copy_text(format!("{} {}", workdir.trim(), selected_id));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                    }
                }

                if ui
                    .button(pick(ctx.lang, "复制 resume", "Copy resume"))
                    .clicked()
                {
                    ui.ctx().copy_text(resume_cmd.clone());
                    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                }

                if cfg!(windows)
                    && ui
                        .button(pick(ctx.lang, "在 wt 中恢复", "Open in wt"))
                        .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else if !std::path::Path::new(workdir.trim()).exists() {
                        *ctx.last_error = Some(format!(
                            "{}: {}",
                            pick(ctx.lang, "目录不存在", "Directory not found"),
                            workdir.trim()
                        ));
                    } else if let Err(e) = spawn_windows_terminal_wt_new_tab(
                        ctx.view.history.wt_window,
                        workdir.trim(),
                        ctx.view.history.shell.trim(),
                        ctx.view.history.keep_open,
                        &resume_cmd,
                    ) {
                        *ctx.last_error = Some(format!("spawn wt failed: {e}"));
                    }
                }

                if ui.button(pick(ctx.lang, "打开文件", "Open file")).clicked() {
                    if let Err(e) = open_in_file_manager(&selected.path, true) {
                        *ctx.last_error = Some(format!("open session failed: {e}"));
                    }
                }
            });

            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        if ctx.view.history.auto_load_transcript {
            let tail = if ctx.view.history.transcript_full {
                None
            } else {
                Some(ctx.view.history.transcript_tail)
            };
            let key = (selected_id.clone(), tail);
            ensure_transcript_loading(ctx, selected.path.clone(), key);
        }

        cols[2].add_space(6.0);
        cols[2].horizontal(|ui| {
            let mut hide = ctx.view.history.hide_tool_calls;
            ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
            if hide != ctx.view.history.hide_tool_calls {
                ctx.view.history.hide_tool_calls = hide;
                ctx.view.history.transcript_messages =
                    filter_tool_calls(ctx.view.history.transcript_raw_messages.clone(), hide);
                ctx.view.history.transcript_plain_key = None;
                ctx.view.history.transcript_plain_text.clear();
            }

            ui.label(pick(ctx.lang, "显示", "View"));
            egui::ComboBox::from_id_salt("history_transcript_view_all")
                .selected_text(match ctx.view.history.transcript_view {
                    TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                    TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::Messages,
                        pick(ctx.lang, "消息列表", "Messages"),
                    );
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::PlainText,
                        pick(ctx.lang, "纯文本", "Plain text"),
                    );
                });
            let mut full = ctx.view.history.transcript_full;
            ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
            if full != ctx.view.history.transcript_full {
                ctx.view.history.transcript_full = full;
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
            }

            ui.label(pick(ctx.lang, "尾部条数", "Tail"));
            ui.add_enabled(
                !ctx.view.history.transcript_full,
                egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                    .range(10..=500)
                    .speed(1),
            );
            if ctx.view.history.transcript_full {
                ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
            }

            if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
                if !ctx.view.history.auto_load_transcript {
                    ctx.view.history.auto_load_transcript = true;
                }
            }

            if ui.button(pick(ctx.lang, "复制", "Copy")).clicked() {
                let mut out = String::new();
                for msg in ctx.view.history.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    out.push_str(&format!("[{ts}] {role}:\n"));
                    out.push_str(msg.text.as_str());
                    out.push_str("\n\n");
                }
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }
        });

        if let Some(err) = ctx.view.history.transcript_error.as_deref() {
            cols[2].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        render_transcript_body(&mut cols[2], ctx, 360.0);
    });
}

fn cancel_transcript_load(state: &mut HistoryViewState) {
    if let Some(load) = state.transcript_load.take() {
        load.join.abort();
    }
}

fn poll_transcript_loader(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history.transcript_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history.transcript_load = None;
                return;
            }

            let key = load.key.clone();
            ctx.view.history.transcript_load = None;

            match res {
                Ok(msgs) => {
                    ctx.view.history.transcript_raw_messages = msgs;
                    ctx.view.history.transcript_messages = filter_tool_calls(
                        ctx.view.history.transcript_raw_messages.clone(),
                        ctx.view.history.hide_tool_calls,
                    );
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = Some(key);
                    ctx.view.history.transcript_selected_msg_idx = 0;
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
                Err(e) => {
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = Some(e.to_string());
                    ctx.view.history.loaded_for = None;
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            ctx.view.history.transcript_load = None;
        }
    }
}

fn ensure_transcript_loading(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    if ctx.view.history.loaded_for.as_ref() == Some(&key) {
        return;
    }
    if let Some(load) = ctx.view.history.transcript_load.as_ref()
        && load.key == key
    {
        return;
    }

    start_transcript_load(ctx, path, key);
}

fn start_transcript_load(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    cancel_transcript_load(&mut ctx.view.history);

    ctx.view.history.transcript_load_seq = ctx.view.history.transcript_load_seq.saturating_add(1);
    let seq = ctx.view.history.transcript_load_seq;
    let tail = key.1;

    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let res = crate::sessions::read_codex_session_transcript(&path, tail).await;
        let _ = tx.send((seq, res));
    });

    ctx.view.history.transcript_load = Some(TranscriptLoad { seq, key, rx, join });
}

fn render_transcript_body(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, max_height: f32) {
    if ctx.view.history.transcript_load.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(pick(ctx.lang, "加载中…", "Loading..."));
        });
        ui.add_space(6.0);
    }

    if ctx.view.history.transcript_messages.is_empty() {
        ui.label(pick(ctx.lang, "（无内容）", "(empty)"));
        return;
    }

    match ctx.view.history.transcript_view {
        TranscriptViewMode::Messages => {
            let list_h = (max_height * 0.45).clamp(140.0, 260.0);
            let total = ctx.view.history.transcript_messages.len();
            let row_h = 22.0;

            egui::ScrollArea::vertical().max_height(list_h).show_rows(
                ui,
                row_h,
                total,
                |ui, range| {
                    for i in range {
                        let selected = i == ctx.view.history.transcript_selected_msg_idx;
                        let (ts, role, preview) = {
                            let m = &ctx.view.history.transcript_messages[i];
                            let ts = m.timestamp.as_deref().unwrap_or("-");
                            let role = m.role.as_str();
                            let first_line = m.text.lines().next().unwrap_or("");
                            let preview = first_line.replace('\t', " ");
                            (ts.to_string(), role.to_string(), preview)
                        };
                        let label = format!(
                            "#{:>4}  {}  {}  {}",
                            i.saturating_add(1),
                            shorten(&ts, 19),
                            shorten(&role, 10),
                            shorten(&preview, 60)
                        );
                        if ui.selectable_label(selected, label).clicked() {
                            ctx.view.history.transcript_selected_msg_idx = i;
                        }
                    }
                },
            );

            ui.add_space(6.0);

            if total == 0 {
                return;
            }
            let idx = ctx
                .view
                .history
                .transcript_selected_msg_idx
                .min(total.saturating_sub(1));
            ctx.view.history.transcript_selected_msg_idx = idx;

            let (ts, role) = {
                let m = &ctx.view.history.transcript_messages[idx];
                (
                    m.timestamp.clone().unwrap_or_else(|| "-".to_string()),
                    m.role.clone(),
                )
            };
            ui.label(format!("[{ts}] {role}:"));
            ui.add(
                egui::TextEdit::multiline(&mut ctx.view.history.transcript_messages[idx].text)
                    .desired_rows(6)
                    .font(egui::TextStyle::Monospace)
                    .interactive(false),
            );
        }
        TranscriptViewMode::PlainText => {
            let cache_key = ctx
                .view
                .history
                .loaded_for
                .clone()
                .map(|(id, tail)| (id, tail, ctx.view.history.hide_tool_calls));
            if let Some(k) = cache_key.clone()
                && ctx.view.history.transcript_plain_key.as_ref() != Some(&k)
            {
                ctx.view.history.transcript_plain_text.clear();
                for msg in ctx.view.history.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    ctx.view
                        .history
                        .transcript_plain_text
                        .push_str(&format!("[{ts}] {role}:\n"));
                    ctx.view.history.transcript_plain_text.push_str(&msg.text);
                    ctx.view.history.transcript_plain_text.push_str("\n\n");
                }
                ctx.view.history.transcript_plain_key = Some(k);
            }

            egui::ScrollArea::vertical()
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut ctx.view.history.transcript_plain_text)
                            .desired_rows(18)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                });
        }
    }
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

fn filter_tool_calls(
    mut msgs: Vec<SessionTranscriptMessage>,
    hide_tool_calls: bool,
) -> Vec<SessionTranscriptMessage> {
    if !hide_tool_calls {
        return msgs;
    }
    msgs.retain(|m| {
        let role = m.role.trim().to_ascii_lowercase();
        role != "tool" && role != "tools" && role != "function"
    });
    msgs
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
        let mut enabled = ctx.gui_cfg.proxy.auto_attach_or_start;
        ui.checkbox(
            &mut enabled,
            pick(
                ctx.lang,
                "启动时自动附着/启动代理",
                "Auto attach-or-start on launch",
            ),
        );
        if enabled != ctx.gui_cfg.proxy.auto_attach_or_start {
            ctx.gui_cfg.proxy.auto_attach_or_start = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        let mut enabled = ctx.gui_cfg.proxy.discovery_scan_fallback;
        ui.checkbox(
            &mut enabled,
            pick(
                ctx.lang,
                "探测失败后扫 3210-3220",
                "Scan 3210-3220 on failure",
            ),
        );
        if enabled != ctx.gui_cfg.proxy.discovery_scan_fallback {
            ctx.gui_cfg.proxy.discovery_scan_fallback = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

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
        ui.label(pick(ctx.lang, "启动时行为", "Startup behavior"));

        let mut behavior = ctx.gui_cfg.window.startup_behavior.clone();
        let selected_label = match behavior.as_str() {
            "show" => pick(ctx.lang, "显示窗口", "Show window"),
            "minimized" => pick(ctx.lang, "最小化到任务栏", "Minimize"),
            _ => pick(ctx.lang, "最小化到托盘", "Minimize to tray"),
        };

        egui::ComboBox::from_id_salt("window_startup_behavior")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut behavior,
                    "show".to_string(),
                    pick(ctx.lang, "显示窗口", "Show window"),
                );
                ui.selectable_value(
                    &mut behavior,
                    "minimized".to_string(),
                    pick(ctx.lang, "最小化到任务栏", "Minimize"),
                );
                ui.selectable_value(
                    &mut behavior,
                    "minimize_to_tray".to_string(),
                    pick(ctx.lang, "最小化到托盘", "Minimize to tray"),
                );
            });

        if behavior != ctx.gui_cfg.window.startup_behavior {
            ctx.gui_cfg.window.startup_behavior = behavior;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info = Some(
                    pick(ctx.lang, "已保存（下次启动生效）", "Saved (next launch)").to_string(),
                );
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

        if ui
            .button(pick(ctx.lang, "从 Codex 导入", "Import from Codex"))
            .clicked()
        {
            ctx.view.config.import_codex.open = true;
            ctx.view.config.import_codex.last_error = None;
            ctx.view.config.import_codex.preview = None;
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

    // Modal: import/sync providers from Codex CLI.
    let mut do_preview = false;
    let mut do_apply = false;
    if ctx.view.config.import_codex.open {
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new(pick(
            ctx.lang,
            "从 Codex 导入（providers / env_key）",
            "Import from Codex (providers / env_key)",
        ))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label(pick(
                ctx.lang,
                "读取 ~/.codex/config.toml 与 ~/.codex/auth.json，同步 providers 的 base_url/env_key（仅写入 env var 名，不写入密钥）。",
                "Reads ~/.codex/config.toml and ~/.codex/auth.json, syncing providers' base_url/env_key (writes only env var names, no secrets).",
            ));
            ui.add_space(6.0);

            ui.checkbox(
                &mut ctx.view.config.import_codex.add_missing,
                pick(ctx.lang, "添加缺失的 provider", "Add missing providers"),
            );
            ui.checkbox(
                &mut ctx.view.config.import_codex.set_active,
                pick(
                    ctx.lang,
                    "同步 active 为 Codex 当前 model_provider",
                    "Set active to Codex model_provider",
                ),
            );
            ui.checkbox(
                &mut ctx.view.config.import_codex.force,
                pick(ctx.lang, "强制覆盖（谨慎）", "Force overwrite (careful)"),
            );
            if ctx.view.config.import_codex.force {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 40),
                    pick(
                        ctx.lang,
                        "强制覆盖可能会覆盖非 Codex 来源的上游配置，请确认。",
                        "Force overwrite may override non-Codex upstreams. Use with care.",
                    ),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(pick(ctx.lang, "预览", "Preview")).clicked() {
                    do_preview = true;
                }
                if ui.button(pick(ctx.lang, "应用并保存", "Apply & save")).clicked() {
                    do_apply = true;
                }
                if ui.button(pick(ctx.lang, "关闭", "Close")).clicked() {
                    close_clicked = true;
                }
            });

            if let Some(err) = ctx.view.config.import_codex.last_error.as_deref() {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }

            if let Some(report) = ctx.view.config.import_codex.preview.as_ref() {
                ui.add_space(6.0);
                ui.label(format!(
                    "{}: updated={} added={} active_set={}",
                    pick(ctx.lang, "预览结果", "Preview"),
                    report.updated,
                    report.added,
                    report.active_set
                ));
                if !report.warnings.is_empty() {
                    ui.add_space(4.0);
                    ui.label(pick(ctx.lang, "警告：", "Warnings:"));
                    for w in report.warnings.iter().take(12) {
                        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), w);
                    }
                    if report.warnings.len() > 12 {
                        ui.label(format!("… +{} more", report.warnings.len() - 12));
                    }
                }
            }
        });
        if close_clicked {
            open = false;
        }
        ctx.view.config.import_codex.open = open;
    }

    if do_preview {
        let options = crate::config::SyncCodexAuthFromCodexOptions {
            add_missing: ctx.view.config.import_codex.add_missing,
            set_active: ctx.view.config.import_codex.set_active,
            force: ctx.view.config.import_codex.force,
        };

        let tmp_opt = if let Some(cfg) = ctx.view.config.working.as_ref() {
            Some(cfg.clone())
        } else {
            match std::fs::read_to_string(ctx.proxy_config_path) {
                Ok(t) => match parse_proxy_config(&t) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        ctx.view.config.import_codex.last_error =
                            Some(format!("parse config failed: {e}"));
                        None
                    }
                },
                Err(e) => {
                    ctx.view.config.import_codex.last_error =
                        Some(format!("read config failed: {e}"));
                    None
                }
            }
        };

        if let Some(mut tmp) = tmp_opt {
            match crate::config::sync_codex_auth_from_codex_cli(&mut tmp, options) {
                Ok(report) => {
                    ctx.view.config.import_codex.preview = Some(report);
                    ctx.view.config.import_codex.last_error = None;
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已生成预览", "Preview ready").to_string());
                }
                Err(e) => {
                    ctx.view.config.import_codex.preview = None;
                    ctx.view.config.import_codex.last_error = Some(e.to_string());
                }
            }
        } else {
            ctx.view.config.import_codex.preview = None;
        }
    }

    if do_apply {
        let options = crate::config::SyncCodexAuthFromCodexOptions {
            add_missing: ctx.view.config.import_codex.add_missing,
            set_active: ctx.view.config.import_codex.set_active,
            force: ctx.view.config.import_codex.force,
        };

        let mut can_apply = true;
        if ctx.view.config.working.is_none() {
            match std::fs::read_to_string(ctx.proxy_config_path) {
                Ok(t) => match parse_proxy_config(&t) {
                    Ok(cfg) => {
                        ctx.view.config.working = Some(cfg);
                        ctx.view.config.load_error = None;
                    }
                    Err(e) => {
                        ctx.view.config.import_codex.last_error =
                            Some(format!("parse config failed: {e}"));
                        can_apply = false;
                    }
                },
                Err(e) => {
                    ctx.view.config.import_codex.last_error =
                        Some(format!("read config failed: {e}"));
                    can_apply = false;
                }
            }
        }

        let report = if can_apply {
            match crate::config::sync_codex_auth_from_codex_cli(
                ctx.view.config.working.as_mut().expect("loaded above"),
                options,
            ) {
                Ok(r) => Some(r),
                Err(e) => {
                    ctx.view.config.import_codex.last_error = Some(e.to_string());
                    ctx.view.config.import_codex.preview = None;
                    None
                }
            }
        } else {
            None
        };

        if let Some(report) = report {
            let summary = format!(
                "updated={} added={} active_set={}",
                report.updated, report.added, report.active_set
            );

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

                    ctx.view.config.import_codex.preview = Some(report);
                    ctx.view.config.import_codex.last_error = None;
                    *ctx.last_info = Some(format!(
                        "{}: {summary}",
                        pick(ctx.lang, "已导入并保存", "Imported & saved")
                    ));
                }
                Err(e) => {
                    ctx.view.config.import_codex.preview = Some(report);
                    ctx.view.config.import_codex.last_error = Some(format!("save failed: {e}"));
                    *ctx.last_error = Some(format!("save failed: {e}"));
                }
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

    let (active_name, active_fallback, names) = {
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
        (
            mgr.active.clone(),
            mgr.active_config().map(|c| c.name.clone()),
            v,
        )
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
    let mut action_clear_active = false;
    let mut action_health_start: Option<(bool, Vec<String>)> = None;
    let mut action_health_cancel: Option<(bool, Vec<String>)> = None;
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
                        let is_fallback_active = active_name.is_none()
                            && active_fallback.as_deref() == Some(name.as_str());
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
                        } else if is_fallback_active {
                            label = format!("◇ {label}");
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
            let active_label = mgr
                .active
                .clone()
                .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>").to_string());
            let effective_label = mgr
                .active_config()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "-".to_string());

            cols[1].label(format!("active: {active_label}"));
            cols[1].label(format!(
                "{}: {effective_label}",
                pick(ctx.lang, "生效配置", "Effective")
            ));
            cols[1].add_space(6.0);

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

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(ctx.lang, "健康检查", "Health check"));

            let selected_service = match ctx.view.config.service {
                crate::config::ServiceKind::Claude => "claude",
                crate::config::ServiceKind::Codex => "codex",
            };

            let (runtime_service, supports_v1, cfg_health, hc_status): (
                Option<String>,
                bool,
                Option<ConfigHealth>,
                Option<HealthCheckStatus>,
            ) = match ctx.proxy.kind() {
                ProxyModeKind::Running => {
                    if let Some(r) = ctx.proxy.running() {
                        let state = r.state.clone();
                        let (health, checks) = ctx.rt.block_on(async {
                            tokio::join!(
                                state.get_config_health(r.service_name),
                                state.list_health_checks(r.service_name)
                            )
                        });
                        (
                            Some(r.service_name.to_string()),
                            true,
                            health.get(&name).cloned(),
                            checks.get(&name).cloned(),
                        )
                    } else {
                        (None, false, None, None)
                    }
                }
                ProxyModeKind::Attached => {
                    if let Some(att) = ctx.proxy.attached() {
                        (
                            att.service_name.clone(),
                            att.api_version == Some(1),
                            att.config_health.get(&name).cloned(),
                            att.health_checks.get(&name).cloned(),
                        )
                    } else {
                        (None, false, None, None)
                    }
                }
                _ => (None, false, None, None),
            };

            if runtime_service.is_none() {
                cols[1].label(pick(
                    ctx.lang,
                    "代理未运行/未附着，无法执行健康检查。",
                    "Proxy is not running/attached; health check disabled.",
                ));
            } else if !supports_v1 {
                cols[1].label(pick(
                    ctx.lang,
                    "附着代理未启用 API v1：健康检查不可用。",
                    "Attached proxy has no API v1: health check disabled.",
                ));
            } else if runtime_service.as_deref() != Some(selected_service) {
                cols[1].label(pick(
                    ctx.lang,
                    "当前代理服务与所选服务不一致：健康检查已禁用。",
                    "Runtime service differs from selected service: health check disabled.",
                ));
            } else {
                if let Some(st) = hc_status.as_ref() {
                    cols[1].label(format!(
                        "status: {}/{} ok={} err={} cancel={} done={}",
                        st.completed, st.total, st.ok, st.err, st.cancel_requested, st.done
                    ));
                    if let Some(e) = st.last_error.as_deref() {
                        cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), e);
                    }
                } else {
                    cols[1].label(pick(ctx.lang, "(无状态)", "(no status)"));
                }

                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "检查当前", "Check selected"))
                        .clicked()
                    {
                        action_health_start = Some((false, vec![name.clone()]));
                    }
                    if ui
                        .button(pick(ctx.lang, "取消当前", "Cancel selected"))
                        .clicked()
                    {
                        action_health_cancel = Some((false, vec![name.clone()]));
                    }
                    if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
                        action_health_start = Some((true, Vec::new()));
                    }
                    if ui
                        .button(pick(ctx.lang, "取消全部", "Cancel all"))
                        .clicked()
                    {
                        action_health_cancel = Some((true, Vec::new()));
                    }
                });

                if let Some(h) = cfg_health.as_ref() {
                    cols[1].add_space(6.0);
                    cols[1].label(format!(
                        "{}: {}  upstreams={}",
                        pick(ctx.lang, "最近检查", "Last checked"),
                        h.checked_at_ms,
                        h.upstreams.len()
                    ));
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(&mut cols[1], |ui| {
                            let max = 12usize;
                            for up in h.upstreams.iter().rev().take(max) {
                                let ok = up.ok.map(|v| if v { "ok" } else { "err" }).unwrap_or("-");
                                let sc = up
                                    .status_code
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "-".to_string());
                                let lat = up
                                    .latency_ms
                                    .map(|v| format!("{v}ms"))
                                    .unwrap_or_else(|| "-".to_string());
                                let err = up
                                    .error
                                    .as_deref()
                                    .map(|e| shorten(e, 60))
                                    .unwrap_or_else(|| "-".to_string());
                                ui.label(format!(
                                    "{ok} {sc} {lat}  {}  {err}",
                                    shorten_middle(&up.base_url, 48)
                                ));
                            }
                            if h.upstreams.len() > max {
                                ui.label(format!("… +{} more", h.upstreams.len() - max));
                            }
                        });
                }
            }

            cols[1].add_space(6.0);
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "设为 active", "Set active"))
                    .clicked()
                {
                    action_set_active = Some(name.clone());
                }

                if ui
                    .button(pick(ctx.lang, "清除 active", "Clear active"))
                    .clicked()
                {
                    action_clear_active = true;
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

    if action_clear_active {
        let cfg = ctx.view.config.working.as_mut().expect("checked above");
        let mgr = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        mgr.active = None;
        *ctx.last_info = Some(pick(ctx.lang, "已清除 active", "Active cleared").to_string());
    }

    if let Some((all, names)) = action_health_start {
        if let Err(e) = ctx.proxy.start_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check start failed: {e}"));
        } else {
            *ctx.last_info =
                Some(pick(ctx.lang, "已开始健康检查", "Health check started").to_string());
        }
    }

    if let Some((all, names)) = action_health_cancel {
        if let Err(e) = ctx.proxy.cancel_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check cancel failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
        }
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

fn config_options_from_gui_configs(configs: &[ConfigOption]) -> Vec<(String, String)> {
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
