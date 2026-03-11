use eframe::egui;

use std::collections::{BTreeMap, HashMap, HashSet};

use super::autostart;
use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::proxy_control::{DiscoveredProxy, GuiRuntimeSnapshot, PortInUseAction, ProxyModeKind};
use super::util::{
    open_in_file_manager, spawn_windows_terminal_wt_new_tab,
    spawn_windows_terminal_wt_tabs_in_one_window,
};
use crate::config::{
    GroupConfigV2, GroupMemberRefV2, PersistedProviderSpec, PersistedStationProviderRef,
    PersistedStationSpec, ProviderConfigV2, ProviderEndpointV2, RetryConfig, RetryProfileName,
    RetryStrategy,
};
use crate::dashboard_core::{
    CapabilitySupport, ConfigCapabilitySummary, ConfigOption, ControlProfileOption,
    HostLocalControlPlaneCapabilities, ModelCatalogKind, RemoteAdminAccessCapabilities,
};
use crate::doctor::{DoctorLang, DoctorStatus};
use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, LbConfigView,
    ResolvedRouteValue, RouteValueSource, RuntimeConfigState, SessionIdentityCard,
    SessionObservationScope, SessionStats,
};
use crate::usage::UsageMetrics;

mod components;
mod history;

pub use history::HistoryViewState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Overview,
    Stations,
    Doctor,
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
    pub stations: StationsViewState,
    pub doctor: DoctorViewState,
    pub sessions: SessionsViewState,
    pub requests: RequestsViewState,
    pub config: ConfigViewState,
    pub history: HistoryViewState,
}

#[derive(Debug, Default)]
pub struct StationsViewState {
    pub search: String,
    pub enabled_only: bool,
    pub overrides_only: bool,
    pub selected_name: Option<String>,
    retry_editor: StationsRetryEditorState,
}

#[derive(Debug, Default)]
struct StationsRetryEditorState {
    source_signature: Option<String>,
    profile: String,
    cloudflare_challenge_cooldown_secs: String,
    cloudflare_timeout_cooldown_secs: String,
    transport_cooldown_secs: String,
    cooldown_backoff_factor: String,
    cooldown_backoff_max_secs: String,
}

#[derive(Debug, Default)]
pub struct DoctorViewState {
    pub report: Option<crate::doctor::DoctorReport>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ConfigMode {
    #[default]
    Form,
    Raw,
}

#[derive(Debug)]
pub struct ConfigViewState {
    mode: ConfigMode,
    service: crate::config::ServiceKind,
    selected_name: Option<String>,
    station_editor: ConfigStationEditorState,
    selected_provider_name: Option<String>,
    provider_editor: ConfigProviderEditorState,
    selected_profile_name: Option<String>,
    new_profile_name: String,
    profile_editor: ConfigProfileEditorState,
    working: Option<ConfigWorkingDocument>,
    load_error: Option<String>,
    import_codex: ImportCodexModalState,
}

impl Default for ConfigViewState {
    fn default() -> Self {
        Self {
            mode: ConfigMode::Form,
            service: crate::config::ServiceKind::Codex,
            selected_name: None,
            station_editor: ConfigStationEditorState::default(),
            selected_provider_name: None,
            provider_editor: ConfigProviderEditorState::default(),
            selected_profile_name: None,
            new_profile_name: String::new(),
            profile_editor: ConfigProfileEditorState::default(),
            working: None,
            load_error: None,
            import_codex: ImportCodexModalState::default(),
        }
    }
}

#[derive(Debug, Default)]
struct ConfigStationEditorState {
    station_name: Option<String>,
    alias: String,
    enabled: bool,
    level: u8,
    members: Vec<ConfigStationMemberEditorState>,
    new_station_name: String,
}

#[derive(Debug, Default, Clone)]
struct ConfigStationMemberEditorState {
    provider: String,
    endpoint_names: String,
    preferred: bool,
}

#[derive(Debug, Default)]
struct ConfigProviderEditorState {
    provider_name: Option<String>,
    alias: String,
    enabled: bool,
    auth_token_env: String,
    api_key_env: String,
    endpoints: Vec<ConfigProviderEndpointEditorState>,
    new_provider_name: String,
}

#[derive(Debug, Default, Clone)]
struct ConfigProviderEndpointEditorState {
    name: String,
    base_url: String,
    enabled: bool,
}

#[derive(Debug, Default)]
struct ConfigProfileEditorState {
    profile_name: Option<String>,
    station: Option<String>,
    model: String,
    reasoning_effort: String,
    service_tier: String,
}

#[derive(Debug, Clone)]
enum ConfigWorkingDocument {
    Legacy(crate::config::ProxyConfig),
    V2(crate::config::ProxyConfigV2),
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

#[derive(Debug, Default)]
pub struct SessionsViewState {
    pub active_only: bool,
    pub errors_only: bool,
    pub overrides_only: bool,
    pub lock_order: bool,
    pub search: String,
    pub default_profile_selection: Option<String>,
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
    profile_selection: Option<String>,
    model_override: String,
    config_override: Option<String>,
    effort_override: Option<String>,
    custom_effort: String,
    service_tier_override: Option<String>,
    custom_service_tier: String,
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
            (Page::Stations, pick(lang, "站点", "Stations")),
            (Page::Doctor, pick(lang, "诊断", "Doctor")),
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
        Page::Stations => render_stations(ui, ctx),
        Page::Doctor => render_doctor(ui, ctx),
        Page::Config => render_config(ui, ctx),
        Page::Sessions => render_sessions(ui, ctx),
        Page::Requests => render_requests(ui, ctx),
        Page::Stats => render_stats(ui, ctx),
        Page::History => history::render_history(ui, ctx),
        Page::Settings => render_settings(ui, ctx),
    }
}

pub(super) fn remote_attached_proxy_active(proxy: &super::proxy_control::ProxyController) -> bool {
    matches!(proxy.kind(), super::proxy_control::ProxyModeKind::Attached)
        && !host_local_session_features_available(proxy)
}

fn attached_host_local_session_features_available(
    admin_base_url: &str,
    host_local_session_history: bool,
) -> bool {
    management_base_url_is_loopback(admin_base_url) && host_local_session_history
}

fn format_host_local_capability_summary(
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
) -> Option<String> {
    let mut parts = Vec::new();
    if caps.session_history {
        parts.push(pick(lang, "会话历史", "session history"));
    }
    if caps.transcript_read {
        parts.push(pick(lang, "对话读取", "transcript read"));
    }
    if caps.cwd_enrichment {
        parts.push(pick(lang, "cwd 补全", "cwd enrichment"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

pub(super) fn remote_local_only_warning_message(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
    requested_features: &[&str],
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    let requested = if requested_features.is_empty() {
        pick(lang, "这些功能", "these features").to_string()
    } else {
        requested_features.join(" / ")
    };

    match (lang, format_host_local_capability_summary(caps, lang)) {
        (Language::Zh, Some(summary)) => Some(format!(
            "当前是远端附着：{requested} 属于 host-local 功能，这台设备不能直接访问。附着目标声明这些能力只在代理主机本地可用：{summary}；如需使用，请在代理主机上运行 codex-helper GUI。"
        )),
        (Language::Zh, None) => Some(format!(
            "当前是远端附着：{requested} 属于 host-local 功能，这台设备不能直接访问。附着目标也没有声明可供主机本地使用的 session/transcript/cwd 能力；如需使用，请切回本机代理或在代理主机上运行 codex-helper GUI。"
        )),
        (Language::En, Some(summary)) => Some(format!(
            "This is a remote attach: {requested} are host-local features and are not directly available from this device. The attached target reports these as host-only capabilities on the proxy machine: {summary}. Run codex-helper GUI on the proxy host to use them."
        )),
        (Language::En, None) => Some(format!(
            "This is a remote attach: {requested} are host-local features and are not directly available from this device. The attached target does not advertise host-local session/transcript/cwd capabilities either. Use a local proxy on this device or run codex-helper GUI on the proxy host."
        )),
    }
}

fn remote_admin_token_present() -> bool {
    std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn remote_admin_access_short_label(
    admin_base_url: &str,
    caps: &RemoteAdminAccessCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }
    if !caps.remote_enabled {
        return Some(
            pick(lang, "远端 admin 未开放", "Remote admin locked to loopback").to_string(),
        );
    }
    if !remote_admin_token_present() {
        return Some(
            pick(
                lang,
                "远端 admin 需 token（本机未设置）",
                "Remote admin needs token (client missing)",
            )
            .to_string(),
        );
    }
    Some(pick(lang, "远端 admin 已启用 token", "Remote admin token ready").to_string())
}

fn remote_admin_access_message(
    admin_base_url: &str,
    caps: &RemoteAdminAccessCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    if !caps.remote_enabled {
        return Some(match lang {
            Language::Zh => format!(
                "当前目标的 admin 控制面仍是 loopback-only。要允许 LAN/Tailscale 设备附着，请在代理主机设置环境变量 {}，客户端随后需通过请求头 {} 发送相同 token。",
                caps.token_env_var, caps.token_header
            ),
            Language::En => format!(
                "This target keeps its admin control plane loopback-only. To allow LAN/Tailscale attach, set {} on the proxy host, then clients must send the same token via header {}.",
                caps.token_env_var, caps.token_header
            ),
        });
    }

    if !remote_admin_token_present() {
        return Some(match lang {
            Language::Zh => format!(
                "目标已开放远端 admin，但当前 GUI 进程未设置 {}。若继续远端附着，admin 请求会被拒绝；请在当前设备设置该环境变量，并让其值与代理主机一致，请求头名为 {}。",
                caps.token_env_var, caps.token_header
            ),
            Language::En => format!(
                "The target allows remote admin, but this GUI process has no {} set. Remote attach admin requests will be rejected until this device provides the same token; the required header name is {}.",
                caps.token_env_var, caps.token_header
            ),
        });
    }

    Some(match lang {
        Language::Zh => format!(
            "当前远端 admin 将通过环境变量 {} 注入，并以请求头 {} 发送。请确保客户端与代理主机使用相同 token。",
            caps.token_env_var, caps.token_header
        ),
        Language::En => format!(
            "Remote admin will use the token from env {} and send it via header {}. Ensure the client and proxy host use the same token value.",
            caps.token_env_var, caps.token_header
        ),
    })
}

fn merge_info_message<I>(base: String, extras: I) -> String
where
    I: IntoIterator<Item = String>,
{
    let extras = extras
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if extras.is_empty() {
        base
    } else {
        format!("{base} {}", extras.join(" "))
    }
}

pub(super) fn host_local_session_features_available(
    proxy: &super::proxy_control::ProxyController,
) -> bool {
    match proxy.kind() {
        super::proxy_control::ProxyModeKind::Attached => proxy.attached().is_some_and(|attached| {
            attached_host_local_session_features_available(
                attached.admin_base_url.as_str(),
                attached.host_local_capabilities.session_history,
            )
        }),
        _ => true,
    }
}

fn management_base_url_is_loopback(base_url: &str) -> bool {
    let input = base_url.trim();
    if input.is_empty() {
        return false;
    }

    let after_scheme = input
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(input);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if authority.is_empty() {
        return false;
    }

    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host).unwrap_or(rest)
    } else if let Some((host, _)) = authority.rsplit_once(':') {
        host
    } else {
        authority
    };

    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

fn render_setup(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "快速设置", "Setup"));
    ui.label(pick(
        ctx.lang,
        "目标：让 Codex/Claude 走本地 codex-helper 代理（常驻后台），并完成基础配置。",
        "Goal: route Codex/Claude through the local codex-helper proxy (resident) and complete basic setup.",
    ));
    ui.label(pick(
        ctx.lang,
        "推荐顺序：先 1) 配置，再 2) 启动/附着代理，最后 3) 切换客户端。如果你已在 TUI 启动代理，请在第 2 步使用“扫描并附着”。",
        "Recommended order: 1) config, 2) start/attach proxy, 3) switch client. If you already started the proxy in TUI, use “Scan & attach” in step 2.",
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
                && let Err(e) = open_in_file_manager(&cfg_path, true)
            {
                *ctx.last_error = Some(format!("open config failed: {e}"));
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

    let mut action_scan_local_proxies = false;
    let mut action_attach_discovered: Option<DiscoveredProxy> = None;

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

        ui.add_space(8.0);
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "已运行代理？（例如：你已在 TUI 中启动）",
            "Already running? (e.g. started from TUI)",
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
                .id_salt("setup_discovered_proxies_scroll")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("setup_discovered_proxies_grid")
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
                                ui.vertical(|ui| {
                                    if let Some(err) = p.last_error.as_deref() {
                                        ui.label(err);
                                    } else {
                                        ui.label(pick(ctx.lang, "可用", "OK"));
                                    }
                                    if let Some(warning) = remote_local_only_warning_message(
                                        p.admin_base_url.as_str(),
                                        &p.host_local_capabilities,
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
                                        p.admin_base_url.as_str(),
                                        &p.remote_admin_access,
                                        ctx.lang,
                                    ) {
                                        let color = if p.remote_admin_access.remote_enabled
                                            && remote_admin_token_present()
                                        {
                                            egui::Color32::from_rgb(60, 160, 90)
                                        } else {
                                            egui::Color32::from_rgb(200, 120, 40)
                                        };
                                        let response = ui.colored_label(color, label);
                                        if let Some(message) = remote_admin_access_message(
                                            p.admin_base_url.as_str(),
                                            &p.remote_admin_access,
                                            ctx.lang,
                                        ) {
                                            response.on_hover_text(message);
                                        }
                                    }
                                });

                                if ui.button(pick(ctx.lang, "附着", "Attach")).clicked() {
                                    action_attach_discovered = Some(p.clone());
                                }
                                ui.end_row();
                            }
                        });
                });
        }
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
                            if !st.has_backup {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 120, 40),
                                    pick(
                                        ctx.lang,
                                        "提示：当前已指向本地代理但未找到备份文件；请勿重复 switch on，否则备份可能覆盖原始配置。",
                                        "Tip: enabled but no backup found; avoid repeated switch on (backup may not represent the original config).",
                                    ),
                                );
                            }
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
                            if !st.has_backup {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 120, 40),
                                    pick(
                                        ctx.lang,
                                        "提示：当前已指向本地代理但未找到备份文件；请勿重复 switch on，否则备份可能覆盖原始配置。",
                                        "Tip: enabled but no backup found; avoid repeated switch on (backup may not represent the original config).",
                                    ),
                                );
                            }
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

    if action_scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found").to_string());
        } else {
            *ctx.last_info = Some(
                pick(
                    ctx.lang,
                    "扫描完成：请选择一个代理进行附着",
                    "Scan done: pick a proxy to attach",
                )
                .to_string(),
            );
        }
    }

    if let Some(proxy) = action_attach_discovered {
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
        ctx.proxy.set_desired_port(proxy.port);
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        ctx.gui_cfg.proxy.default_port = proxy.port;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                pick(ctx.lang, "已附着到代理。", "Attached.").to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
        }
    }
}

fn render_doctor(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "诊断", "Doctor"));
    ui.label(pick(
        ctx.lang,
        "用于排查：配置是否可读、env 是否缺失、Codex CLI 配置/认证文件是否存在、自动导入链路是否可用、日志与用量提供商配置是否正常。",
        "Helps diagnose: config readability, missing env vars, Codex CLI config/auth presence, auto-import viability, logs and usage providers.",
    ));
    ui.separator();

    let lang = match ctx.lang {
        Language::En => DoctorLang::En,
        _ => DoctorLang::Zh,
    };

    ui.horizontal(|ui| {
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.view.doctor.report = None;
            ctx.view.doctor.last_error = None;
            ctx.view.doctor.loaded_at_ms = None;
        }

        if ui
            .button(pick(ctx.lang, "复制 JSON", "Copy JSON"))
            .clicked()
        {
            if let Some(r) = ctx.view.doctor.report.as_ref() {
                let text = serde_json::to_string_pretty(r)
                    .unwrap_or_else(|_| "{\"checks\":[]}".to_string());
                ui.ctx().copy_text(text);
                *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
            } else {
                *ctx.last_error =
                    Some(pick(ctx.lang, "尚未加载报告", "Report not loaded").to_string());
            }
        }

        if ui
            .button(pick(ctx.lang, "打开配置文件", "Open config file"))
            .clicked()
        {
            let path = crate::config::config_file_path();
            if let Err(e) = open_in_file_manager(&path, true) {
                *ctx.last_error = Some(format!("open config failed: {e}"));
            }
        }

        if ui
            .button(pick(ctx.lang, "打开日志目录", "Open logs folder"))
            .clicked()
        {
            let dir = crate::config::proxy_home_dir().join("logs");
            if let Err(e) = open_in_file_manager(&dir, false) {
                *ctx.last_error = Some(format!("open logs failed: {e}"));
            }
        }
    });

    if ctx.view.doctor.report.is_none() && ctx.view.doctor.last_error.is_none() {
        let report = ctx.rt.block_on(crate::doctor::run_doctor(lang));
        ctx.view.doctor.loaded_at_ms = Some(now_ms());
        ctx.view.doctor.report = Some(report);
    }

    let Some(report) = ctx.view.doctor.report.as_ref() else {
        if let Some(err) = ctx.view.doctor.last_error.as_deref() {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        } else {
            ui.label(pick(ctx.lang, "暂无报告", "No report"));
        }
        return;
    };

    fn status_color(st: DoctorStatus) -> egui::Color32 {
        match st {
            DoctorStatus::Ok => egui::Color32::from_rgb(60, 160, 90),
            DoctorStatus::Info => egui::Color32::from_rgb(80, 160, 200),
            DoctorStatus::Warn => egui::Color32::from_rgb(200, 120, 40),
            DoctorStatus::Fail => egui::Color32::from_rgb(200, 60, 60),
        }
    }

    let mut ok = 0usize;
    let mut info = 0usize;
    let mut warn = 0usize;
    let mut fail = 0usize;
    for c in &report.checks {
        match c.status {
            DoctorStatus::Ok => ok += 1,
            DoctorStatus::Info => info += 1,
            DoctorStatus::Warn => warn += 1,
            DoctorStatus::Fail => fail += 1,
        }
    }

    ui.label(format!(
        "{}: OK {ok} | INFO {info} | WARN {warn} | FAIL {fail}",
        pick(ctx.lang, "汇总", "Summary")
    ));
    if let Some(ts) = ctx.view.doctor.loaded_at_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "加载时间(ms)", "Loaded at (ms)"),
            ts
        ));
    }

    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("doctor_report_scroll")
        .show(ui, |ui| {
            for c in &report.checks {
                ui.horizontal(|ui| {
                    let label = match c.status {
                        DoctorStatus::Ok => "OK",
                        DoctorStatus::Info => "INFO",
                        DoctorStatus::Warn => "WARN",
                        DoctorStatus::Fail => "FAIL",
                    };
                    ui.colored_label(status_color(c.status), label);
                    ui.label(c.id);
                });
                ui.label(&c.message);
                ui.separator();
            }
        });
}

fn render_overview(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "总览", "Overview"));

    ui.separator();

    let mut action_scan_local_proxies = false;
    let mut action_attach_discovered: Option<DiscoveredProxy> = None;

    // Sync defaults from GUI config (so Settings changes take effect without restart).
    // Avoid overriding the UI state while running/attached.
    if matches!(ctx.proxy.kind(), ProxyModeKind::Stopped) {
        ctx.proxy
            .set_defaults(ctx.gui_cfg.proxy.default_port, ctx.gui_cfg.service_kind());
    }

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "连接与路由", "Connection & routing"));

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

        if let Some(s) = ctx.proxy.snapshot() {
            if let Some(base) = s.base_url.as_deref() {
                ui.label(format!("{}: {base}", pick(ctx.lang, "地址", "Base URL")));
            }
            if let Some(svc) = s.service_name.as_deref() {
                ui.label(format!("{}: {svc}", pick(ctx.lang, "服务", "Service")));
            }
            if let Some(port) = s.port {
                ui.label(format!("{}: {port}", pick(ctx.lang, "端口", "Port")));
            }
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "API", "API"),
                if s.supports_v1 { "v1" } else { "legacy" }
            ));
        }

        let can_edit = matches!(kind, ProxyModeKind::Stopped);
        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "服务", "Service"));
            ui.add_enabled_ui(can_edit, |ui| {
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
            });

            ui.add_space(12.0);
            ui.label(pick(ctx.lang, "端口", "Port"));
            ui.add_enabled_ui(can_edit, |ui| {
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

            if !can_edit {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(ctx.lang, "（停止后可修改）", "(stop to edit)"),
                );
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            match kind {
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
                ProxyModeKind::Running => {
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
                    if ui.button(pick(ctx.lang, "取消附着", "Detach")).clicked() {
                        ctx.proxy.clear_port_in_use_modal();
                        ctx.proxy.detach();
                        *ctx.last_info = Some(pick(ctx.lang, "已取消附着", "Detached").to_string());
                    }
                }
                ProxyModeKind::Starting => {
                    ui.spinner();
                }
            }

            if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
                && ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked()
            {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
            }

            if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
                && ui
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

        ui.add_space(6.0);
        ui.collapsing(
            pick(
                ctx.lang,
                "附着到已运行的代理",
                "Attach to an existing proxy",
            ),
            |ui| {
                if !matches!(kind, ProxyModeKind::Stopped) {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        pick(
                            ctx.lang,
                            "提示：请先停止/取消附着，再切换到其他代理。",
                            "Tip: stop/detach first before switching to another proxy.",
                        ),
                    );
                }

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

                    if ui
                        .add_enabled(
                            matches!(kind, ProxyModeKind::Stopped),
                            egui::Button::new(pick(ctx.lang, "附着", "Attach")),
                        )
                        .clicked()
                    {
                        ctx.proxy.request_attach(attach_port);
                        ctx.gui_cfg.attach.last_port = Some(attach_port);
                        if let Err(e) = ctx.gui_cfg.save() {
                            *ctx.last_error = Some(format!("save gui config failed: {e}"));
                        } else {
                            *ctx.last_info =
                                Some(pick(ctx.lang, "正在附着…", "Attaching...").into());
                        }
                    }
                });

                let discovered = ctx.proxy.discovered_proxies().to_vec();
                if discovered.is_empty() {
                    ui.label(pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"));
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("overview_discovered_proxies_scroll")
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

                                    let now = now_ms();
                                    for p in discovered {
                                        let mut hover = format!("base_url: {}", p.base_url);
                                        if !p.endpoints.is_empty() {
                                            hover.push_str(&format!(
                                                "\nendpoints: {}",
                                                p.endpoints.len()
                                            ));
                                        }
                                        if let Some(ms) = p.runtime_loaded_at_ms {
                                            hover.push_str(&format!(
                                                "\nruntime_loaded: {}",
                                                format_age(now, Some(ms))
                                            ));
                                        }
                                        ui.label(p.port.to_string()).on_hover_text(hover);
                                        ui.label(
                                            p.service_name.as_deref().unwrap_or_else(|| {
                                                pick(ctx.lang, "未知", "unknown")
                                            }),
                                        );
                                        ui.label(match p.api_version {
                                            Some(v) => format!("v{v}"),
                                            None => "-".to_string(),
                                        });
                                        ui.vertical(|ui| {
                                            if let Some(err) = p.last_error.as_deref() {
                                                ui.label(err);
                                            } else {
                                                ui.label(pick(ctx.lang, "可用", "OK"));
                                            }
                                            if let Some(warning) = remote_local_only_warning_message(
                                                p.admin_base_url.as_str(),
                                                &p.host_local_capabilities,
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
                                                p.admin_base_url.as_str(),
                                                &p.remote_admin_access,
                                                ctx.lang,
                                            ) {
                                                let color = if p.remote_admin_access.remote_enabled
                                                    && remote_admin_token_present()
                                                {
                                                    egui::Color32::from_rgb(60, 160, 90)
                                                } else {
                                                    egui::Color32::from_rgb(200, 120, 40)
                                                };
                                                let response = ui.colored_label(color, label);
                                                if let Some(message) = remote_admin_access_message(
                                                    p.admin_base_url.as_str(),
                                                    &p.remote_admin_access,
                                                    ctx.lang,
                                                ) {
                                                    response.on_hover_text(message);
                                                }
                                            }
                                        });

                                        if ui
                                            .add_enabled(
                                                matches!(kind, ProxyModeKind::Stopped),
                                                egui::Button::new(pick(ctx.lang, "附着", "Attach")),
                                            )
                                            .clicked()
                                        {
                                            action_attach_discovered = Some(p.clone());
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                }
            },
        );

        ui.add_space(8.0);
        ui.separator();
        render_profile_management_entrypoint(ui, ctx);

        render_overview_station_summary(ui, ctx);
    });

    match ctx.proxy.kind() {
        ProxyModeKind::Stopped => {
            ui.add_space(8.0);
            ui.label(pick(
                ctx.lang,
                "提示：可在上方“连接与路由”面板启动或附着到代理。",
                "Tip: use the panel above to start or attach to a proxy.",
            ));
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
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
                    r.global_override
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
                ));

                let active_name = match r.service_name {
                    "claude" => r.cfg.claude.active.clone(),
                    _ => r.cfg.codex.active.clone(),
                };
                let active_fallback = match r.service_name {
                    "claude" => r.cfg.claude.active_config().map(|c| c.name.clone()),
                    _ => r.cfg.codex.active_config().map(|c| c.name.clone()),
                };
                let active_display = active_name
                    .clone()
                    .or(active_fallback.clone())
                    .unwrap_or_else(|| "-".to_string());
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "当前站点(active)", "Active station"),
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
                    crate::config::model_routing_warnings(r.cfg.as_ref(), r.service_name);
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
                            for w in warnings {
                                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), w);
                            }
                        });
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
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
                    att.global_override
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
                ));
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "提示：附着模式下不会改你的本机配置文件，但如果远端代理支持 API v1 扩展，上方的运行时控制仍可直接作用于该代理进程。",
                        "Tip: attached mode won't change your local config file, but runtime controls above can still act on the remote proxy process when supported.",
                    ),
                );
                if let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
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
                if let Some(label) = remote_admin_access_short_label(
                    att.admin_base_url.as_str(),
                    &att.remote_admin_access,
                    ctx.lang,
                ) {
                    let color =
                        if att.remote_admin_access.remote_enabled && remote_admin_token_present() {
                            egui::Color32::from_rgb(60, 160, 90)
                        } else {
                            egui::Color32::from_rgb(200, 120, 40)
                        };
                    let response = ui.colored_label(color, label);
                    let remote_admin_message = remote_admin_access_message(
                        att.admin_base_url.as_str(),
                        &att.remote_admin_access,
                        ctx.lang,
                    );
                    if let Some(message) = remote_admin_message.clone() {
                        response.on_hover_text(message.clone());
                        if !att.remote_admin_access.remote_enabled || !remote_admin_token_present()
                        {
                            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
                        }
                    }
                }
            }
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

    if let Some(proxy) = action_attach_discovered {
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
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                pick(ctx.lang, "正在附着…", "Attaching...").to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
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

fn render_overview_station_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };
    if snapshot.configs.is_empty() {
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let override_count = snapshot
        .configs
        .iter()
        .filter(|cfg| {
            cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some()
        })
        .count();
    let health_count = runtime_maps.config_health.len();
    let active_station = current_runtime_active_station(ctx.proxy);

    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "站点控制摘要", "Stations summary"));
    ui.horizontal(|ui| {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "站点数", "Stations"),
            snapshot.configs.len()
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "健康记录", "Health records"),
            health_count
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行时覆盖", "Runtime overrides"),
            override_count
        ));
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
    });
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
        snapshot
            .global_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前 active_station", "Current active_station"),
        active_station.as_deref().unwrap_or_else(|| pick(
            ctx.lang,
            "<未知/仅本机可见>",
            "<unknown/local-only>"
        ))
    ));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "更细的 quick switch、drain、breaker、健康检查已经移到单独的 Stations 页。",
            "Detailed quick switch, drain, breaker, and health controls now live in the dedicated Stations page.",
        ),
    );
}

fn render_stations(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "站点", "Stations"));
    ui.label(pick(
        ctx.lang,
        "面向 operator 的运行态站点面板：在这里集中查看站点能力、健康、熔断/冷却状态，并执行 quick switch 与运行时控制。",
        "Operator-focused runtime station panel: inspect station capabilities, health, breaker/cooldown state, and perform quick switch plus runtime control here.",
    ));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "当前没有运行中的本地代理，也没有附着到远端代理。请先在“总览”页启动或附着。",
            "No running or attached proxy is available. Start or attach one from Overview first.",
        ));
        if ui
            .button(pick(ctx.lang, "前往总览", "Go to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
        return;
    };

    if snapshot.configs.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "当前运行态没有可见站点。你可以先去“配置”页或原始配置文件里定义 station/provider。",
            "No stations are visible in the current runtime. Define stations/providers in Config first.",
        ));
        ui.horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "前往配置页", "Open Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
            if ui
                .button(pick(ctx.lang, "返回总览", "Back to Overview"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Overview);
            }
        });
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let active_station = current_runtime_active_station(ctx.proxy);
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot
        .effective_active_station
        .clone()
        .or(active_station.clone());
    let supports_persisted_station_config = snapshot.supports_persisted_station_config;
    let mut stations = snapshot.configs.clone();
    stations.sort_by(|a, b| {
        a.level
            .clamp(1, 10)
            .cmp(&b.level.clamp(1, 10))
            .then_with(|| a.name.cmp(&b.name))
    });

    let search_query = ctx.view.stations.search.trim().to_ascii_lowercase();
    let enabled_only = ctx.view.stations.enabled_only;
    let overrides_only = ctx.view.stations.overrides_only;
    let filtered = stations
        .into_iter()
        .filter(|cfg| {
            if enabled_only && !cfg.enabled {
                return false;
            }
            if overrides_only
                && cfg.runtime_enabled_override.is_none()
                && cfg.runtime_level_override.is_none()
                && cfg.runtime_state_override.is_none()
            {
                return false;
            }
            if search_query.is_empty() {
                return true;
            }
            let alias = cfg.alias.as_deref().unwrap_or("");
            let capability = format_runtime_config_capability_label(ctx.lang, &cfg.capabilities);
            let haystack = format!(
                "{} {} {} {}",
                cfg.name.to_ascii_lowercase(),
                alias.to_ascii_lowercase(),
                format_runtime_station_health_status(
                    runtime_maps.config_health.get(cfg.name.as_str()),
                    runtime_maps.health_checks.get(cfg.name.as_str())
                )
                .to_ascii_lowercase(),
                capability.to_ascii_lowercase(),
            );
            haystack.contains(search_query.as_str())
        })
        .collect::<Vec<_>>();

    if ctx
        .view
        .stations
        .selected_name
        .as_ref()
        .is_none_or(|name| !filtered.iter().any(|cfg| cfg.name == *name))
    {
        ctx.view.stations.selected_name = filtered.first().map(|cfg| cfg.name.clone());
    }
    let mut selected_name = ctx.view.stations.selected_name.clone();

    ui.add_space(8.0);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "模式", "Mode"),
                match snapshot.kind {
                    ProxyModeKind::Running => pick(ctx.lang, "本地运行", "Running"),
                    ProxyModeKind::Attached => pick(ctx.lang, "远端附着", "Attached"),
                    ProxyModeKind::Starting => pick(ctx.lang, "启动中", "Starting"),
                    ProxyModeKind::Stopped => pick(ctx.lang, "停止", "Stopped"),
                }
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "服务", "Service"),
                snapshot
                    .service_name
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "-", "-"))
            ));
            if let Some(base_url) = snapshot.base_url.as_deref() {
                ui.label(format!("base: {}", shorten_middle(base_url, 56)));
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
                snapshot
                    .global_override
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "配置 active_station", "Configured active_station"),
                configured_active_station
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "生效站点", "Effective station"),
                effective_active_station
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "<未知/仅本机可见>", "<unknown/local-only>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "配置 default_profile", "Configured default_profile"),
                snapshot
                    .configured_default_profile
                    .as_deref()
                    .or(snapshot.default_profile.as_deref())
                    .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
            ));
            if snapshot
                .configured_default_profile
                .as_deref()
                != snapshot.default_profile.as_deref()
            {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "生效 default_profile", "Effective default_profile"),
                    snapshot
                        .default_profile
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
                ));
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "持久化站点配置", "Persisted station config"),
                if supports_persisted_station_config {
                    pick(ctx.lang, "可用", "available")
                } else {
                    pick(ctx.lang, "不可用", "unavailable")
                }
            ));
            if matches!(snapshot.kind, ProxyModeKind::Attached) {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "远端写回", "Remote write-back"),
                    if supports_persisted_station_config {
                        pick(ctx.lang, "已启用", "enabled")
                    } else {
                        pick(ctx.lang, "未提供", "not exposed")
                    }
                ));
            }
        });
        ui.horizontal(|ui| {
            if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
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
                .button(pick(ctx.lang, "打开配置页", "Open Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
            if ui
                .button(pick(ctx.lang, "回到总览", "Back to Overview"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Overview);
            }
        });
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            if matches!(snapshot.kind, ProxyModeKind::Attached) {
                if supports_persisted_station_config {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；下面的“配置控制”也会直接写回远端代理配置，不会改动本机文件。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; the persisted config controls below also write back to the remote proxy rather than this device's local file.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；当前附着目标还没有暴露 persisted station config API，因此只能做运行时控制。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; this attached target does not expose persisted station config APIs yet, so only runtime controls are available.",
                    )
                }
            } else {
                pick(
                    ctx.lang,
                    "这里的 global pin 是运行时覆盖；“配置控制”会通过本地 control-plane 写回配置文件并刷新运行态。",
                    "Global pin here is runtime-only; the persisted config controls write through the local control plane and refresh the runtime.",
                )
            },
        );
        if matches!(snapshot.kind, ProxyModeKind::Attached)
            && let Some(base_url) = snapshot.base_url.as_deref()
            && let Some(label) = remote_admin_access_short_label(
                base_url,
                &snapshot.remote_admin_access,
                ctx.lang,
            )
        {
            let color = if snapshot.remote_admin_access.remote_enabled
                && remote_admin_token_present()
            {
                egui::Color32::from_rgb(60, 160, 90)
            } else {
                egui::Color32::from_rgb(200, 120, 40)
            };
            let response = ui.colored_label(color, label);
            if let Some(message) =
                remote_admin_access_message(base_url, &snapshot.remote_admin_access, ctx.lang)
            {
                response.on_hover_text(message.clone());
                if !snapshot.remote_admin_access.remote_enabled || !remote_admin_token_present() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
                }
            }
        }
    });

    ui.add_space(8.0);
    render_stations_retry_panel(ui, ctx, &snapshot);

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add_sized(
            [320.0, 20.0],
            egui::TextEdit::singleline(&mut ctx.view.stations.search).hint_text(pick(
                ctx.lang,
                "按 station / alias / health / capability 过滤…",
                "Filter by station / alias / health / capability...",
            )),
        );
        ui.checkbox(
            &mut ctx.view.stations.enabled_only,
            pick(ctx.lang, "仅启用", "Enabled only"),
        );
        ui.checkbox(
            &mut ctx.view.stations.overrides_only,
            pick(ctx.lang, "仅运行时覆盖", "Overrides only"),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.stations.search.clear();
            ctx.view.stations.enabled_only = false;
            ctx.view.stations.overrides_only = false;
        }
    });

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "站点列表", "Stations"));
        cols[0].add_space(4.0);
        if filtered.is_empty() {
            cols[0].label(pick(
                ctx.lang,
                "筛选后没有匹配站点。",
                "No stations matched the current filters.",
            ));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("stations_page_list_scroll")
                .max_height(560.0)
                .show(&mut cols[0], |ui| {
                    for cfg in filtered.iter() {
                        let is_selected = selected_name.as_deref() == Some(cfg.name.as_str());
                        let is_active = active_station.as_deref() == Some(cfg.name.as_str());
                        let is_pinned =
                            snapshot.global_override.as_deref() == Some(cfg.name.as_str());
                        let health_label = format_runtime_station_health_status(
                            runtime_maps.config_health.get(cfg.name.as_str()),
                            runtime_maps.health_checks.get(cfg.name.as_str()),
                        );
                        let breaker_label =
                            format_runtime_lb_summary(runtime_maps.lb_view.get(cfg.name.as_str()));

                        let mut label = format!("L{} {}", cfg.level.clamp(1, 10), cfg.name);
                        if let Some(alias) = cfg.alias.as_deref()
                            && !alias.trim().is_empty()
                        {
                            label.push_str(&format!(" ({alias})"));
                        }
                        if is_active {
                            label = format!("★ {label}");
                        } else if is_pinned {
                            label = format!("◆ {label}");
                        }
                        if !cfg.enabled {
                            label.push_str("  [off]");
                        }

                        let capability_hover =
                            runtime_config_capability_hover_text(ctx.lang, &cfg.capabilities);
                        let hover = format!(
                            "health: {health_label}\nbreaker: {breaker_label}\n{}\nsource: {}",
                            capability_hover,
                            format_runtime_config_source(ctx.lang, cfg)
                        );
                        if ui
                            .selectable_label(is_selected, label)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            selected_name = Some(cfg.name.clone());
                        }
                        ui.small(format!(
                            "{}  |  {}",
                            health_label,
                            format_runtime_config_capability_label(ctx.lang, &cfg.capabilities)
                        ));
                        ui.add_space(4.0);
                    }
                });
        }

        cols[1].heading(pick(ctx.lang, "站点详情", "Station details"));
        cols[1].add_space(4.0);

        let Some(name) = selected_name.clone() else {
            cols[1].label(pick(ctx.lang, "未选择站点。", "No station selected."));
            return;
        };
        let Some(cfg) = filtered.iter().find(|cfg| cfg.name == name).cloned() else {
            cols[1].label(pick(
                ctx.lang,
                "当前选中站点不在筛选结果中。",
                "The selected station is not visible under the current filters.",
            ));
            return;
        };

        let health = runtime_maps.config_health.get(cfg.name.as_str());
        let health_status = runtime_maps.health_checks.get(cfg.name.as_str());
        let lb = runtime_maps.lb_view.get(cfg.name.as_str());
        let referencing_profiles = snapshot
            .profiles
            .iter()
            .filter(|profile| profile.station.as_deref() == Some(cfg.name.as_str()))
            .map(|profile| format_profile_display(profile.name.as_str(), Some(profile)))
            .collect::<Vec<_>>();

        cols[1].label(format!("name: {}", cfg.name));
        cols[1].label(format!(
            "alias: {}",
            cfg.alias
                .as_deref()
                .unwrap_or_else(|| pick(ctx.lang, "-", "-"))
        ));
        cols[1].label(format!(
            "{}: {}",
            pick(ctx.lang, "路由角色", "Routing role"),
            if effective_active_station.as_deref() == Some(cfg.name.as_str()) {
                pick(ctx.lang, "当前 active_station", "current active_station")
            } else if snapshot.global_override.as_deref() == Some(cfg.name.as_str()) {
                pick(ctx.lang, "当前 global pin", "current global pin")
            } else {
                pick(ctx.lang, "普通候选", "normal candidate")
            }
        ));
        if configured_active_station.as_deref() == Some(cfg.name.as_str())
            && effective_active_station.as_deref() != Some(cfg.name.as_str())
        {
            cols[1].small(pick(
                ctx.lang,
                "该站点是配置 active_station，但当前生效路由已被 fallback / pin / runtime 状态改变。",
                "This station is the configured active_station, but the effective route currently differs because of fallback, pin, or runtime state.",
            ));
        }
        cols[1].label(format!(
            "enabled: {}  (configured: {})",
            cfg.enabled, cfg.configured_enabled
        ));
        cols[1].label(format!(
            "level: L{}  (configured: L{})",
            cfg.level.clamp(1, 10),
            cfg.configured_level.clamp(1, 10)
        ));
        cols[1].label(format!(
            "state: {}",
            runtime_config_state_label(ctx.lang, cfg.runtime_state)
        ));
        cols[1].label(format!(
            "source: {}",
            format_runtime_config_source(ctx.lang, &cfg)
        ));
        cols[1].label(format!(
            "health: {}",
            format_runtime_station_health_status(health, health_status)
        ));
        cols[1].label(format!("breaker: {}", format_runtime_lb_summary(lb)));
        cols[1].label(format!(
            "{}: {}",
            pick(ctx.lang, "Profiles", "Profiles"),
            if referencing_profiles.is_empty() {
                pick(ctx.lang, "<无>", "<none>").to_string()
            } else {
                referencing_profiles.join(", ")
            }
        ));
        cols[1]
            .small(format_runtime_config_capability_label(
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
            cols[1].small(format!("models: {}{suffix}", preview.join(", ")));
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(
            ctx.lang,
            "Quick switch（运行时）",
            "Quick switch (runtime)",
        ));
        cols[1].horizontal(|ui| {
            if ui
                .add_enabled(
                    snapshot.supports_v1,
                    egui::Button::new(pick(ctx.lang, "Pin 当前站点", "Pin selected station")),
                )
                .clicked()
            {
                match ctx
                    .proxy
                    .apply_global_config_override(ctx.rt, Some(cfg.name.clone()))
                {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已应用全局站点覆盖", "Global station pin applied")
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply global override failed: {e}"));
                    }
                }
            }
            if ui
                .add_enabled(
                    snapshot.supports_v1 && snapshot.global_override.is_some(),
                    egui::Button::new(pick(ctx.lang, "清除 global pin", "Clear global pin")),
                )
                .clicked()
            {
                match ctx.proxy.apply_global_config_override(ctx.rt, None) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已清除全局覆盖", "Global pin cleared").to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("clear global override failed: {e}"));
                    }
                }
            }
        });
        cols[1].small(pick(
            ctx.lang,
            "这里的 pin 只影响当前代理运行态，不修改配置文件。",
            "Pins here only affect the current proxy runtime and do not rewrite persisted config.",
        ));

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "配置控制", "Persisted config"));
        if supports_persisted_station_config {
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(
                        ctx.lang,
                        "设为配置 active_station",
                        "Set configured active_station",
                    ))
                    .clicked()
                {
                    match ctx
                        .proxy
                        .set_persisted_active_station(ctx.rt, Some(cfg.name.clone()))
                    {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            refresh_config_editor_from_disk_if_running(ctx);
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已更新配置 active_station",
                                    "Configured active_station updated",
                                )
                                .to_string(),
                            );
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            *ctx.last_error =
                                Some(format!("set persisted active station failed: {e}"));
                        }
                    }
                }
                if ui
                    .add_enabled(
                        configured_active_station.is_some(),
                        egui::Button::new(pick(
                            ctx.lang,
                            "清除配置 active_station",
                            "Clear configured active_station",
                        )),
                    )
                    .clicked()
                {
                    match ctx.proxy.set_persisted_active_station(ctx.rt, None) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            refresh_config_editor_from_disk_if_running(ctx);
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已清除配置 active_station",
                                    "Configured active_station cleared",
                                )
                                .to_string(),
                            );
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            *ctx.last_error =
                                Some(format!("clear persisted active station failed: {e}"));
                        }
                    }
                }
            });

            let mut persisted_enabled = cfg.configured_enabled;
            let mut persisted_level = cfg.configured_level.clamp(1, 10);
            cols[1].horizontal(|ui| {
                ui.checkbox(
                    &mut persisted_enabled,
                    pick(ctx.lang, "配置启用", "Configured enabled"),
                );
                ui.label(pick(ctx.lang, "配置等级", "Configured level"));
                egui::ComboBox::from_id_salt(("stations_persisted_level", cfg.name.as_str()))
                    .selected_text(persisted_level.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in 1u8..=10 {
                            ui.selectable_value(
                                &mut persisted_level,
                                candidate,
                                candidate.to_string(),
                            );
                        }
                    });
            });
            if persisted_enabled != cfg.configured_enabled
                || persisted_level != cfg.configured_level.clamp(1, 10)
            {
                match ctx.proxy.update_persisted_station(
                    ctx.rt,
                    cfg.name.clone(),
                    persisted_enabled,
                    persisted_level,
                ) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        refresh_config_editor_from_disk_if_running(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已写回站点配置字段",
                                "Persisted station fields updated",
                            )
                            .to_string(),
                        );
                        *ctx.last_error = None;
                    }
                    Err(e) => {
                        *ctx.last_error =
                            Some(format!("update persisted station fields failed: {e}"));
                    }
                }
            }
            cols[1].small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
                pick(
                    ctx.lang,
                    "这里直接写回附着代理的配置，不依赖本机文件。",
                    "These controls write back to the attached proxy's config directly and do not rely on this device's local file.",
                )
            } else {
                pick(
                    ctx.lang,
                    "这里通过本地 control-plane 写回配置文件，并与运行态保持同步。",
                    "These controls write back through the local control plane and keep runtime in sync.",
                )
            });
        } else {
            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(
                    ctx.lang,
                    "当前目标没有暴露 persisted station config API，因此这里只能查看配置态，不能直接修改。",
                    "This target does not expose persisted station config APIs yet, so persisted fields are view-only here.",
                ),
            );
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "运行时控制", "Runtime control"));
        if snapshot.supports_config_runtime_override {
            let mut runtime_state = cfg.runtime_state;
            cols[1].horizontal(|ui| {
                ui.label(pick(ctx.lang, "状态", "State"));
                egui::ComboBox::from_id_salt(("stations_runtime_state", cfg.name.as_str()))
                    .selected_text(runtime_config_state_label(ctx.lang, runtime_state))
                    .show_ui(ui, |ui| {
                        for candidate in [
                            RuntimeConfigState::Normal,
                            RuntimeConfigState::Draining,
                            RuntimeConfigState::BreakerOpen,
                        ] {
                            ui.selectable_value(
                                &mut runtime_state,
                                candidate,
                                runtime_config_state_label(ctx.lang, candidate),
                            );
                        }
                    });
                if runtime_state != cfg.runtime_state {
                    match ctx.proxy.set_runtime_config_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        None,
                        None,
                        Some(Some(runtime_state)),
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时状态",
                                    "Runtime station state updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime state failed: {e}"));
                        }
                    }
                }
            });

            cols[1].horizontal(|ui| {
                let mut enabled = cfg.enabled;
                if ui
                    .checkbox(&mut enabled, pick(ctx.lang, "启用", "Enabled"))
                    .changed()
                {
                    match ctx.proxy.set_runtime_config_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        Some(Some(enabled)),
                        None,
                        None,
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时开关",
                                    "Runtime station enabled updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime enabled failed: {e}"));
                        }
                    }
                }

                let mut level = cfg.level.clamp(1, 10);
                ui.label(pick(ctx.lang, "等级", "Level"));
                egui::ComboBox::from_id_salt(("stations_runtime_level", cfg.name.as_str()))
                    .selected_text(level.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in 1u8..=10 {
                            ui.selectable_value(&mut level, candidate, candidate.to_string());
                        }
                    });
                if level != cfg.level {
                    match ctx.proxy.set_runtime_config_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        None,
                        Some(Some(level)),
                        None,
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时等级",
                                    "Runtime station level updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime level failed: {e}"));
                        }
                    }
                }
            });

            let has_override = cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some();
            if cols[1]
                .add_enabled(
                    has_override,
                    egui::Button::new(pick(ctx.lang, "清除运行时覆盖", "Clear runtime override")),
                )
                .clicked()
            {
                match ctx.proxy.set_runtime_config_meta(
                    ctx.rt,
                    cfg.name.clone(),
                    cfg.runtime_enabled_override.map(|_| None),
                    cfg.runtime_level_override.map(|_| None),
                    cfg.runtime_state_override.map(|_| None),
                ) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已清除站点运行时覆盖",
                                "Runtime station override cleared",
                            )
                            .to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error =
                            Some(format!("clear runtime station override failed: {e}"));
                    }
                }
            }
        } else {
            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(
                    ctx.lang,
                    "当前代理不支持运行时站点控制；此区域只读。",
                    "This proxy does not support runtime station control; this area is read-only.",
                ),
            );
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "健康检查", "Health check"));
        if let Some(status) = health_status {
            cols[1].label(format!(
                "status: {}/{} ok={} err={} cancel={} done={}",
                status.completed,
                status.total,
                status.ok,
                status.err,
                status.cancel_requested,
                status.done
            ));
            if let Some(err) = status.last_error.as_deref() {
                cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }
        } else {
            cols[1].label(pick(ctx.lang, "(无状态)", "(no status)"));
        }
        cols[1].horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "检查当前", "Check selected"))
                .clicked()
            {
                match ctx
                    .proxy
                    .start_health_checks(ctx.rt, false, vec![cfg.name.clone()])
                {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已开始健康检查", "Health check started").to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check start failed: {e}"));
                    }
                }
            }
            if ui
                .button(pick(ctx.lang, "取消当前", "Cancel selected"))
                .clicked()
            {
                match ctx
                    .proxy
                    .cancel_health_checks(ctx.rt, false, vec![cfg.name.clone()])
                {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                    }
                }
            }
            if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
                match ctx.proxy.start_health_checks(ctx.rt, true, Vec::new()) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已开始健康检查", "Health check started").to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check start failed: {e}"));
                    }
                }
            }
            if ui
                .button(pick(ctx.lang, "取消全部", "Cancel all"))
                .clicked()
            {
                match ctx.proxy.cancel_health_checks(ctx.rt, true, Vec::new()) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                    }
                }
            }
        });

        if let Some(health) = health {
            cols[1].add_space(6.0);
            cols[1].label(format!(
                "{}: {}  upstreams={}",
                pick(ctx.lang, "最近检查", "Last checked"),
                health.checked_at_ms,
                health.upstreams.len()
            ));
            egui::ScrollArea::vertical()
                .id_salt(("stations_health_upstreams_scroll", cfg.name.as_str()))
                .max_height(140.0)
                .show(&mut cols[1], |ui| {
                    let max = 12usize;
                    for up in health.upstreams.iter().rev().take(max) {
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
                            shorten_middle(&up.base_url, 52)
                        ));
                    }
                    if health.upstreams.len() > max {
                        ui.label(format!("… +{} more", health.upstreams.len() - max));
                    }
                });
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "熔断/冷却细节", "Breaker/cooldown details"));
        if let Some(lb) = lb {
            if lb.upstreams.is_empty() {
                cols[1].label(pick(ctx.lang, "(无上游状态)", "(no upstream state)"));
            } else {
                egui::ScrollArea::vertical()
                    .id_salt(("stations_lb_scroll", cfg.name.as_str()))
                    .max_height(120.0)
                    .show(&mut cols[1], |ui| {
                        for (idx, upstream) in lb.upstreams.iter().enumerate() {
                            let cooldown = upstream
                                .cooldown_remaining_secs
                                .map(|secs| format!("{secs}s"))
                                .unwrap_or_else(|| "-".to_string());
                            ui.label(format!(
                                "#{} fail={} cooldown={} quota_exhausted={}",
                                idx, upstream.failure_count, cooldown, upstream.usage_exhausted
                            ));
                        }
                        if let Some(last_good_index) = lb.last_good_index {
                            ui.small(format!("last_good_index={last_good_index}"));
                        }
                    });
            }
        } else {
            cols[1].label(pick(ctx.lang, "(无熔断数据)", "(no breaker data)"));
        }
    });

    ctx.view.stations.selected_name = selected_name;
}

fn render_profile_management_entrypoint(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(ctx.lang, "控制 profiles", "Control profiles"));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "旧版 GUI routing preset 已停用。现在统一使用代理配置里的 [codex.profiles.*]；默认 profile 在“配置”页管理，单会话覆盖在“会话”页管理。",
            "Legacy GUI routing presets are retired. Use [codex.profiles.*] in proxy config instead; manage default profiles in Config and per-session overrides in Sessions.",
        ),
    );
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "前往配置页", "Open Config page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Config);
        }
        if ui
            .button(pick(ctx.lang, "前往会话页", "Open Sessions page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Sessions);
        }
    });
}

fn render_stations_retry_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) {
    if snapshot.supports_retry_config_api {
        let configured_retry = snapshot.configured_retry.clone().unwrap_or_default();
        sync_stations_retry_editor(&mut ctx.view.stations.retry_editor, &configured_retry);
    }

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "Retry / Failover", "Retry / Failover"));
        ui.label(pick(
            ctx.lang,
            "这里管理全局的 retry profile 与冷却/熔断惩罚；它影响整个代理的路由行为，不是单个 station 的局部设置。",
            "Manage the global retry profile plus cooldown/breaker penalties here; it affects whole-proxy routing behavior rather than a single station.",
        ));

        if snapshot.supports_retry_config_api {
            {
                let editor = &mut ctx.view.stations.retry_editor;
                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "Retry profile", "Retry profile"));
                    egui::ComboBox::from_id_salt("stations_retry_profile")
                        .selected_text(retry_profile_display_text(
                            ctx.lang,
                            retry_profile_name_from_value(editor.profile.as_str()),
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut editor.profile,
                                String::new(),
                                retry_profile_display_text(ctx.lang, None),
                            );
                            for profile in [
                                RetryProfileName::Balanced,
                                RetryProfileName::SameUpstream,
                                RetryProfileName::AggressiveFailover,
                                RetryProfileName::CostPrimary,
                            ] {
                                ui.selectable_value(
                                    &mut editor.profile,
                                    retry_profile_name_value(profile).to_string(),
                                    retry_profile_display_text(ctx.lang, Some(profile)),
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("cf challenge");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_challenge_cooldown_secs),
                    );
                    ui.label("cf timeout");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_timeout_cooldown_secs),
                    );
                    ui.label("transport");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.transport_cooldown_secs),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("backoff factor");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_factor),
                    );
                    ui.label("backoff max");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_max_secs),
                    );
                });
            }

            ui.horizontal(|ui| {
                if ui
                    .button(pick(
                        ctx.lang,
                        "写回 retry 配置",
                        "Apply persisted retry config",
                    ))
                    .clicked()
                {
                    let base_retry = snapshot.configured_retry.as_ref().cloned().unwrap_or_default();
                    match build_retry_config_from_editor(&ctx.view.stations.retry_editor, &base_retry)
                    {
                        Ok(retry) => match ctx.proxy.set_persisted_retry_config(ctx.rt, retry) {
                            Ok(()) => {
                                ctx.proxy.refresh_current_if_due(
                                    ctx.rt,
                                    std::time::Duration::from_secs(0),
                                );
                                refresh_config_editor_from_disk_if_running(ctx);
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已写回 retry/failover 配置",
                                        "Persisted retry/failover config updated",
                                    )
                                    .to_string(),
                                );
                                *ctx.last_error = None;
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("set persisted retry config failed: {e}"));
                            }
                        },
                        Err(e) => {
                            *ctx.last_error = Some(format!("invalid retry config: {e}"));
                        }
                    }
                }

                if ui
                    .button(pick(ctx.lang, "恢复 balanced 表单", "Reset form to balanced"))
                    .clicked()
                {
                    load_stations_retry_editor_fields(
                        &mut ctx.view.stations.retry_editor,
                        &RetryConfig::default(),
                    );
                }
            });

            ui.small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
                pick(
                    ctx.lang,
                    "附着模式下，这里直接写回远端代理暴露的 retry config API，不依赖本机文件。",
                    "In attached mode, this writes directly to the remote proxy's retry config API instead of any local file on this device.",
                )
            } else {
                pick(
                    ctx.lang,
                    "本地运行模式下，这里通过 control-plane 写回配置文件并触发 reload。",
                    "In local running mode, this writes through the control plane to persisted config and reloads the runtime.",
                )
            });
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                if matches!(snapshot.kind, ProxyModeKind::Attached) {
                    pick(
                        ctx.lang,
                        "当前附着目标没有暴露 retry config API，因此这里只能查看 resolved policy，不能直接写回。",
                        "This attached target does not expose retry config APIs, so only the resolved policy is visible here.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "当前运行态没有可写 retry config API；下面仅展示 resolved policy。",
                        "No writable retry config API is available for the current runtime; only the resolved policy is shown below.",
                    )
                },
            );
        }

        ui.add_space(6.0);
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "Resolved policy",
            "Resolved policy",
        ));
        if let Some(retry) = snapshot.resolved_retry.as_ref() {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "upstream: {} / attempts={}",
                    retry_strategy_label(retry.upstream.strategy),
                    retry.upstream.max_attempts
                ));
                ui.label(format!(
                    "provider: {} / attempts={}",
                    retry_strategy_label(retry.provider.strategy),
                    retry.provider.max_attempts
                ));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "cf challenge={}s",
                    retry.cloudflare_challenge_cooldown_secs
                ));
                ui.label(format!(
                    "cf timeout={}s",
                    retry.cloudflare_timeout_cooldown_secs
                ));
                ui.label(format!("transport={}s", retry.transport_cooldown_secs));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "backoff factor={}",
                    retry.cooldown_backoff_factor
                ));
                ui.label(format!(
                    "backoff max={}s",
                    retry.cooldown_backoff_max_secs
                ));
            });
            ui.small(format!(
                "upstream backoff={}..{} ms  provider backoff={}..{} ms",
                retry.upstream.backoff_ms,
                retry.upstream.backoff_max_ms,
                retry.provider.backoff_ms,
                retry.provider.backoff_max_ms
            ));
        } else {
            ui.label(pick(
                ctx.lang,
                "当前还没有可见的 resolved retry policy。",
                "No resolved retry policy is visible for the current runtime yet.",
            ));
        }
    });
}

fn sync_stations_retry_editor(editor: &mut StationsRetryEditorState, retry: &RetryConfig) {
    let signature = format!("{retry:?}");
    if editor.source_signature.as_deref() == Some(signature.as_str()) {
        return;
    }
    load_stations_retry_editor_fields(editor, retry);
    editor.source_signature = Some(signature);
}

fn load_stations_retry_editor_fields(editor: &mut StationsRetryEditorState, retry: &RetryConfig) {
    editor.profile = retry
        .profile
        .map(retry_profile_name_value)
        .unwrap_or_default()
        .to_string();
    editor.cloudflare_challenge_cooldown_secs =
        optional_u64_editor_value(retry.cloudflare_challenge_cooldown_secs);
    editor.cloudflare_timeout_cooldown_secs =
        optional_u64_editor_value(retry.cloudflare_timeout_cooldown_secs);
    editor.transport_cooldown_secs = optional_u64_editor_value(retry.transport_cooldown_secs);
    editor.cooldown_backoff_factor = optional_u64_editor_value(retry.cooldown_backoff_factor);
    editor.cooldown_backoff_max_secs = optional_u64_editor_value(retry.cooldown_backoff_max_secs);
}

fn optional_u64_editor_value(value: Option<u64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn build_retry_config_from_editor(
    editor: &StationsRetryEditorState,
    base: &RetryConfig,
) -> Result<RetryConfig, String> {
    let mut retry = base.clone();
    retry.profile = retry_profile_name_from_value(editor.profile.as_str());
    retry.cloudflare_challenge_cooldown_secs = parse_optional_u64_editor_value(
        "cloudflare_challenge_cooldown_secs",
        &editor.cloudflare_challenge_cooldown_secs,
    )?;
    retry.cloudflare_timeout_cooldown_secs = parse_optional_u64_editor_value(
        "cloudflare_timeout_cooldown_secs",
        &editor.cloudflare_timeout_cooldown_secs,
    )?;
    retry.transport_cooldown_secs = parse_optional_u64_editor_value(
        "transport_cooldown_secs",
        &editor.transport_cooldown_secs,
    )?;
    retry.cooldown_backoff_factor = parse_optional_u64_editor_value(
        "cooldown_backoff_factor",
        &editor.cooldown_backoff_factor,
    )?;
    retry.cooldown_backoff_max_secs = parse_optional_u64_editor_value(
        "cooldown_backoff_max_secs",
        &editor.cooldown_backoff_max_secs,
    )?;
    Ok(retry)
}

fn parse_optional_u64_editor_value(field: &str, raw: &str) -> Result<Option<u64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| format!("{field} must be a non-negative integer"))
}

fn retry_profile_name_value(profile: RetryProfileName) -> &'static str {
    match profile {
        RetryProfileName::Balanced => "balanced",
        RetryProfileName::SameUpstream => "same-upstream",
        RetryProfileName::AggressiveFailover => "aggressive-failover",
        RetryProfileName::CostPrimary => "cost-primary",
    }
}

fn retry_profile_name_from_value(value: &str) -> Option<RetryProfileName> {
    match value.trim() {
        "balanced" => Some(RetryProfileName::Balanced),
        "same-upstream" => Some(RetryProfileName::SameUpstream),
        "aggressive-failover" => Some(RetryProfileName::AggressiveFailover),
        "cost-primary" => Some(RetryProfileName::CostPrimary),
        _ => None,
    }
}

fn retry_profile_display_text(lang: Language, profile: Option<RetryProfileName>) -> String {
    match profile {
        None => pick(lang, "自动（默认 balanced）", "Auto (default balanced)").to_string(),
        Some(RetryProfileName::Balanced) => pick(lang, "balanced（均衡）", "balanced").to_string(),
        Some(RetryProfileName::SameUpstream) => {
            pick(lang, "same-upstream（优先同上游）", "same-upstream").to_string()
        }
        Some(RetryProfileName::AggressiveFailover) => pick(
            lang,
            "aggressive-failover（积极切换）",
            "aggressive-failover",
        )
        .to_string(),
        Some(RetryProfileName::CostPrimary) => {
            pick(lang, "cost-primary（成本优先）", "cost-primary").to_string()
        }
    }
}

fn retry_strategy_label(strategy: RetryStrategy) -> &'static str {
    match strategy {
        RetryStrategy::Failover => "failover",
        RetryStrategy::SameUpstream => "same_upstream",
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
    let host_local_session_features = host_local_session_features_available(ctx.proxy);

    let last_error = snapshot.last_error.clone();
    let active = snapshot.active.clone();
    let recent = snapshot.recent.clone();
    let global_override = snapshot.global_override.clone();
    let default_profile = snapshot.default_profile.clone();
    let profiles = snapshot.profiles.clone();
    let session_model_overrides = snapshot.session_model_overrides.clone();
    let session_effort_overrides = snapshot.session_effort_overrides.clone();
    let session_config_overrides = snapshot.session_config_overrides.clone();
    let session_service_tier_overrides = snapshot.session_service_tier_overrides.clone();
    let session_stats = snapshot.session_stats.clone();
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot.effective_active_station.clone();
    let mut force_refresh = false;
    let runtime_station_catalog = snapshot
        .configs
        .iter()
        .cloned()
        .map(|config| (config.name.clone(), config))
        .collect::<BTreeMap<_, _>>();
    let session_preview_service_name =
        snapshot
            .service_name
            .as_deref()
            .unwrap_or(match ctx.view.config.service {
                crate::config::ServiceKind::Claude => "claude",
                crate::config::ServiceKind::Codex => "codex",
            });
    let session_preview_catalogs = ctx
        .proxy
        .attached()
        .and_then(|att| {
            att.supports_station_spec_api.then(|| {
                (
                    att.persisted_stations.clone(),
                    att.persisted_station_providers.clone(),
                )
            })
        })
        .or_else(|| {
            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
                None
            } else {
                local_profile_preview_catalogs_from_text(
                    ctx.proxy_config_text,
                    session_preview_service_name,
                )
            }
        });
    let session_preview_station_specs = session_preview_catalogs
        .as_ref()
        .map(|(stations, _)| stations);
    let session_preview_provider_catalog = session_preview_catalogs
        .as_ref()
        .map(|(_, providers)| providers);
    let session_preview_runtime_station_catalog = Some(&runtime_station_catalog);

    if ctx
        .view
        .sessions
        .default_profile_selection
        .as_ref()
        .is_none_or(|name| !profiles.iter().any(|profile| profile.name == *name))
    {
        ctx.view.sessions.default_profile_selection = default_profile
            .clone()
            .or_else(|| profiles.first().map(|profile| profile.name.clone()));
    }

    if let Some(err) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    if remote_attached_proxy_active(ctx.proxy) {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(
                ctx.lang,
                "当前附着的是远端代理：共享的 session 控制仍可用，但 cwd / transcript 这类 host-local 入口已按远端模式收敛。",
                "A remote proxy is attached: shared session controls remain available, but host-local entries such as cwd/transcript are gated for remote safety.",
            ),
        );
        ui.add_space(4.0);
    }

    if !profiles.is_empty() {
        let current_default_label = match default_profile.as_deref() {
            Some(name) => {
                format_profile_display(name, profiles.iter().find(|profile| profile.name == name))
            }
            None => pick(ctx.lang, "<无>", "<none>").to_string(),
        };

        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(pick(ctx.lang, "新会话默认 profile", "New-session default"));
                ui.monospace(current_default_label);

                let mut selected_default = ctx.view.sessions.default_profile_selection.clone();
                egui::ComboBox::from_id_salt("sessions_default_profile")
                    .selected_text(match selected_default.as_deref() {
                        Some(name) => format_profile_display(
                            name,
                            profiles.iter().find(|profile| profile.name == name),
                        ),
                        None => pick(ctx.lang, "<选择>", "<select>").to_string(),
                    })
                    .show_ui(ui, |ui| {
                        for profile in profiles.iter() {
                            ui.selectable_value(
                                &mut selected_default,
                                Some(profile.name.clone()),
                                format_profile_display(profile.name.as_str(), Some(profile)),
                            );
                        }
                    });
                if selected_default != ctx.view.sessions.default_profile_selection {
                    ctx.view.sessions.default_profile_selection = selected_default;
                }

                if ui
                    .button(pick(ctx.lang, "设为默认", "Set default"))
                    .clicked()
                {
                    if !snapshot.supports_default_profile_override {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "当前代理不支持运行时切换默认 profile。",
                                "Current proxy does not support runtime default profile switch.",
                            )
                            .to_string(),
                        );
                    } else if let Some(profile_name) =
                        ctx.view.sessions.default_profile_selection.clone()
                    {
                        match ctx
                            .proxy
                            .set_default_profile(ctx.rt, Some(profile_name.clone()))
                        {
                            Ok(()) => {
                                force_refresh = true;
                                ctx.view.sessions.default_profile_selection = Some(profile_name);
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已切换新会话默认 profile",
                                        "Default profile switched",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => {
                                *ctx.last_error = Some(format!("set default profile failed: {e}"));
                            }
                        }
                    } else {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "请先选择一个 profile。",
                                "Select a profile first.",
                            )
                            .to_string(),
                        );
                    }
                }

                if ui
                    .button(pick(ctx.lang, "回到配置默认", "Use config default"))
                    .clicked()
                {
                    if !snapshot.supports_default_profile_override {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "当前代理不支持运行时切换默认 profile。",
                                "Current proxy does not support runtime default profile switch.",
                            )
                            .to_string(),
                        );
                    } else {
                        match ctx.proxy.set_default_profile(ctx.rt, None) {
                            Ok(()) => {
                                force_refresh = true;
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已恢复配置文件默认 profile",
                                        "Fell back to config default profile",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("clear default profile failed: {e}"));
                            }
                        }
                    }
                }
            });

            ui.small(pick(
                ctx.lang,
                "只影响新的 session；已经建立 binding 的会话会保持当前绑定。",
                "Only affects new sessions; already bound sessions keep their current binding.",
            ));
        });

        ui.add_space(6.0);
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
                "按 session_id / cwd / model / station / config 过滤…",
                "Filter by session_id / cwd / model / station / config...",
            )),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.sessions.search.clear();
        }
    });

    ui.add_space(6.0);

    let has_session_cards = !snapshot.session_cards.is_empty();
    let rows = if has_session_cards {
        build_session_rows_from_cards(&snapshot.session_cards)
    } else {
        build_session_rows(
            active,
            &recent,
            &session_model_overrides,
            &session_effort_overrides,
            &session_config_overrides,
            &session_service_tier_overrides,
            global_override.as_deref(),
            &session_stats,
        )
    };

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
                && row.override_model.is_none()
                && row.override_effort.is_none()
                && row.override_config_name.is_none()
                && row.override_service_tier.is_none()
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
        ctx.view.sessions.editor.profile_selection = selected
            .and_then(|row| row.binding_profile_name.clone())
            .filter(|name| profiles.iter().any(|profile| profile.name == *name))
            .or_else(|| default_profile.clone())
            .or_else(|| profiles.first().map(|profile| profile.name.clone()));
        ctx.view.sessions.editor.model_override = selected
            .and_then(|r| r.override_model.clone())
            .unwrap_or_default();
        ctx.view.sessions.editor.config_override =
            selected.and_then(|r| r.override_config_name.clone());
        ctx.view.sessions.editor.effort_override = selected.and_then(|r| r.override_effort.clone());
        ctx.view.sessions.editor.custom_effort = selected
            .and_then(|r| r.override_effort.clone())
            .unwrap_or_default();
        ctx.view.sessions.editor.service_tier_override =
            selected.and_then(|r| r.override_service_tier.clone());
        ctx.view.sessions.editor.custom_service_tier = selected
            .and_then(|r| r.override_service_tier.clone())
            .unwrap_or_default();
    }

    let mut action_apply_session_profile: Option<(String, String)> = None;

    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "列表", "List"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("sessions_list_scroll")
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
                    let client = format_observed_client_identity(
                        row.last_client_name.as_deref(),
                        row.last_client_addr.as_deref(),
                    )
                    .map(|value| shorten(value.as_str(), 18))
                    .unwrap_or_else(|| "-".to_string());
                    let scope = session_observation_scope_short_label(ctx.lang, row.observation_scope);
                    let label = format!(
                        "{sid}  {cwd}  {active}  st={st}  last={last}  pin={pin}  src={client}  {scope}"
                    );
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
        let client_full = format_observed_client_identity(
            row.last_client_name.as_deref(),
            row.last_client_addr.as_deref(),
        )
        .unwrap_or_else(|| "-".to_string());
        let observation_scope = session_observation_scope_label(ctx.lang, row.observation_scope);
        let cwd_full = row.cwd.as_deref().unwrap_or("-");
        let provider = row.last_provider_id.as_deref().unwrap_or("-");
        let observed_model = row.last_model.as_deref().unwrap_or("-");
        let observed_cfg = row.last_config_name.as_deref().unwrap_or("-");
        let observed_upstream = row.last_upstream_base_url.as_deref().unwrap_or("-");
        let observed_effort = row.last_reasoning_effort.as_deref().unwrap_or("-");
        let observed_service_tier = row.last_service_tier.as_deref().unwrap_or("-");
        let binding_profile = row.binding_profile_name.as_deref().unwrap_or("-");
        let binding_mode = row
            .binding_continuity_mode
            .map(|mode| format!("{mode:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "-".to_string());
        let effective_model = format_resolved_route_value(row.effective_model.as_ref(), ctx.lang);
        let effective_cfg =
            format_resolved_route_value(row.effective_config_name.as_ref(), ctx.lang);
        let effective_upstream =
            format_resolved_route_value(row.effective_upstream_base_url.as_ref(), ctx.lang);
        let effective_effort =
            format_resolved_route_value(row.effective_reasoning_effort.as_ref(), ctx.lang);
        let effective_service_tier =
            format_resolved_route_value(row.effective_service_tier.as_ref(), ctx.lang);

        cols[1].label(format!("session: {sid_full}"));
        cols[1].label(format!("scope: {observation_scope}"));
        cols[1].label(format!("client(last): {client_full}"));
        cols[1].label(format!("cwd: {cwd_full}"));
        cols[1].label(format!("provider: {provider}"));
        cols[1].label(format!("binding: {binding_profile} ({binding_mode})"));
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "观测到的最近路由", "Observed route"));
        cols[1].label(format!("model(last): {observed_model}"));
        cols[1].label(format!("station(last): {observed_cfg}"));
        cols[1].label(format!("upstream(last): {observed_upstream}"));
        cols[1].label(format!("effort(last): {observed_effort}"));
        cols[1].label(format!("service_tier(last): {observed_service_tier}"));
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "当前生效控制", "Effective route"));
        cols[1].label(format!("model: {effective_model}"));
        cols[1].label(format!("station: {effective_cfg}"));
        cols[1].label(format!("upstream: {effective_upstream}"));
        cols[1].label(format!("effort: {effective_effort}"));
        cols[1].label(format!("service_tier: {effective_service_tier}"));
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "来源解释", "Source explanation"));
        egui::Grid::new("sessions_effective_route_explanation_grid")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(&mut cols[1], |ui| {
                ui.strong(pick(ctx.lang, "字段", "Field"));
                ui.strong(pick(ctx.lang, "当前值 / 来源", "Value / source"));
                ui.strong(pick(ctx.lang, "为什么", "Why"));
                ui.end_row();

                for field in EffectiveRouteField::ALL {
                    let explanation = explain_effective_route_field(row, field, ctx.lang);
                    ui.label(effective_route_field_label(field, ctx.lang));
                    ui.vertical(|ui| {
                        ui.monospace(explanation.value);
                        ui.small(format!("[{}]", explanation.source_label));
                    });
                    ui.small(explanation.reason);
                    ui.end_row();
                }
            });
        if !has_session_cards {
            cols[1].small(pick(
                ctx.lang,
                "当前附着数据来自旧接口回退，这里的来源解释是 best effort 推导。",
                "Current attach data came from legacy fallback endpoints, so this explanation is best effort.",
            ));
        }

        cols[1].horizontal(|ui| {
            let can_copy = row.session_id.is_some();
            if ui
                .add_enabled(
                    can_copy,
                    egui::Button::new(pick(ctx.lang, "复制 session_id", "Copy session_id")),
                )
                .clicked()
                && let Some(sid) = row.session_id.as_deref()
            {
                ui.ctx().copy_text(sid.to_string());
                *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
            }

            let can_open_cwd = row.cwd.is_some() && host_local_session_features;
            let mut open_cwd = ui.add_enabled(
                can_open_cwd,
                egui::Button::new(pick(ctx.lang, "打开 cwd", "Open cwd")),
            );
            if row.cwd.is_none() {
                open_cwd = open_cwd.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有可用 cwd。",
                    "The current session has no cwd.",
                ));
            } else if !host_local_session_features {
                open_cwd = open_cwd.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前附着的是远端代理；这个 cwd 来自 host-local 观测，不一定存在于这台设备上。",
                    "A remote proxy is attached; this cwd came from host-local observation and may not exist on this device.",
                ));
            }
            if open_cwd.clicked()
                && let Some(cwd) = row.cwd.as_deref()
            {
                let path = std::path::PathBuf::from(cwd);
                if let Err(e) = open_in_file_manager(&path, false) {
                    *ctx.last_error = Some(format!("open cwd failed: {e}"));
                }
            }

            let can_open_transcript = row.session_id.is_some() && host_local_session_features;
            let mut open_transcript = ui.add_enabled(
                can_open_transcript,
                egui::Button::new(pick(ctx.lang, "打开对话记录", "Open transcript")),
            );
            if row.session_id.is_none() {
                open_transcript = open_transcript.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有 session_id。",
                    "The current session has no session_id.",
                ));
            } else if !host_local_session_features {
                open_transcript = open_transcript.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前附着的是远端代理；GUI 无法假设这台设备能直接读取远端 host 的 ~/.codex/sessions。",
                    "A remote proxy is attached; the GUI cannot assume this device can directly read the remote host's ~/.codex/sessions.",
                ));
            }
            if open_transcript.clicked()
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
                                    source: SessionSummarySource::LocalFile,
                                    sort_hint_ms: None,
                                },
                            );
                            0
                        };

                        history::prepare_select_session_from_external(
                            &mut ctx.view.history,
                            selected_idx,
                            sid.clone(),
                        );
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
        if !host_local_session_features {
            if let Some(att) = ctx.proxy.attached()
                && let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
                    ctx.lang,
                    &[pick(ctx.lang, "cwd", "cwd"), pick(ctx.lang, "transcript", "transcript")],
                )
            {
                cols[1].small(warning);
            } else {
                cols[1].small(pick(
                    ctx.lang,
                    "提示：远端附着时，cwd / transcript 入口会被禁用；请用 Sessions / Requests 查看共享观测数据。",
                    "Tip: in remote-attached mode, cwd/transcript entries are disabled; use Sessions / Requests for shared observed data.",
                ));
            }
        }

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

        let override_model = row.override_model.as_deref().unwrap_or("-");
        let override_cfg = row.override_config_name.as_deref().unwrap_or("-");
        let override_eff = row.override_effort.as_deref().unwrap_or("-");
        let override_service_tier = row.override_service_tier.as_deref().unwrap_or("-");
        let global_cfg = global_override.as_deref().unwrap_or("-");
        cols[1].label(format!(
            "{}: model={override_model}, effort={override_eff}, station={override_cfg}, tier={override_service_tier}, global_station={global_cfg}",
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

        if profiles.is_empty() {
            cols[1].label(pick(
                ctx.lang,
                "当前未加载 control profile；可在 config.toml 的 [codex.profiles.*] 中定义。",
                "No control profiles loaded; define them in config.toml [codex.profiles.*].",
            ));
        } else {
            cols[1].horizontal_wrapped(|ui| {
                ui.label(pick(ctx.lang, "快捷应用", "Quick apply"));
                for profile in profiles.iter() {
                    let mut label =
                        format_profile_display(profile.name.as_str(), Some(profile));
                    if row.binding_profile_name.as_deref() == Some(profile.name.as_str()) {
                        label.push_str(match ctx.lang {
                            Language::Zh => " [当前绑定]",
                            Language::En => " [bound]",
                        });
                    }
                    let response =
                        ui.button(label).on_hover_text(format_profile_summary(profile));
                    if response.clicked() {
                        ctx.view.sessions.editor.profile_selection = Some(profile.name.clone());
                        action_apply_session_profile =
                            Some((sid.clone(), profile.name.clone()));
                    }
                }
            });

            cols[1].horizontal(|ui| {
                ui.label(pick(ctx.lang, "应用 profile", "Apply profile"));

                let mut selected_profile = ctx.view.sessions.editor.profile_selection.clone();
                egui::ComboBox::from_id_salt(("session_profile_apply", sid.as_str()))
                    .selected_text(match selected_profile.as_deref() {
                        Some(name) => format_profile_display(
                            name,
                            profiles.iter().find(|profile| profile.name == name),
                        ),
                        None => pick(ctx.lang, "<选择>", "<select>").to_string(),
                    })
                    .show_ui(ui, |ui| {
                        for profile in profiles.iter() {
                            ui.selectable_value(
                                &mut selected_profile,
                                Some(profile.name.clone()),
                                format_profile_display(profile.name.as_str(), Some(profile)),
                            );
                        }
                    });
                if selected_profile != ctx.view.sessions.editor.profile_selection {
                    ctx.view.sessions.editor.profile_selection = selected_profile;
                }

                if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                    if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.clone() {
                        action_apply_session_profile = Some((sid.clone(), profile_name));
                    } else {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "请先选择一个 profile。",
                                "Select a profile first.",
                            )
                            .to_string(),
                        );
                    }
                }
            });

            if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.as_deref()
                && let Some(profile) = profiles.iter().find(|profile| profile.name == profile_name)
            {
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "Profile 详情", "Profile details"),
                    format_profile_summary(profile)
                ));
                let preview_profile = service_profile_from_option(profile);
                let preview = build_profile_route_preview(
                    &preview_profile,
                    configured_active_station.as_deref(),
                    effective_active_station.as_deref(),
                    session_preview_station_specs,
                    session_preview_provider_catalog,
                    session_preview_runtime_station_catalog,
                );
                render_session_profile_apply_preview(
                    &mut cols[1],
                    ctx.lang,
                    row,
                    profile_name,
                    &preview_profile,
                    &preview,
                );
            }
        }

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "模型覆盖", "Model override"));
            ui.add(
                egui::TextEdit::singleline(&mut ctx.view.sessions.editor.model_override)
                    .desired_width(180.0)
                    .hint_text(pick(ctx.lang, "留空表示自动", "empty = auto")),
            );

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
                let desired = {
                    let v = ctx.view.sessions.editor.model_override.trim().to_string();
                    if v.is_empty() { None } else { Some(v) }
                };
                if !snapshot.supports_v1 {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "附着到的代理不支持会话模型覆盖（需要 API v1）。",
                            "Attached proxy does not support session model override (need API v1).",
                        )
                        .to_string(),
                    );
                } else {
                    match ctx.proxy.apply_session_model_override(ctx.rt, sid, desired) {
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
            ui.label(pick(ctx.lang, "固定站点", "Pinned station"));

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
                            "附着到的代理不支持会话固定站点（需要 API v1）。",
                            "Attached proxy does not support pinned session station (need API v1).",
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

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "Fast / Service Tier", "Fast / Service tier"));

            let mut choice = match ctx.view.sessions.editor.service_tier_override.as_deref() {
                None => "auto",
                Some("default") => "default",
                Some("priority") => "priority",
                Some("flex") => "flex",
                Some(_) => "custom",
            };

            egui::ComboBox::from_id_salt(("session_service_tier_choice", sid.as_str()))
                .selected_text(match choice {
                    "priority" => pick(ctx.lang, "priority（fast）", "priority (fast)"),
                    v => v,
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, "auto", "auto");
                    ui.selectable_value(&mut choice, "default", "default");
                    ui.selectable_value(
                        &mut choice,
                        "priority",
                        pick(ctx.lang, "priority（fast）", "priority (fast)"),
                    );
                    ui.selectable_value(&mut choice, "flex", "flex");
                    ui.selectable_value(&mut choice, "custom", "custom");
                });

            if choice == "auto" {
                ctx.view.sessions.editor.service_tier_override = None;
            } else if choice != "custom" {
                ctx.view.sessions.editor.service_tier_override = Some(choice.to_string());
                ctx.view.sessions.editor.custom_service_tier = choice.to_string();
            } else if ctx.view.sessions.editor.service_tier_override.is_none() {
                ctx.view.sessions.editor.service_tier_override =
                    Some(ctx.view.sessions.editor.custom_service_tier.clone());
            }

            ui.add(
                egui::TextEdit::singleline(&mut ctx.view.sessions.editor.custom_service_tier)
                    .desired_width(100.0)
                    .hint_text(pick(ctx.lang, "自定义", "custom")),
            );

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
                let desired = match choice {
                    "auto" => None,
                    "custom" => {
                        let v = ctx
                            .view
                            .sessions
                            .editor
                            .custom_service_tier
                            .trim()
                            .to_string();
                        if v.is_empty() { None } else { Some(v) }
                    }
                    v => Some(v.to_string()),
                };
                if !snapshot.supports_v1 {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "附着到的代理不支持会话 service tier 覆盖（需要 API v1）。",
                            "Attached proxy does not support session service tier override (need API v1).",
                        )
                        .to_string(),
                    );
                } else {
                    match ctx
                        .proxy
                        .apply_session_service_tier_override(ctx.rt, sid, desired)
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
    });

    if let Some((sid, profile_name)) = action_apply_session_profile {
        match ctx
            .proxy
            .apply_session_profile(ctx.rt, sid, profile_name.clone())
        {
            Ok(()) => {
                force_refresh = true;
                *ctx.last_info = Some(format!(
                    "{}: {profile_name}",
                    pick(ctx.lang, "已应用 profile", "Profile applied")
                ));
            }
            Err(e) => {
                *ctx.last_error = Some(format!("apply profile failed: {e}"));
            }
        }
    }

    if force_refresh {
        ctx.proxy
            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
    }
}

fn session_row_matches_query(row: &SessionRow, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    for s in [
        row.session_id.as_deref(),
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
        row.cwd.as_deref(),
        row.last_model.as_deref(),
        row.last_service_tier.as_deref(),
        row.last_provider_id.as_deref(),
        row.last_config_name.as_deref(),
        row.last_upstream_base_url.as_deref(),
        row.binding_profile_name.as_deref(),
        row.effective_model.as_ref().map(|v| v.value.as_str()),
        row.effective_reasoning_effort
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_service_tier
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_config_name.as_ref().map(|v| v.value.as_str()),
        row.effective_upstream_base_url
            .as_ref()
            .map(|v| v.value.as_str()),
        row.override_model.as_deref(),
        row.override_effort.as_deref(),
        row.override_config_name.as_deref(),
        row.override_service_tier.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if s.to_lowercase().contains(q) {
            return true;
        }
    }
    false
}

fn format_profile_display(name: &str, profile: Option<&ControlProfileOption>) -> String {
    match profile {
        Some(profile) if profile.is_default => format!("{name} [default]"),
        _ => name.to_string(),
    }
}

fn format_profile_summary(profile: &ControlProfileOption) -> String {
    let station = profile.station.as_deref().unwrap_or("auto");
    let model = profile.model.as_deref().unwrap_or("auto");
    let effort = profile.reasoning_effort.as_deref().unwrap_or("auto");
    let tier = profile.service_tier.as_deref().unwrap_or("auto");
    format!("station={station}, model={model}, effort={effort}, tier={tier}")
}

fn service_profile_from_option(
    profile: &ControlProfileOption,
) -> crate::config::ServiceControlProfile {
    crate::config::ServiceControlProfile {
        station: profile.station.clone(),
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfilePreviewStationSource {
    Profile,
    ConfiguredActive,
    Auto,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfilePreviewMemberRoute {
    provider_name: String,
    provider_alias: Option<String>,
    provider_enabled: Option<bool>,
    provider_missing: bool,
    endpoint_names: Vec<String>,
    uses_all_endpoints: bool,
    preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileRoutePreview {
    station_source: ProfilePreviewStationSource,
    resolved_station_name: Option<String>,
    station_exists: bool,
    structure_available: bool,
    station_alias: Option<String>,
    station_enabled: Option<bool>,
    station_level: Option<u8>,
    members: Vec<ProfilePreviewMemberRoute>,
    capabilities: Option<ConfigCapabilitySummary>,
    model_supported: Option<bool>,
    service_tier_supported: Option<bool>,
    reasoning_supported: Option<bool>,
}

fn build_profile_route_preview(
    profile: &crate::config::ServiceControlProfile,
    configured_active_station: Option<&str>,
    auto_station: Option<&str>,
    station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    runtime_station_catalog: Option<&BTreeMap<String, ConfigOption>>,
) -> ProfileRoutePreview {
    let explicit_station = non_empty_trimmed(profile.station.as_deref());
    let configured_active_station = non_empty_trimmed(configured_active_station);
    let auto_station = non_empty_trimmed(auto_station);

    let (station_source, resolved_station_name) = if let Some(name) = explicit_station {
        (ProfilePreviewStationSource::Profile, Some(name))
    } else if let Some(name) = configured_active_station {
        (ProfilePreviewStationSource::ConfiguredActive, Some(name))
    } else if let Some(name) = auto_station {
        (ProfilePreviewStationSource::Auto, Some(name))
    } else {
        (ProfilePreviewStationSource::Unresolved, None)
    };

    let station_spec = resolved_station_name
        .as_deref()
        .and_then(|name| station_specs.and_then(|specs| specs.get(name)));
    let runtime_station = resolved_station_name
        .as_deref()
        .and_then(|name| runtime_station_catalog.and_then(|catalog| catalog.get(name)));
    let capabilities = runtime_station.map(|station| station.capabilities.clone());

    let members = station_spec
        .map(|station| {
            station
                .members
                .iter()
                .map(|member| {
                    let provider =
                        provider_catalog.and_then(|providers| providers.get(&member.provider));
                    let endpoint_names = if member.endpoint_names.is_empty() {
                        provider
                            .map(|provider| {
                                provider
                                    .endpoints
                                    .iter()
                                    .map(|endpoint| endpoint.name.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        member.endpoint_names.clone()
                    };
                    ProfilePreviewMemberRoute {
                        provider_name: member.provider.clone(),
                        provider_alias: provider.and_then(|provider| provider.alias.clone()),
                        provider_enabled: provider.map(|provider| provider.enabled),
                        provider_missing: provider.is_none(),
                        endpoint_names,
                        uses_all_endpoints: member.endpoint_names.is_empty(),
                        preferred: member.preferred,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let model_supported = profile
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .and_then(|model| {
            capabilities.as_ref().and_then(|capabilities| {
                if capabilities.supported_models.is_empty() {
                    None
                } else {
                    Some(
                        capabilities
                            .supported_models
                            .iter()
                            .any(|item| item == model),
                    )
                }
            })
        });
    let service_tier_supported = profile
        .service_tier
        .as_deref()
        .filter(|tier| !tier.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_service_tier)
            })
        });
    let reasoning_supported = profile
        .reasoning_effort
        .as_deref()
        .filter(|effort| !effort.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_reasoning_effort)
            })
        });

    ProfileRoutePreview {
        station_source,
        station_exists: station_spec.is_some() || runtime_station.is_some(),
        structure_available: station_spec.is_some(),
        resolved_station_name,
        station_alias: station_spec
            .and_then(|station| station.alias.clone())
            .or_else(|| runtime_station.and_then(|station| station.alias.clone())),
        station_enabled: station_spec
            .map(|station| station.enabled)
            .or_else(|| runtime_station.map(|station| station.enabled)),
        station_level: station_spec
            .map(|station| station.level)
            .or_else(|| runtime_station.map(|station| station.level)),
        members,
        capabilities,
        model_supported,
        service_tier_supported,
        reasoning_supported,
    }
}

fn local_profile_preview_catalogs_from_text(
    text: &str,
    service_name: &str,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    let ConfigWorkingDocument::V2(cfg) = parse_proxy_config_document(text).ok()? else {
        return None;
    };
    let view = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    let catalog = crate::config::build_persisted_station_catalog(view);
    Some((
        catalog
            .stations
            .into_iter()
            .map(|station| (station.name.clone(), station))
            .collect(),
        catalog
            .providers
            .into_iter()
            .map(|provider| (provider.name.clone(), provider))
            .collect(),
    ))
}

fn capability_support_truthy(support: CapabilitySupport) -> Option<bool> {
    match support {
        CapabilitySupport::Supported => Some(true),
        CapabilitySupport::Unsupported => Some(false),
        CapabilitySupport::Unknown => None,
    }
}

fn render_profile_route_preview(
    ui: &mut egui::Ui,
    lang: Language,
    profile: &crate::config::ServiceControlProfile,
    preview: &ProfileRoutePreview,
) {
    ui.add_space(6.0);
    ui.group(|ui| {
        ui.label(pick(lang, "联动预览", "Linked preview"));

        let station_source = match preview.station_source {
            ProfilePreviewStationSource::Profile => pick(lang, "profile.station", "profile.station"),
            ProfilePreviewStationSource::ConfiguredActive => {
                pick(lang, "active_station", "active_station")
            }
            ProfilePreviewStationSource::Auto => pick(lang, "自动候选", "auto candidate"),
            ProfilePreviewStationSource::Unresolved => pick(lang, "未解析", "unresolved"),
        };
        ui.small(format!(
            "{}: {} ({})",
            pick(lang, "目标站点", "Target station"),
            preview
                .resolved_station_name
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<未确定>", "<unresolved>")),
            station_source
        ));

        if let Some(enabled) = preview.station_enabled {
            ui.small(format!(
                "{}: {}  {}: {}",
                pick(lang, "启用", "Enabled"),
                enabled,
                pick(lang, "等级", "Level"),
                preview.station_level.unwrap_or(1)
            ));
        }
        if let Some(alias) = preview.station_alias.as_deref()
            && !alias.trim().is_empty()
        {
            ui.small(format!("alias: {alias}"));
        }

        if preview.resolved_station_name.is_some() && !preview.station_exists {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前预览目标站点不存在，profile 落地后会失效或被校验拒绝。",
                    "The previewed target station does not exist; this profile would be invalid or rejected.",
                ),
            );
        }

        if let Some(capabilities) = preview.capabilities.as_ref() {
            ui.small(format!(
                "{}: {}  {}: {}",
                pick(lang, "支持 service tier", "Supports service tier"),
                capability_support_label(lang, capabilities.supports_service_tier),
                pick(lang, "支持 reasoning", "Supports reasoning"),
                capability_support_label(lang, capabilities.supports_reasoning_effort)
            ));
            if !capabilities.supported_models.is_empty() {
                ui.small(format!(
                    "{}: {}",
                    pick(lang, "支持模型", "Supported models"),
                    capabilities.supported_models.join(", ")
                ));
            }
        }

        if profile.service_tier.as_deref() == Some("priority") {
            ui.small(pick(
                lang,
                "fast mode 提示：当前 profile 使用 service_tier=priority。",
                "Fast mode hint: this profile uses service_tier=priority.",
            ));
        }
        if let Some(false) = preview.model_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 model 不在该站点已知支持模型列表内。",
                    "The current model is not in the station's known supported model list.",
                ),
            );
        }
        if let Some(false) = preview.service_tier_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 service_tier 与该站点能力摘要不匹配。",
                    "The current service_tier does not match the station capability summary.",
                ),
            );
        }
        if let Some(false) = preview.reasoning_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 reasoning_effort 与该站点能力摘要不匹配。",
                    "The current reasoning_effort does not match the station capability summary.",
                ),
            );
        }

        if !preview.structure_available {
            ui.small(pick(
                lang,
                "当前没有可见的 station/provider 结构，因此这里只能预览到站点层。",
                "No visible station/provider structure is available, so this preview is limited to the station layer.",
            ));
        } else if preview.members.is_empty() {
            ui.small(pick(
                lang,
                "当前站点还没有 member/provider 引用。",
                "The current station does not have any member/provider refs yet.",
            ));
        } else {
            ui.small(format!(
                "{}: {}",
                pick(lang, "成员路由", "Member routes"),
                preview.members.len()
            ));
            for (index, member) in preview.members.iter().enumerate() {
                let endpoint_scope = if member.uses_all_endpoints {
                    if member.endpoint_names.is_empty() {
                        pick(lang, "<全部 endpoint>", "<all endpoints>").to_string()
                    } else {
                        format!(
                            "{} ({})",
                            pick(lang, "全部 endpoint", "all endpoints"),
                            member.endpoint_names.join(", ")
                        )
                    }
                } else if member.endpoint_names.is_empty() {
                    pick(lang, "<未指定 endpoint>", "<no endpoints>").to_string()
                } else {
                    member.endpoint_names.join(", ")
                };
                let alias = member.provider_alias.as_deref().unwrap_or("-");
                let preferred = if member.preferred {
                    pick(lang, " preferred", " preferred")
                } else {
                    ""
                };
                let enabled_suffix = match member.provider_enabled {
                    Some(false) => " [off]",
                    _ => "",
                };
                let missing_suffix = if member.provider_missing {
                    pick(lang, " [missing]", " [missing]")
                } else {
                    ""
                };
                ui.small(format!(
                    "#{} {} ({}){}{}{} -> {}",
                    index + 1,
                    member.provider_name,
                    alias,
                    preferred,
                    enabled_suffix,
                    missing_suffix,
                    endpoint_scope
                ));
            }
        }
    });
}

fn session_route_preview_value(
    resolved: Option<&ResolvedRouteValue>,
    fallback: Option<&str>,
    lang: Language,
) -> String {
    resolved
        .map(|value| value.value.clone())
        .or_else(|| {
            fallback
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| pick(lang, "<未解析>", "<unresolved>").to_string())
}

fn session_profile_target_value(raw: Option<&str>, lang: Language) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>").to_string())
}

fn session_profile_target_station_value(preview: &ProfileRoutePreview, lang: Language) -> String {
    match preview.resolved_station_name.as_deref() {
        Some(name) => {
            let source = match preview.station_source {
                ProfilePreviewStationSource::Profile => "profile.station",
                ProfilePreviewStationSource::ConfiguredActive => "active_station",
                ProfilePreviewStationSource::Auto => "auto",
                ProfilePreviewStationSource::Unresolved => "unresolved",
            };
            format!("{name} ({source})")
        }
        None => match preview.station_source {
            ProfilePreviewStationSource::Unresolved => {
                pick(lang, "<未解析>", "<unresolved>").to_string()
            }
            _ => pick(lang, "<自动>", "<auto>").to_string(),
        },
    }
}

fn render_session_profile_apply_preview(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    profile_name: &str,
    profile: &crate::config::ServiceControlProfile,
    preview: &ProfileRoutePreview,
) {
    let has_manual_overrides = row.override_model.is_some()
        || row.override_config_name.is_some()
        || row.override_effort.is_some()
        || row.override_service_tier.is_some();

    ui.add_space(6.0);
    ui.group(|ui| {
        ui.label(pick(lang, "应用预览", "Apply preview"));
        ui.small(pick(
            lang,
            "应用 profile 会重写当前 session binding，并清空当前会话的 station / model / reasoning / service_tier overrides。",
            "Applying a profile rewrites the current session binding and clears the session's station / model / reasoning / service_tier overrides.",
        ));

        if row.binding_profile_name.as_deref() == Some(profile_name) {
            ui.small(if has_manual_overrides {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile，但重新应用仍会清空手动 session overrides。",
                    "This session is already bound to this profile, but reapplying it will still clear manual session overrides.",
                )
            } else {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile；重新应用通常只会刷新同一份绑定。",
                    "This session is already bound to this profile; reapplying it usually just refreshes the same binding.",
                )
            });
        }

        ui.small(format!(
            "{}: {} -> {}",
            pick(lang, "binding profile", "binding profile"),
            row.binding_profile_name
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<无>", "<none>")),
            profile_name
        ));
        ui.small(format!(
            "station: {} -> {}",
            session_route_preview_value(
                row.effective_config_name.as_ref(),
                row.last_config_name.as_deref(),
                lang,
            ),
            session_profile_target_station_value(preview, lang)
        ));
        ui.small(format!(
            "model: {} -> {}",
            session_route_preview_value(row.effective_model.as_ref(), row.last_model.as_deref(), lang),
            session_profile_target_value(profile.model.as_deref(), lang)
        ));
        ui.small(format!(
            "reasoning: {} -> {}",
            session_route_preview_value(
                row.effective_reasoning_effort.as_ref(),
                row.last_reasoning_effort.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.reasoning_effort.as_deref(), lang)
        ));
        ui.small(format!(
            "service_tier: {} -> {}",
            session_route_preview_value(
                row.effective_service_tier.as_ref(),
                row.last_service_tier.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.service_tier.as_deref(), lang)
        ));
    });

    render_profile_route_preview(ui, lang, profile, preview);
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
            .id_salt("requests_list_scroll")
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
            "service_tier: {}",
            r.service_tier.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "station: {}",
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
            .id_salt("stats_top_configs_scroll")
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
            .id_salt("stats_top_providers_scroll")
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
            Ok(t) => match parse_proxy_config_document(&t) {
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
                Ok(t) => match parse_proxy_config_document(&t) {
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
            match sync_codex_auth_into_document(&mut tmp, options) {
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
                Ok(t) => match parse_proxy_config_document(&t) {
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
            match sync_codex_auth_into_document(
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
                save_proxy_config_document(ctx.rt, cfg)
            };

            match save_res {
                Ok(()) => {
                    let new_path = crate::config::config_file_path();
                    if let Ok(t) = std::fs::read_to_string(&new_path) {
                        *ctx.proxy_config_text = t;
                    }
                    if let Ok(t) = std::fs::read_to_string(&new_path)
                        && let Ok(parsed) = parse_proxy_config_document(&t)
                    {
                        ctx.view.config.working = Some(parsed);
                    }

                    if matches!(
                        ctx.proxy.kind(),
                        super::proxy_control::ProxyModeKind::Running
                            | super::proxy_control::ProxyModeKind::Attached
                    ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                    {
                        *ctx.last_error = Some(format!("reload runtime failed: {e}"));
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

    if matches!(
        ctx.view.config.working.as_ref(),
        Some(ConfigWorkingDocument::V2(_))
    ) {
        render_config_form_v2(ui, ctx);
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
        let cfg = working_legacy_config(&ctx.view.config).expect("legacy branch");
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

    let selected_service_kind = ctx.view.config.service;
    let mut selected_name = ctx.view.config.selected_name.clone();
    let mut action_set_active: Option<String> = None;
    let mut action_clear_active = false;
    let mut action_health_start: Option<(bool, Vec<String>)> = None;
    let mut action_health_cancel: Option<(bool, Vec<String>)> = None;
    let mut action_save_apply = false;

    {
        let cfg = working_legacy_config_mut(&mut ctx.view.config).expect("legacy branch");
        ui.columns(2, |cols| {
            cols[0].heading(pick(ctx.lang, "配置列表", "Configs"));
            cols[0].add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("config_configs_scroll")
                .max_height(520.0)
                .show(&mut cols[0], |ui| {
                    for name in names.iter() {
                        let is_active = active_name.as_deref() == Some(name.as_str());
                        let is_fallback_active = active_name.is_none()
                            && active_fallback.as_deref() == Some(name.as_str());
                        let is_selected = selected_name.as_deref() == Some(name.as_str());

                        let svc = match selected_service_kind {
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
                            selected_name = Some(name.clone());
                        }
                    }
                });

            cols[1].heading(pick(ctx.lang, "详情", "Details"));
            cols[1].add_space(4.0);

            let Some(name) = selected_name.clone() else {
                cols[1].label(pick(ctx.lang, "未选择配置。", "No config selected."));
                return;
            };

            let mgr = match selected_service_kind {
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

            let selected_service = match selected_service_kind {
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
                        .id_salt("config_health_upstreams_scroll")
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

    ctx.view.config.selected_name = selected_name;

    if let Some(name) = action_set_active {
        let selected_service_kind = ctx.view.config.service;
        let cfg = working_legacy_config_mut(&mut ctx.view.config).expect("legacy branch");
        let mgr = match selected_service_kind {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        mgr.active = Some(name);
        *ctx.last_info = Some(pick(ctx.lang, "已设置 active", "Active set").to_string());
    }

    if action_clear_active {
        let selected_service_kind = ctx.view.config.service;
        let cfg = working_legacy_config_mut(&mut ctx.view.config).expect("legacy branch");
        let mgr = match selected_service_kind {
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
            save_proxy_config_document(ctx.rt, cfg)
        };
        match save_res {
            Ok(()) => {
                let new_path = crate::config::config_file_path();
                if let Ok(t) = std::fs::read_to_string(&new_path) {
                    *ctx.proxy_config_text = t;
                }
                if let Ok(t) = std::fs::read_to_string(&new_path)
                    && let Ok(parsed) = parse_proxy_config_document(&t)
                {
                    ctx.view.config.working = Some(parsed);
                }

                if matches!(
                    ctx.proxy.kind(),
                    super::proxy_control::ProxyModeKind::Running
                        | super::proxy_control::ProxyModeKind::Attached
                ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                {
                    *ctx.last_error = Some(format!("reload runtime failed: {e}"));
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

fn render_config_form_v2(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);
    ui.label(pick(
        ctx.lang,
        "当前文件是 v2 station/provider 布局。表单视图现在支持 station/provider/profile 的常用结构管理；provider tags、supported_models、model_mapping 等高级字段仍建议用“原始”视图。",
        "This file uses the v2 station/provider schema. Form view now covers common station/provider/profile structure management; use Raw view for advanced provider tags, supported_models, and model_mapping edits.",
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

    let (
        schema_version,
        active_name,
        active_fallback,
        default_profile,
        station_names,
        profile_names,
    ) = {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_ref() else {
            return;
        };
        let runtime = crate::config::compile_v2_to_runtime(cfg).ok();
        let (view, runtime_mgr) = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => {
                (&cfg.claude, runtime.as_ref().map(|r| &r.claude))
            }
            crate::config::ServiceKind::Codex => (&cfg.codex, runtime.as_ref().map(|r| &r.codex)),
        };
        let mut names = view.groups.keys().cloned().collect::<Vec<_>>();
        names.sort_by(|a, b| {
            let la = view.groups.get(a).map(|c| c.level).unwrap_or(1);
            let lb = view.groups.get(b).map(|c| c.level).unwrap_or(1);
            la.cmp(&lb).then_with(|| a.cmp(b))
        });
        let profiles = view.profiles.keys().cloned().collect::<Vec<_>>();
        (
            cfg.version,
            view.active_group.clone(),
            runtime_mgr.and_then(|mgr| mgr.active_config().map(|cfg| cfg.name.clone())),
            view.default_profile.clone(),
            names,
            profiles,
        )
    };

    let selected_service = match ctx.view.config.service {
        crate::config::ServiceKind::Claude => "claude",
        crate::config::ServiceKind::Codex => "codex",
    };
    let control_plane_snapshot = ctx.proxy.snapshot().filter(|snapshot| {
        snapshot.supports_v1 && snapshot.service_name.as_deref() == Some(selected_service)
    });
    let station_control_plane_snapshot = control_plane_snapshot
        .clone()
        .filter(|snapshot| snapshot.supports_persisted_station_config);
    let station_control_plane_catalog = station_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .configs
                .iter()
                .cloned()
                .map(|config| (config.name.clone(), config))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let station_control_plane_enabled = station_control_plane_snapshot.is_some();
    let station_control_plane_configured_active = station_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.configured_active_station.clone());
    let station_control_plane_effective_active = station_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.effective_active_station.clone());
    let station_default_profile = if station_control_plane_enabled {
        station_control_plane_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.configured_default_profile.clone())
            .or_else(|| {
                station_control_plane_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.default_profile.clone())
            })
    } else {
        default_profile.clone()
    };
    let attached_station_specs = ctx
        .proxy
        .attached()
        .filter(|att| {
            att.service_name.as_deref() == Some(selected_service) && att.supports_station_spec_api
        })
        .map(|att| {
            (
                att.persisted_stations.clone(),
                att.persisted_station_providers.clone(),
            )
        });
    let station_structure_control_plane_enabled = attached_station_specs.is_some();
    let station_structure_edit_enabled = station_structure_control_plane_enabled
        || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
    let attached_provider_specs = ctx
        .proxy
        .attached()
        .filter(|att| {
            att.service_name.as_deref() == Some(selected_service) && att.supports_provider_spec_api
        })
        .map(|att| att.persisted_providers.clone());
    let provider_structure_control_plane_enabled = attached_provider_specs.is_some();
    let provider_structure_edit_enabled = provider_structure_control_plane_enabled
        || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
    let station_display_names = if let Some((stations, _)) = attached_station_specs.as_ref() {
        let mut names = stations.values().cloned().collect::<Vec<_>>();
        names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
        names
            .into_iter()
            .map(|station| station.name)
            .collect::<Vec<_>>()
    } else if let Some(snapshot) = station_control_plane_snapshot.as_ref() {
        let mut names = snapshot.configs.clone();
        names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
        names
            .into_iter()
            .map(|config| config.name)
            .collect::<Vec<_>>()
    } else {
        station_names.clone()
    };

    if ctx
        .view
        .config
        .selected_name
        .as_ref()
        .is_none_or(|n| !station_display_names.iter().any(|x| x == n))
    {
        ctx.view.config.selected_name = station_display_names.first().cloned();
    }
    let selected_name = ctx.view.config.selected_name.clone();
    let selected_station_name = selected_name.clone().unwrap_or_default();
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
                    health.get(&selected_station_name).cloned(),
                    checks.get(&selected_station_name).cloned(),
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
                    att.config_health.get(&selected_station_name).cloned(),
                    att.health_checks.get(&selected_station_name).cloned(),
                )
            } else {
                (None, false, None, None)
            }
        }
        _ => (None, false, None, None),
    };

    let mut action_set_active: Option<String> = None;
    let mut action_clear_active = false;
    let mut action_set_active_remote: Option<Option<String>> = None;
    let mut action_health_start: Option<(bool, Vec<String>)> = None;
    let mut action_health_cancel: Option<(bool, Vec<String>)> = None;
    let mut action_save_apply = false;
    let mut action_save_apply_remote: Option<(String, bool, u8)> = None;
    let mut action_upsert_station_spec_remote: Option<(String, PersistedStationSpec)> = None;
    let mut action_delete_station_spec_remote: Option<String> = None;
    let mut action_upsert_provider_spec_remote: Option<(String, PersistedProviderSpec)> = None;
    let mut action_delete_provider_spec_remote: Option<String> = None;
    let mut station_editor_name = ctx.view.config.station_editor.station_name.clone();
    let mut station_editor_alias = ctx.view.config.station_editor.alias.clone();
    let mut station_editor_enabled = ctx.view.config.station_editor.enabled;
    let mut station_editor_level = ctx.view.config.station_editor.level.max(1);
    let mut station_editor_members = ctx.view.config.station_editor.members.clone();
    let mut new_station_name = ctx.view.config.station_editor.new_station_name.clone();
    let mut selected_provider_name = ctx.view.config.selected_provider_name.clone();
    let mut provider_editor_name = ctx.view.config.provider_editor.provider_name.clone();
    let mut provider_editor_alias = ctx.view.config.provider_editor.alias.clone();
    let mut provider_editor_enabled = ctx.view.config.provider_editor.enabled;
    let mut provider_editor_auth_token_env = ctx.view.config.provider_editor.auth_token_env.clone();
    let mut provider_editor_api_key_env = ctx.view.config.provider_editor.api_key_env.clone();
    let mut provider_editor_endpoints = ctx.view.config.provider_editor.endpoints.clone();
    let mut new_provider_name = ctx.view.config.provider_editor.new_provider_name.clone();
    if station_structure_control_plane_enabled {
        let selected_station = selected_name.as_deref().and_then(|name| {
            attached_station_specs
                .as_ref()
                .and_then(|specs| specs.0.get(name))
        });
        if station_editor_name.as_deref() != selected_name.as_deref() {
            station_editor_name = selected_name.clone();
            station_editor_alias = selected_station
                .and_then(|station| station.alias.clone())
                .unwrap_or_default();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(true);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members = selected_station
                .map(|station| {
                    station
                        .members
                        .iter()
                        .map(config_station_member_editor_from_member)
                        .collect()
                })
                .unwrap_or_default();
        }
    } else if station_control_plane_enabled {
        let selected_station = selected_name
            .as_deref()
            .and_then(|name| station_control_plane_catalog.get(name));
        if station_editor_name.as_deref() != selected_name.as_deref() {
            station_editor_name = selected_name.clone();
            station_editor_alias = String::new();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(false);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members.clear();
        }
    }
    let profile_control_plane_snapshot = control_plane_snapshot;
    let profile_control_plane_catalog = profile_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .profiles
                .iter()
                .map(|profile| (profile.name.clone(), service_profile_from_option(profile)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let profile_control_plane_default = profile_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.configured_default_profile.clone())
        .or_else(|| {
            profile_control_plane_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.default_profile.clone())
        });
    let profile_control_plane_station_names = profile_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            let mut names = snapshot
                .configs
                .iter()
                .map(|config| config.name.clone())
                .collect::<Vec<_>>();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_else(|| station_names.clone());
    let profile_control_plane_enabled = profile_control_plane_snapshot.is_some();
    let mut selected_profile_name = ctx.view.config.selected_profile_name.clone();
    if profile_control_plane_enabled {
        if selected_profile_name
            .as_ref()
            .is_none_or(|name| !profile_control_plane_catalog.contains_key(name))
        {
            selected_profile_name = profile_control_plane_default
                .clone()
                .or_else(|| profile_control_plane_catalog.keys().next().cloned());
        }
    } else if selected_profile_name
        .as_ref()
        .is_none_or(|name| !profile_names.iter().any(|item| item == name))
    {
        selected_profile_name = default_profile
            .clone()
            .or_else(|| profile_names.first().cloned());
    }
    let mut new_profile_name = ctx.view.config.new_profile_name.clone();
    let mut profile_editor_name = ctx.view.config.profile_editor.profile_name.clone();
    let mut profile_editor_station = ctx.view.config.profile_editor.station.clone();
    let mut profile_editor_model = ctx.view.config.profile_editor.model.clone();
    let mut profile_editor_reasoning_effort =
        ctx.view.config.profile_editor.reasoning_effort.clone();
    let mut profile_editor_service_tier = ctx.view.config.profile_editor.service_tier.clone();
    let mut profile_info: Option<String> = None;
    let mut profile_error: Option<String> = None;
    let mut action_profile_upsert_remote: Option<(String, crate::config::ServiceControlProfile)> =
        None;
    let mut action_profile_delete_remote: Option<String> = None;
    let mut action_profile_set_persisted_default_remote: Option<Option<String>> = None;

    if profile_control_plane_enabled {
        let selected_profile = selected_profile_name
            .as_deref()
            .and_then(|name| profile_control_plane_catalog.get(name));
        if profile_editor_name.as_deref() != selected_profile_name.as_deref() {
            profile_editor_name = selected_profile_name.clone();
            profile_editor_station = selected_profile.and_then(|profile| profile.station.clone());
            profile_editor_model = selected_profile
                .and_then(|profile| profile.model.clone())
                .unwrap_or_default();
            profile_editor_reasoning_effort = selected_profile
                .and_then(|profile| profile.reasoning_effort.clone())
                .unwrap_or_default();
            profile_editor_service_tier = selected_profile
                .and_then(|profile| profile.service_tier.clone())
                .unwrap_or_default();
        }
    }

    {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        let provider_catalog = view.providers.clone();
        let local_provider_catalog = crate::config::build_persisted_provider_catalog(view);
        let local_provider_spec_catalog = local_provider_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let local_station_catalog = crate::config::build_persisted_station_catalog(view);
        let local_station_spec_catalog = local_station_catalog
            .stations
            .iter()
            .cloned()
            .map(|station| (station.name.clone(), station))
            .collect::<BTreeMap<_, _>>();
        let local_provider_ref_catalog = local_station_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let attached_mode = matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
        let preview_station_specs = if station_structure_control_plane_enabled {
            attached_station_specs.as_ref().map(|specs| &specs.0)
        } else if attached_mode {
            None
        } else {
            Some(&local_station_spec_catalog)
        };
        let preview_provider_catalog = if station_structure_control_plane_enabled {
            attached_station_specs.as_ref().map(|specs| &specs.1)
        } else if attached_mode {
            None
        } else {
            Some(&local_provider_ref_catalog)
        };
        let preview_runtime_station_catalog = station_control_plane_snapshot
            .as_ref()
            .map(|_| &station_control_plane_catalog);
        if !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            && station_editor_name.as_deref() != selected_name.as_deref()
        {
            let selected_station = selected_name
                .as_deref()
                .and_then(|name| local_station_spec_catalog.get(name));
            station_editor_name = selected_name.clone();
            station_editor_alias = selected_station
                .and_then(|station| station.alias.clone())
                .unwrap_or_default();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(true);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members = selected_station
                .map(|station| {
                    station
                        .members
                        .iter()
                        .map(config_station_member_editor_from_member)
                        .collect()
                })
                .unwrap_or_default();
        }
        let mut provider_display_names =
            if let Some(provider_specs) = attached_provider_specs.as_ref() {
                provider_specs.keys().cloned().collect::<Vec<_>>()
            } else {
                local_provider_spec_catalog
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            };
        provider_display_names.sort();
        if selected_provider_name
            .as_ref()
            .is_none_or(|name| !provider_display_names.iter().any(|item| item == name))
        {
            selected_provider_name = provider_display_names.first().cloned();
        }
        if provider_structure_control_plane_enabled {
            let selected_provider = selected_provider_name.as_deref().and_then(|name| {
                attached_provider_specs
                    .as_ref()
                    .and_then(|specs| specs.get(name))
            });
            if provider_editor_name.as_deref() != selected_provider_name.as_deref() {
                provider_editor_name = selected_provider_name.clone();
                provider_editor_alias = selected_provider
                    .and_then(|provider| provider.alias.clone())
                    .unwrap_or_default();
                provider_editor_enabled = selected_provider
                    .map(|provider| provider.enabled)
                    .unwrap_or(true);
                provider_editor_auth_token_env = selected_provider
                    .and_then(|provider| provider.auth_token_env.clone())
                    .unwrap_or_default();
                provider_editor_api_key_env = selected_provider
                    .and_then(|provider| provider.api_key_env.clone())
                    .unwrap_or_default();
                provider_editor_endpoints = selected_provider
                    .map(|provider| {
                        provider
                            .endpoints
                            .iter()
                            .map(config_provider_endpoint_editor_from_spec)
                            .collect()
                    })
                    .unwrap_or_default();
            }
        } else if !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            && provider_editor_name.as_deref() != selected_provider_name.as_deref()
        {
            let selected_provider = selected_provider_name
                .as_deref()
                .and_then(|name| local_provider_spec_catalog.get(name));
            provider_editor_name = selected_provider_name.clone();
            provider_editor_alias = selected_provider
                .and_then(|provider| provider.alias.clone())
                .unwrap_or_default();
            provider_editor_enabled = selected_provider
                .map(|provider| provider.enabled)
                .unwrap_or(true);
            provider_editor_auth_token_env = selected_provider
                .and_then(|provider| provider.auth_token_env.clone())
                .unwrap_or_default();
            provider_editor_api_key_env = selected_provider
                .and_then(|provider| provider.api_key_env.clone())
                .unwrap_or_default();
            provider_editor_endpoints = selected_provider
                .map(|provider| {
                    provider
                        .endpoints
                        .iter()
                        .map(config_provider_endpoint_editor_from_spec)
                        .collect()
                })
                .unwrap_or_default();
        }
        let profile_catalog = view.profiles.clone();
        let configured_active_name = if station_control_plane_enabled {
            station_control_plane_configured_active.clone()
        } else {
            active_name.clone()
        };
        let effective_active_name = if station_control_plane_enabled {
            station_control_plane_effective_active.clone()
        } else if active_name.is_some() {
            active_name.clone()
        } else {
            active_fallback.clone()
        };

        ui.columns(2, |cols| {
            cols[0].heading(pick(ctx.lang, "站点列表", "Stations"));
            cols[0].add_space(4.0);
            cols[0].horizontal(|ui| {
                ui.label(pick(ctx.lang, "新建 station", "New station"));
                ui.add_sized(
                    [180.0, 22.0],
                    egui::TextEdit::singleline(&mut new_station_name).hint_text(pick(
                        ctx.lang,
                        "例如 primary / backup",
                        "e.g. primary / backup",
                    )),
                );
                if ui
                    .add_enabled(
                        station_structure_edit_enabled,
                        egui::Button::new(pick(ctx.lang, "新增", "Add")),
                    )
                    .clicked()
                {
                    let name = new_station_name.trim();
                    if name.is_empty() {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "station 名称不能为空。",
                                "Station name cannot be empty.",
                            )
                            .to_string(),
                        );
                    } else if station_structure_control_plane_enabled {
                        if attached_station_specs
                            .as_ref()
                            .is_some_and(|specs| specs.0.contains_key(name))
                        {
                            *ctx.last_error = Some(
                                pick(
                                    ctx.lang,
                                    "station 名称已存在。",
                                    "Station name already exists.",
                                )
                                .to_string(),
                            );
                        } else {
                            action_upsert_station_spec_remote = Some((
                                name.to_string(),
                                PersistedStationSpec {
                                    name: name.to_string(),
                                    alias: None,
                                    enabled: true,
                                    level: 1,
                                    members: Vec::new(),
                                },
                            ));
                            ctx.view.config.selected_name = Some(name.to_string());
                            station_editor_name = Some(name.to_string());
                            station_editor_alias.clear();
                            station_editor_enabled = true;
                            station_editor_level = 1;
                            station_editor_members.clear();
                            new_station_name.clear();
                        }
                    } else if view.groups.contains_key(name) {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "station 名称已存在。",
                                "Station name already exists.",
                            )
                            .to_string(),
                        );
                    } else {
                        view.groups.insert(name.to_string(), GroupConfigV2::default());
                        ctx.view.config.selected_name = Some(name.to_string());
                        new_station_name.clear();
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已新增 station（待保存）。",
                                "Station added (save pending).",
                            )
                            .to_string(),
                        );
                    }
                }
            });
            if !station_structure_edit_enabled {
                cols[0].small(pick(
                    ctx.lang,
                    "当前附着目标还没有暴露 station 结构 API，因此这里暂时不能新增/删除 station。",
                    "This attached target does not expose station structure APIs yet, so station create/delete is unavailable here.",
                ));
            }
            cols[0].add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("config_v2_stations_scroll")
                .max_height(520.0)
                .show(&mut cols[0], |ui| {
                    if station_display_names.is_empty() {
                        ui.label(pick(
                            ctx.lang,
                            "当前没有 station。可以先新增一个空 station，再补 member/provider 引用。",
                            "No stations yet. Add an empty station first, then fill member/provider refs.",
                        ));
                    }
                    for name in station_display_names.iter() {
                        let is_active = configured_active_name.as_deref() == Some(name.as_str());
                        let is_fallback_active = configured_active_name.is_none()
                            && effective_active_name.as_deref() == Some(name.as_str());
                        let is_selected = selected_name.as_deref() == Some(name.as_str());

                        let mut label = if station_control_plane_enabled {
                            let station = station_control_plane_catalog.get(name);
                            let (enabled, level, alias) = station
                                .map(|station| {
                                    (
                                        station.enabled,
                                        station.level.clamp(1, 10),
                                        station.alias.as_deref().unwrap_or(""),
                                    )
                                })
                                .unwrap_or((false, 1, ""));
                            let mut label = format!("L{level} {name}");
                            if !alias.trim().is_empty() {
                                label.push_str(&format!(" ({alias})"));
                            }
                            if !enabled {
                                label.push_str("  [off]");
                            }
                            label
                        } else {
                            let station = view.groups.get(name);
                            let (enabled, level, alias, members, endpoint_refs) = station
                                .map(|station| {
                                    let endpoint_refs = station
                                        .members
                                        .iter()
                                        .map(|member| {
                                            provider_catalog
                                                .get(&member.provider)
                                                .map(|provider| {
                                                    if member.endpoint_names.is_empty() {
                                                        provider.endpoints.len()
                                                    } else {
                                                        member.endpoint_names.len()
                                                    }
                                                })
                                                .unwrap_or(0)
                                        })
                                        .sum::<usize>();
                                    (
                                        station.enabled,
                                        station.level.clamp(1, 10),
                                        station.alias.as_deref().unwrap_or(""),
                                        station.members.len(),
                                        endpoint_refs,
                                    )
                                })
                                .unwrap_or((false, 1, "", 0, 0));

                            let mut label = format!("L{level} {name}");
                            if !alias.trim().is_empty() {
                                label.push_str(&format!(" ({alias})"));
                            }
                            label.push_str(&format!("  members={members} refs={endpoint_refs}"));
                            if !enabled {
                                label.push_str("  [off]");
                            }
                            label
                        };
                        if station_control_plane_enabled
                            && let Some(station) = station_control_plane_catalog.get(name)
                        {
                            if let Some(override_enabled) = station.runtime_enabled_override {
                                label.push_str(&format!("  rt_enabled={override_enabled}"));
                            }
                            if let Some(override_level) = station.runtime_level_override {
                                label.push_str(&format!("  rt_level={override_level}"));
                            }
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

            cols[1].heading(pick(ctx.lang, "站点详情", "Station Details"));
            cols[1].add_space(4.0);

            let Some(name) = ctx.view.config.selected_name.clone() else {
                cols[1].label(pick(ctx.lang, "未选择站点。", "No station selected."));
                return;
            };

            let active_label = if configured_active_name.as_deref() == Some(name.as_str()) {
                pick(ctx.lang, "是", "yes")
            } else {
                pick(ctx.lang, "否", "no")
            };
            let effective_label = effective_active_name
                .clone()
                .unwrap_or_else(|| pick(ctx.lang, "(无)", "(none)").to_string());

            cols[1].label(format!("schema: v{schema_version}"));
            cols[1].label(format!("active_station: {active_label}"));
            cols[1].label(format!(
                "{}: {effective_label}",
                pick(ctx.lang, "生效站点", "Effective station")
            ));
            cols[1].label(format!(
                "default_profile: {}",
                station_default_profile.as_deref().unwrap_or("-")
            ));
            cols[1].add_space(6.0);

            if station_structure_control_plane_enabled
                || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            {
                let (station_snapshot, provider_ref_catalog) = if station_structure_control_plane_enabled {
                    let Some((station_specs, provider_specs)) = attached_station_specs.as_ref() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "远端 station 结构视图不可用。",
                            "Remote station structure view is unavailable.",
                        ));
                        return;
                    };
                    let Some(station_snapshot) = station_specs.get(&name).cloned() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "远端 station 不存在（可能已被删除）。",
                            "Remote station missing.",
                        ));
                        return;
                    };
                    (station_snapshot, provider_specs)
                } else {
                    let Some(station_snapshot) = local_station_spec_catalog.get(&name).cloned() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "站点不存在（可能已被删除）。",
                            "Station missing.",
                        ));
                        return;
                    };
                    (station_snapshot, &local_provider_ref_catalog)
                };

                let referencing_profiles = profile_catalog
                    .iter()
                    .filter_map(|(profile_name, profile)| {
                        (profile.station.as_deref() == Some(name.as_str()))
                            .then_some(profile_name.clone())
                    })
                    .collect::<Vec<_>>();

                cols[1].colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    if station_structure_control_plane_enabled {
                        pick(
                            ctx.lang,
                            "当前通过附着代理暴露的 station 结构 API 直接管理远端配置；provider 密钥仍不会通过这里暴露。",
                            "This view manages the attached proxy through its station structure API directly; provider secrets are still not exposed here.",
                        )
                    } else {
                        pick(
                            ctx.lang,
                            "这里编辑的是本机 v2 station/provider 结构；保存后会重载当前代理。",
                            "This edits the local v2 station/provider structure; saving will reload the current proxy.",
                        )
                    },
                );
                cols[1].add_space(6.0);
                cols[1].label(format!("name: {}", name));
                cols[1].label(format!("members: {}", station_snapshot.members.len()));
                cols[1].label(format!(
                    "profiles: {}",
                    if referencing_profiles.is_empty() {
                        "-".to_string()
                    } else {
                        referencing_profiles.join(", ")
                    }
                ));

                cols[1].horizontal(|ui| {
                    ui.label("alias");
                    ui.add_sized(
                        [220.0, 22.0],
                        egui::TextEdit::singleline(&mut station_editor_alias),
                    );
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        station_editor_alias.clear();
                    }
                });

                cols[1].horizontal(|ui| {
                    ui.checkbox(&mut station_editor_enabled, pick(ctx.lang, "启用", "Enabled"));
                    ui.label(pick(ctx.lang, "等级", "Level"));
                    ui.add(egui::DragValue::new(&mut station_editor_level).range(1..=10));
                });

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(ctx.lang, "成员引用", "Members"));
                render_config_station_member_editor(
                    &mut cols[1],
                    ctx.lang,
                    selected_service,
                    provider_ref_catalog,
                    &mut station_editor_members,
                );

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(ctx.lang, "可用 Provider", "Available Providers"));
                render_config_station_provider_summary(
                    &mut cols[1],
                    ctx.lang,
                    provider_ref_catalog,
                    &station_editor_members,
                );

                cols[1].add_space(8.0);
                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 active_station", "Set active_station"))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(Some(name.clone()));
                        } else {
                            action_set_active = Some(name.clone());
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            "清除 active_station",
                            "Clear active_station",
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(None);
                        } else {
                            action_clear_active = true;
                        }
                    }

                    if ui.button(pick(ctx.lang, "删除 station", "Delete station")).clicked() {
                        if !referencing_profiles.is_empty() {
                            *ctx.last_error = Some(format!(
                                "{}: {}",
                                pick(
                                    ctx.lang,
                                    "仍有 profile 引用了该 station，不能删除",
                                    "Profiles still reference this station; delete is blocked",
                                ),
                                referencing_profiles.join(", ")
                            ));
                        } else if station_structure_control_plane_enabled {
                            action_delete_station_spec_remote = Some(name.clone());
                        } else {
                            view.groups.remove(name.as_str());
                            if view.active_group.as_deref() == Some(name.as_str()) {
                                view.active_group = None;
                            }
                            ctx.view.config.selected_name = view.groups.keys().next().cloned();
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已删除 station（待保存）。",
                                    "Station deleted (save pending).",
                                )
                                .to_string(),
                            );
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            if station_structure_control_plane_enabled {
                                "保存到当前代理"
                            } else {
                                "保存并应用"
                            },
                            if station_structure_control_plane_enabled {
                                "Save to current proxy"
                            } else {
                                "Save & apply"
                            },
                        ))
                        .clicked()
                    {
                        match build_station_spec_from_config_editor(
                            name.as_str(),
                            station_editor_alias.as_str(),
                            station_editor_enabled,
                            station_editor_level,
                            &station_editor_members,
                        ) {
                            Ok(station_spec) => {
                                if station_structure_control_plane_enabled {
                                    action_upsert_station_spec_remote =
                                        Some((name.clone(), station_spec));
                                } else {
                                    view.groups.insert(
                                        name.clone(),
                                        GroupConfigV2 {
                                            alias: station_spec.alias.clone(),
                                            enabled: station_spec.enabled,
                                            level: station_spec.level,
                                            members: station_spec.members.clone(),
                                        },
                                    );
                                    action_save_apply = true;
                                }
                            }
                            Err(e) => {
                                *ctx.last_error = Some(e);
                            }
                        }
                    }
                });
            } else {
                let Some(station_snapshot) = station_control_plane_catalog.get(&name).cloned() else {
                    cols[1].label(pick(
                        ctx.lang,
                        "远端 station 不存在（可能已被删除）。",
                        "Remote station missing.",
                    ));
                    return;
                };
                cols[1].colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "当前 common station 字段直接管理运行中的代理；provider/member 结构请回到原始视图或本机文件查看。",
                        "Common station fields below manage the live proxy directly; use Raw view or the local file for provider/member structure.",
                    ),
                );
                cols[1].add_space(6.0);
                cols[1].label(format!("name: {}", name));
                cols[1].label(format!(
                    "alias: {}",
                    station_snapshot.alias.as_deref().unwrap_or("-")
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "配置启用", "Configured enabled"),
                    station_snapshot.configured_enabled
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "配置等级", "Configured level"),
                    station_snapshot.configured_level
                ));
                cols[1].label(format!(
                    "{}: {:?}",
                    pick(ctx.lang, "运行状态", "Runtime state"),
                    station_snapshot.runtime_state
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "运行时 enabled 覆盖", "Runtime enabled override"),
                    station_snapshot
                        .runtime_enabled_override
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "运行时 level 覆盖", "Runtime level override"),
                    station_snapshot
                        .runtime_level_override
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ));
                cols[1].label(format!(
                    "{}: {:?}",
                    pick(ctx.lang, "模型目录", "Model catalog"),
                    station_snapshot.capabilities.model_catalog_kind
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "支持 service tier", "Supports service tier"),
                    capability_support_label(
                        ctx.lang,
                        station_snapshot.capabilities.supports_service_tier
                    )
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "支持 reasoning", "Supports reasoning"),
                    capability_support_label(
                        ctx.lang,
                        station_snapshot.capabilities.supports_reasoning_effort
                    )
                ));
                if !station_snapshot.capabilities.supported_models.is_empty() {
                    cols[1].small(format!(
                        "{}: {}",
                        pick(ctx.lang, "支持模型", "Supported models"),
                        station_snapshot.capabilities.supported_models.join(", ")
                    ));
                }
                cols[1].add_space(6.0);
                cols[1].horizontal(|ui| {
                    ui.checkbox(
                        &mut station_editor_enabled,
                        pick(ctx.lang, "启用", "Enabled"),
                    );
                    ui.label(pick(ctx.lang, "等级", "Level"));
                    ui.add(egui::DragValue::new(&mut station_editor_level).range(1..=10));
                });
            }

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(ctx.lang, "健康检查", "Health check"));
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
                }
            }

            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
                && !station_structure_control_plane_enabled
            {
                cols[1].add_space(6.0);
                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 active_station", "Set active_station"))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(Some(name.clone()));
                        } else {
                            action_set_active = Some(name.clone());
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            "清除 active_station",
                            "Clear active_station",
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(None);
                        } else {
                            action_clear_active = true;
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            if station_control_plane_enabled {
                                "保存到当前代理"
                            } else {
                                "保存并应用"
                            },
                            if station_control_plane_enabled {
                                "Save to current proxy"
                            } else {
                                "Save & apply"
                            },
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_save_apply_remote = Some((
                                name.clone(),
                                station_editor_enabled,
                                station_editor_level.clamp(1, 10),
                            ));
                        } else {
                            action_save_apply = true;
                        }
                    }
                });
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.group(|ui| {
            ui.heading(pick(ctx.lang, "Providers", "Providers"));
            ui.label(pick(
                ctx.lang,
                "Provider 负责认证引用与 endpoint 集合；适合做快捷切换、故障切换和不同中转站的结构管理。这里不会显示明文密钥。",
                "Providers hold auth references plus endpoint sets; they are the right place for quick switching, failover, and relay structure management. Plaintext secrets are never shown here.",
            ));

            if provider_structure_control_plane_enabled
                || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            {
                ui.columns(2, |cols| {
                    cols[0].heading(pick(ctx.lang, "Provider 列表", "Provider list"));
                    cols[0].add_space(4.0);
                    cols[0].horizontal(|ui| {
                        ui.label(pick(ctx.lang, "新建 provider", "New provider"));
                        ui.add_sized(
                            [180.0, 22.0],
                            egui::TextEdit::singleline(&mut new_provider_name).hint_text(pick(
                                ctx.lang,
                                "例如 right / backup",
                                "e.g. right / backup",
                            )),
                        );
                        if ui
                            .add_enabled(
                                provider_structure_edit_enabled,
                                egui::Button::new(pick(ctx.lang, "新增", "Add")),
                            )
                            .clicked()
                        {
                            let name = new_provider_name.trim();
                            if name.is_empty() {
                                *ctx.last_error = Some(
                                    pick(
                                        ctx.lang,
                                        "provider 名称不能为空。",
                                        "Provider name cannot be empty.",
                                    )
                                    .to_string(),
                                );
                            } else if provider_structure_control_plane_enabled {
                                if attached_provider_specs
                                    .as_ref()
                                    .is_some_and(|providers| providers.contains_key(name))
                                {
                                    *ctx.last_error = Some(
                                        pick(
                                            ctx.lang,
                                            "provider 名称已存在。",
                                            "Provider name already exists.",
                                        )
                                        .to_string(),
                                    );
                                } else {
                                    action_upsert_provider_spec_remote = Some((
                                        name.to_string(),
                                        PersistedProviderSpec {
                                            name: name.to_string(),
                                            alias: None,
                                            enabled: true,
                                            auth_token_env: None,
                                            api_key_env: None,
                                            endpoints: Vec::new(),
                                        },
                                    ));
                                    selected_provider_name = Some(name.to_string());
                                    provider_editor_name = Some(name.to_string());
                                    provider_editor_alias.clear();
                                    provider_editor_enabled = true;
                                    provider_editor_auth_token_env.clear();
                                    provider_editor_api_key_env.clear();
                                    provider_editor_endpoints.clear();
                                    new_provider_name.clear();
                                }
                            } else if view.providers.contains_key(name) {
                                *ctx.last_error = Some(
                                    pick(
                                        ctx.lang,
                                        "provider 名称已存在。",
                                        "Provider name already exists.",
                                    )
                                    .to_string(),
                                );
                            } else {
                                view.providers.insert(name.to_string(), ProviderConfigV2::default());
                                selected_provider_name = Some(name.to_string());
                                provider_editor_name = Some(name.to_string());
                                provider_editor_alias.clear();
                                provider_editor_enabled = true;
                                provider_editor_auth_token_env.clear();
                                provider_editor_api_key_env.clear();
                                provider_editor_endpoints.clear();
                                new_provider_name.clear();
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已新增 provider（待保存）。",
                                        "Provider added (save pending).",
                                    )
                                    .to_string(),
                                );
                            }
                        }
                    });

                    egui::ScrollArea::vertical()
                        .id_salt("config_v2_providers_scroll")
                        .max_height(300.0)
                        .show(&mut cols[0], |ui| {
                            if provider_display_names.is_empty() {
                                ui.label(pick(
                                    ctx.lang,
                                    "当前没有 provider。可以先新增一个空 provider，再补 endpoint 与 env 引用。",
                                    "No providers yet. Add an empty provider first, then fill endpoints and env refs.",
                                ));
                            }
                            for name in provider_display_names.iter() {
                                let provider = if provider_structure_control_plane_enabled {
                                    attached_provider_specs
                                        .as_ref()
                                        .and_then(|providers| providers.get(name))
                                } else {
                                    local_provider_spec_catalog.get(name)
                                };
                                let (alias, enabled, endpoints) = provider
                                    .map(|provider| {
                                        (
                                            provider.alias.as_deref().unwrap_or(""),
                                            provider.enabled,
                                            provider.endpoints.len(),
                                        )
                                    })
                                    .unwrap_or(("", false, 0));
                                let mut label = format!("{name}  endpoints={endpoints}");
                                if !alias.trim().is_empty() {
                                    label.push_str(&format!(" ({alias})"));
                                }
                                if !enabled {
                                    label.push_str("  [off]");
                                }
                                if ui
                                    .selectable_label(
                                        selected_provider_name.as_deref() == Some(name.as_str()),
                                        label,
                                    )
                                    .clicked()
                                {
                                    selected_provider_name = Some(name.clone());
                                }
                            }
                        });

                    cols[1].heading(pick(ctx.lang, "Provider 详情", "Provider details"));
                    cols[1].add_space(4.0);

                    let Some(name) = selected_provider_name.clone() else {
                        cols[1].label(pick(ctx.lang, "未选择 provider。", "No provider selected."));
                        return;
                    };

                    let provider_snapshot = if provider_structure_control_plane_enabled {
                        let Some(provider) = attached_provider_specs
                            .as_ref()
                            .and_then(|providers| providers.get(name.as_str()))
                            .cloned()
                        else {
                            cols[1].label(pick(
                                ctx.lang,
                                "远端 provider 不存在（可能已被删除）。",
                                "Remote provider missing.",
                            ));
                            return;
                        };
                        provider
                    } else {
                        let Some(provider) = local_provider_spec_catalog.get(name.as_str()).cloned()
                        else {
                            cols[1].label(pick(
                                ctx.lang,
                                "provider 不存在（可能已被删除）。",
                                "Provider missing.",
                            ));
                            return;
                        };
                        provider
                    };

                    let referencing_stations = if provider_structure_control_plane_enabled {
                        attached_station_specs
                            .as_ref()
                            .map(|(stations, _)| {
                                stations
                                    .iter()
                                    .filter_map(|(station_name, station)| {
                                        station
                                            .members
                                            .iter()
                                            .any(|member| member.provider == name)
                                            .then_some(station_name.clone())
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        view.groups
                            .iter()
                            .filter_map(|(station_name, station)| {
                                station
                                    .members
                                    .iter()
                                    .any(|member| member.provider == name)
                                    .then_some(station_name.clone())
                            })
                            .collect::<Vec<_>>()
                    };

                    cols[1].colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        if provider_structure_control_plane_enabled {
                            pick(
                                ctx.lang,
                                "当前通过附着代理暴露的 provider 结构 API 直接管理远端 provider；明文密钥、tags、模型映射等高级字段仍不会在这里暴露。",
                                "This view manages the attached proxy through its provider structure API directly; plaintext secrets, tags, and model mappings are still not exposed here.",
                            )
                        } else {
                            pick(
                                ctx.lang,
                                "这里编辑的是本机 v2 provider 结构；保存后会重载当前代理。高级 tags / model_mapping 仍建议在 Raw 视图处理。",
                                "This edits the local v2 provider structure; saving will reload the current proxy. Advanced tags and model_mapping are still better handled in Raw view.",
                            )
                        },
                    );
                    cols[1].add_space(6.0);
                    cols[1].label(format!("name: {name}"));
                    cols[1].label(format!("endpoints: {}", provider_snapshot.endpoints.len()));
                    cols[1].label(format!(
                        "stations: {}",
                        if referencing_stations.is_empty() {
                            "-".to_string()
                        } else {
                            referencing_stations.join(", ")
                        }
                    ));

                    cols[1].horizontal(|ui| {
                        ui.label("alias");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_alias),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_alias.clear();
                        }
                    });

                    cols[1].horizontal(|ui| {
                        ui.checkbox(&mut provider_editor_enabled, pick(ctx.lang, "启用", "Enabled"));
                    });

                    cols[1].horizontal(|ui| {
                        ui.label("auth_token_env");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_auth_token_env),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_auth_token_env.clear();
                        }
                    });

                    cols[1].horizontal(|ui| {
                        ui.label("api_key_env");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_api_key_env),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_api_key_env.clear();
                        }
                    });

                    cols[1].add_space(8.0);
                    cols[1].separator();
                    cols[1].label(pick(ctx.lang, "Endpoints", "Endpoints"));
                    render_config_provider_endpoint_editor(
                        &mut cols[1],
                        ctx.lang,
                        selected_service,
                        name.as_str(),
                        &mut provider_editor_endpoints,
                    );

                    cols[1].add_space(8.0);
                    cols[1].horizontal(|ui| {
                        if ui
                            .button(pick(
                                ctx.lang,
                                if provider_structure_control_plane_enabled {
                                    "保存到当前代理"
                                } else {
                                    "保存并应用"
                                },
                                if provider_structure_control_plane_enabled {
                                    "Save to current proxy"
                                } else {
                                    "Save & apply"
                                },
                            ))
                            .clicked()
                        {
                            match build_provider_spec_from_config_editor(
                                name.as_str(),
                                provider_editor_alias.as_str(),
                                provider_editor_enabled,
                                provider_editor_auth_token_env.as_str(),
                                provider_editor_api_key_env.as_str(),
                                &provider_editor_endpoints,
                            ) {
                                Ok(provider_spec) => {
                                    if provider_structure_control_plane_enabled {
                                        action_upsert_provider_spec_remote =
                                            Some((name.clone(), provider_spec));
                                    } else {
                                        let existing_provider =
                                            view.providers.get(name.as_str()).cloned();
                                        view.providers.insert(
                                            name.clone(),
                                            merge_provider_spec_into_provider_config(
                                                existing_provider.as_ref(),
                                                &provider_spec,
                                            ),
                                        );
                                        action_save_apply = true;
                                    }
                                }
                                Err(e) => {
                                    *ctx.last_error = Some(e);
                                }
                            }
                        }

                        if ui
                            .button(pick(ctx.lang, "删除 provider", "Delete provider"))
                            .clicked()
                        {
                            if !provider_structure_control_plane_enabled
                                && !referencing_stations.is_empty()
                            {
                                *ctx.last_error = Some(format!(
                                    "{}: {}",
                                    pick(
                                        ctx.lang,
                                        "仍有 station 引用了该 provider，不能删除",
                                        "Stations still reference this provider; delete is blocked",
                                    ),
                                    referencing_stations.join(", ")
                                ));
                            } else if provider_structure_control_plane_enabled {
                                action_delete_provider_spec_remote = Some(name.clone());
                            } else {
                                view.providers.remove(name.as_str());
                                selected_provider_name = view.providers.keys().next().cloned();
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已删除 provider（待保存）。",
                                        "Provider deleted (save pending).",
                                    )
                                    .to_string(),
                                );
                            }
                        }
                    });
                });
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "当前附着目标还没有暴露 provider 结构 API；这里保持只读，避免误导为会写回本机文件。需要查看高级字段时请使用 Raw 视图。",
                        "This attached target does not expose provider structure APIs yet; this section stays read-only to avoid implying local-file writes. Use Raw view for advanced fields.",
                    ),
                );
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.group(|ui| {
            ui.heading(pick(ctx.lang, "Profiles", "Profiles"));
            ui.label(pick(
                ctx.lang,
                "Profile 用于把 station / model / reasoning_effort / service_tier 组合成可复用控制模板；更适合表达 fast mode、模型切换和思考模式。",
                "Profiles bundle station / model / reasoning_effort / service_tier into reusable control templates for fast mode, model switching, and reasoning mode.",
            ));
            if profile_control_plane_enabled {
                render_config_v2_profiles_control_plane(
                    ui,
                    ctx.lang,
                    selected_service,
                    &profile_control_plane_catalog,
                    profile_control_plane_default.as_deref(),
                    &profile_control_plane_station_names,
                    &mut selected_profile_name,
                    &mut new_profile_name,
                    &mut profile_editor_name,
                    &mut profile_editor_station,
                    &mut profile_editor_model,
                    &mut profile_editor_reasoning_effort,
                    &mut profile_editor_service_tier,
                    &mut profile_error,
                    &mut action_profile_upsert_remote,
                    &mut action_profile_delete_remote,
                    &mut action_profile_set_persisted_default_remote,
                    matches!(ctx.proxy.kind(), ProxyModeKind::Attached),
                    station_control_plane_enabled,
                    station_control_plane_configured_active.as_deref(),
                    station_control_plane_effective_active.as_deref(),
                    preview_station_specs,
                    preview_provider_catalog,
                    preview_runtime_station_catalog,
                );
            } else {
            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "新建 profile", "New profile"));
                ui.add_sized(
                    [180.0, 22.0],
                    egui::TextEdit::singleline(&mut new_profile_name).hint_text(pick(
                        ctx.lang,
                        "例如 fast / deep / cheap",
                        "e.g. fast / deep / cheap",
                    )),
                );
                if ui.button(pick(ctx.lang, "新增", "Add")).clicked() {
                    let name = new_profile_name.trim();
                    if name.is_empty() {
                        profile_error = Some(
                            pick(
                                ctx.lang,
                                "profile 名称不能为空。",
                                "Profile name cannot be empty.",
                            )
                            .to_string(),
                        );
                    } else if view.profiles.contains_key(name) {
                        profile_error = Some(
                            pick(
                                ctx.lang,
                                "profile 名称已存在。",
                                "Profile name already exists.",
                            )
                            .to_string(),
                        );
                    } else {
                        view.profiles.insert(
                            name.to_string(),
                            crate::config::ServiceControlProfile::default(),
                        );
                        if view.default_profile.is_none() {
                            view.default_profile = Some(name.to_string());
                        }
                        selected_profile_name = Some(name.to_string());
                        new_profile_name.clear();
                        profile_info = Some(
                            pick(
                                ctx.lang,
                                "已新增 profile（待保存）。",
                                "Profile added (save pending).",
                            )
                            .to_string(),
                        );
                    }
                }
            });

            ui.add_space(6.0);
            ui.columns(2, |cols| {
                cols[0].label(pick(ctx.lang, "Profile 列表", "Profile list"));
                cols[0].add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("config_v2_profiles_scroll")
                    .max_height(240.0)
                    .show(&mut cols[0], |ui| {
                        let names = view.profiles.keys().cloned().collect::<Vec<_>>();
                        if names.is_empty() {
                            ui.label(pick(
                                ctx.lang,
                                "(当前没有 profile)",
                                "(no profiles yet)",
                            ));
                        } else {
                            for name in names {
                                let is_selected =
                                    selected_profile_name.as_deref() == Some(name.as_str());
                                let label = if view.default_profile.as_deref()
                                    == Some(name.as_str())
                                {
                                    format!("{name} [default]")
                                } else {
                                    name.clone()
                                };
                                if ui.selectable_label(is_selected, label).clicked() {
                                    selected_profile_name = Some(name);
                                }
                            }
                        }
                    });

                cols[1].label(pick(ctx.lang, "Profile 详情", "Profile details"));
                cols[1].add_space(4.0);

                let Some(profile_name) = selected_profile_name.clone() else {
                    cols[1].label(pick(
                        ctx.lang,
                        "未选择 profile。",
                        "No profile selected.",
                    ));
                    return;
                };

                let is_default = view.default_profile.as_deref() == Some(profile_name.as_str());
                let mut delete_selected = false;
                let Some(profile) = view.profiles.get_mut(profile_name.as_str()) else {
                    cols[1].label(pick(
                        ctx.lang,
                        "profile 不存在（可能已被删除）。",
                        "Profile missing.",
                    ));
                    return;
                };

                cols[1].label(format!("name: {profile_name}"));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "默认", "Default"),
                    if is_default {
                        pick(ctx.lang, "是", "yes")
                    } else {
                        pick(ctx.lang, "否", "no")
                    }
                ));

                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 default_profile", "Set default_profile"))
                        .clicked()
                    {
                        view.default_profile = Some(profile_name.clone());
                        profile_info = Some(
                            pick(
                                ctx.lang,
                                "已更新 default_profile（待保存）。",
                                "default_profile updated (save pending).",
                            )
                            .to_string(),
                        );
                    }
                    if ui
                        .button(pick(ctx.lang, "清除 default_profile", "Clear default_profile"))
                        .clicked()
                    {
                        if is_default {
                            view.default_profile = None;
                            profile_info = Some(
                                pick(
                                    ctx.lang,
                                    "已清除 default_profile（待保存）。",
                                    "default_profile cleared (save pending).",
                                )
                                .to_string(),
                            );
                        }
                    }
                    if ui.button(pick(ctx.lang, "删除 profile", "Delete profile")).clicked() {
                        delete_selected = true;
                    }
                });

                let mut station = profile.station.clone();
                cols[1].horizontal(|ui| {
                    ui.label(pick(ctx.lang, "station", "station"));
                    egui::ComboBox::from_id_salt(format!(
                        "config_v2_profile_station_{selected_service}_{profile_name}"
                    ))
                    .selected_text(
                        station
                            .as_deref()
                            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>")),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut station,
                            None,
                            pick(ctx.lang, "<自动>", "<auto>"),
                        );
                        for station_name in station_names.iter() {
                            ui.selectable_value(
                                &mut station,
                                Some(station_name.clone()),
                                station_name.as_str(),
                            );
                        }
                    });
                });
                if station != profile.station {
                    profile.station = station;
                }

                let mut model = profile.model.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("model");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut model));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        model.clear();
                    }
                });
                let next_model = non_empty_trimmed(Some(model.as_str()));
                if next_model != profile.model {
                    profile.model = next_model;
                }

                let mut effort = profile.reasoning_effort.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("reasoning_effort");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut effort));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        effort.clear();
                    }
                });
                let next_effort = non_empty_trimmed(Some(effort.as_str()));
                if next_effort != profile.reasoning_effort {
                    profile.reasoning_effort = next_effort;
                }

                let mut tier = profile.service_tier.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("service_tier");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut tier));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        tier.clear();
                    }
                });
                let next_tier = non_empty_trimmed(Some(tier.as_str()));
                if next_tier != profile.service_tier {
                    profile.service_tier = next_tier;
                }

                cols[1].add_space(6.0);
                cols[1].small(format_profile_summary(&ControlProfileOption {
                    name: profile_name.clone(),
                    station: profile.station.clone(),
                    model: profile.model.clone(),
                    reasoning_effort: profile.reasoning_effort.clone(),
                    service_tier: profile.service_tier.clone(),
                    is_default,
                }));
                cols[1].small(pick(
                    ctx.lang,
                    "提示：service_tier=priority 通常可视为 fast mode；reasoning_effort 可表达思考模式。",
                    "Tip: service_tier=priority usually maps to fast mode; reasoning_effort expresses reasoning mode.",
                ));
                let profile_preview = build_profile_route_preview(
                    profile,
                    configured_active_name.as_deref(),
                    effective_active_name.as_deref(),
                    preview_station_specs,
                    preview_provider_catalog,
                    preview_runtime_station_catalog,
                );
                render_profile_route_preview(&mut cols[1], ctx.lang, profile, &profile_preview);

                if delete_selected {
                    view.profiles.remove(profile_name.as_str());
                    if view.default_profile.as_deref() == Some(profile_name.as_str()) {
                        view.default_profile = None;
                    }
                    selected_profile_name = view
                        .default_profile
                        .clone()
                        .or_else(|| view.profiles.keys().next().cloned());
                    profile_info = Some(
                        pick(
                            ctx.lang,
                            "已删除 profile（待保存）。",
                            "Profile deleted (save pending).",
                        )
                        .to_string(),
                    );
                }
            });

            ui.add_space(6.0);
            if ui
                .button(pick(ctx.lang, "保存并应用 profile 变更", "Save & apply profile changes"))
                .clicked()
            {
                action_save_apply = true;
            }
            }
        });
    }

    ctx.view.config.selected_provider_name = selected_provider_name;
    ctx.view.config.selected_profile_name = selected_profile_name;
    ctx.view.config.new_profile_name = new_profile_name;
    ctx.view.config.station_editor.station_name = station_editor_name;
    ctx.view.config.station_editor.alias = station_editor_alias;
    ctx.view.config.station_editor.enabled = station_editor_enabled;
    ctx.view.config.station_editor.level = station_editor_level.clamp(1, 10);
    ctx.view.config.station_editor.members = station_editor_members;
    ctx.view.config.station_editor.new_station_name = new_station_name;
    ctx.view.config.provider_editor.provider_name = provider_editor_name;
    ctx.view.config.provider_editor.alias = provider_editor_alias;
    ctx.view.config.provider_editor.enabled = provider_editor_enabled;
    ctx.view.config.provider_editor.auth_token_env = provider_editor_auth_token_env;
    ctx.view.config.provider_editor.api_key_env = provider_editor_api_key_env;
    ctx.view.config.provider_editor.endpoints = provider_editor_endpoints;
    ctx.view.config.provider_editor.new_provider_name = new_provider_name;
    ctx.view.config.profile_editor.profile_name = profile_editor_name;
    ctx.view.config.profile_editor.station = profile_editor_station;
    ctx.view.config.profile_editor.model = profile_editor_model;
    ctx.view.config.profile_editor.reasoning_effort = profile_editor_reasoning_effort;
    ctx.view.config.profile_editor.service_tier = profile_editor_service_tier;
    if let Some(message) = profile_info {
        *ctx.last_info = Some(message);
    }
    if let Some(message) = profile_error {
        *ctx.last_error = Some(message);
    }

    if let Some((profile_name, profile)) = action_profile_upsert_remote {
        match ctx
            .proxy
            .upsert_persisted_profile(ctx.rt, profile_name.clone(), profile)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_profile_name = Some(profile_name);
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已写入 profile 配置并刷新代理。",
                        "Profile config saved and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!("save profile via control plane failed: {e}"));
            }
        }
    }

    if let Some(default_profile_name) = action_profile_set_persisted_default_remote {
        match ctx
            .proxy
            .set_persisted_default_profile(ctx.rt, default_profile_name.clone())
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                *ctx.last_info = Some(
                    match default_profile_name {
                        Some(_) => pick(
                            ctx.lang,
                            "已更新配置默认 profile。",
                            "Configured default profile updated.",
                        ),
                        None => pick(
                            ctx.lang,
                            "已清除配置默认 profile。",
                            "Configured default profile cleared.",
                        ),
                    }
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "set persisted default profile via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some(profile_name) = action_profile_delete_remote {
        match ctx.proxy.delete_persisted_profile(ctx.rt, profile_name) {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_profile_name = None;
                ctx.view.config.profile_editor = ConfigProfileEditorState::default();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已删除 profile 并刷新代理。",
                        "Profile deleted and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "delete persisted profile via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some(station_name) = action_set_active_remote {
        match ctx
            .proxy
            .set_persisted_active_station(ctx.rt, station_name.clone())
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                *ctx.last_info = Some(
                    match station_name {
                        Some(_) => pick(
                            ctx.lang,
                            "已更新配置 active_station 并刷新代理。",
                            "Configured active_station updated and proxy refreshed.",
                        ),
                        None => pick(
                            ctx.lang,
                            "已清除配置 active_station 并刷新代理。",
                            "Configured active_station cleared and proxy refreshed.",
                        ),
                    }
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "set persisted active station via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some((station_name, enabled, level)) = action_save_apply_remote {
        match ctx
            .proxy
            .update_persisted_station(ctx.rt, station_name, enabled, level)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已保存 station 配置并刷新代理。",
                        "Station config saved and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!("save station via control plane failed: {e}"));
            }
        }
    }

    if let Some((station_name, station_spec)) = action_upsert_station_spec_remote {
        match ctx
            .proxy
            .upsert_persisted_station_spec(ctx.rt, station_name.clone(), station_spec)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_name = Some(station_name);
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已写入 station 结构并刷新代理。",
                        "Station structure saved and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "save station structure via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some(station_name) = action_delete_station_spec_remote {
        match ctx
            .proxy
            .delete_persisted_station_spec(ctx.rt, station_name)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_name = None;
                ctx.view.config.station_editor = ConfigStationEditorState::default();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已删除 station 并刷新代理。",
                        "Station deleted and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "delete station structure via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some((provider_name, provider_spec)) = action_upsert_provider_spec_remote {
        match ctx
            .proxy
            .upsert_persisted_provider_spec(ctx.rt, provider_name.clone(), provider_spec)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_provider_name = Some(provider_name);
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已写入 provider 结构并刷新代理。",
                        "Provider structure saved and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "save provider structure via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some(provider_name) = action_delete_provider_spec_remote {
        match ctx
            .proxy
            .delete_persisted_provider_spec(ctx.rt, provider_name)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
                ctx.view.config.selected_provider_name = None;
                ctx.view.config.provider_editor = ConfigProviderEditorState::default();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已删除 provider 并刷新代理。",
                        "Provider deleted and proxy refreshed.",
                    )
                    .to_string(),
                );
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!(
                    "delete provider structure via control plane failed: {e}"
                ));
            }
        }
    }

    if let Some(name) = action_set_active {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        view.active_group = Some(name);
        *ctx.last_info =
            Some(pick(ctx.lang, "已设置 active_station", "active_station set").to_string());
    }

    if action_clear_active {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        view.active_group = None;
        *ctx.last_info =
            Some(pick(ctx.lang, "已清除 active_station", "active_station cleared").to_string());
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
            save_proxy_config_document(ctx.rt, cfg)
        };
        match save_res {
            Ok(()) => {
                let new_path = crate::config::config_file_path();
                if let Ok(t) = std::fs::read_to_string(&new_path) {
                    *ctx.proxy_config_text = t;
                }
                if let Ok(t) = std::fs::read_to_string(&new_path)
                    && let Ok(parsed) = parse_proxy_config_document(&t)
                {
                    ctx.view.config.working = Some(parsed);
                }

                if matches!(
                    ctx.proxy.kind(),
                    super::proxy_control::ProxyModeKind::Running
                        | super::proxy_control::ProxyModeKind::Attached
                ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                {
                    *ctx.last_error = Some(format!("reload runtime failed: {e}"));
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

fn config_station_member_editor_from_member(
    member: &GroupMemberRefV2,
) -> ConfigStationMemberEditorState {
    ConfigStationMemberEditorState {
        provider: member.provider.clone(),
        endpoint_names: member.endpoint_names.join(", "),
        preferred: member.preferred,
    }
}

fn parse_station_member_endpoint_names(raw: &str) -> Vec<String> {
    let mut out = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    out.dedup();
    out
}

fn build_station_spec_from_config_editor(
    station_name: &str,
    alias: &str,
    enabled: bool,
    level: u8,
    members: &[ConfigStationMemberEditorState],
) -> Result<PersistedStationSpec, String> {
    let station_name = station_name.trim();
    if station_name.is_empty() {
        return Err("station name is required".to_string());
    }

    let mut spec_members = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let provider = member.provider.trim();
        if provider.is_empty() {
            return Err(format!("member #{} provider is required", index + 1));
        }
        spec_members.push(GroupMemberRefV2 {
            provider: provider.to_string(),
            endpoint_names: parse_station_member_endpoint_names(member.endpoint_names.as_str()),
            preferred: member.preferred,
        });
    }

    Ok(PersistedStationSpec {
        name: station_name.to_string(),
        alias: non_empty_trimmed(Some(alias)),
        enabled,
        level: level.clamp(1, 10),
        members: spec_members,
    })
}

fn render_config_station_member_editor(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    provider_catalog: &BTreeMap<String, PersistedStationProviderRef>,
    members: &mut Vec<ConfigStationMemberEditorState>,
) {
    let default_provider = provider_catalog.keys().next().cloned().unwrap_or_default();

    if ui.button(pick(lang, "新增成员", "Add member")).clicked() {
        members.push(ConfigStationMemberEditorState {
            provider: default_provider,
            endpoint_names: String::new(),
            preferred: false,
        });
    }

    egui::ScrollArea::vertical()
        .id_salt(format!("config_v2_station_members_edit_{selected_service}"))
        .max_height(180.0)
        .show(ui, |ui| {
            if members.is_empty() {
                ui.label(pick(
                    lang,
                    "(无成员；可先保存空 station，再逐步补引用)",
                    "(no members yet; you can save an empty station first and fill refs later)",
                ));
                return;
            }

            let mut delete_idx = None;
            for (idx, member) in members.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{}", idx + 1));
                        ui.checkbox(&mut member.preferred, pick(lang, "preferred", "preferred"));
                        if ui.button(pick(lang, "删除", "Delete")).clicked() {
                            delete_idx = Some(idx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("provider");
                        egui::ComboBox::from_id_salt(format!(
                            "config_v2_station_member_provider_{selected_service}_{idx}"
                        ))
                        .selected_text(if member.provider.trim().is_empty() {
                            pick(lang, "<未选择>", "<unset>")
                        } else {
                            member.provider.as_str()
                        })
                        .show_ui(ui, |ui| {
                            if provider_catalog.is_empty() {
                                ui.label(pick(lang, "(无 provider)", "(no providers)"));
                            } else {
                                for provider_name in provider_catalog.keys() {
                                    ui.selectable_value(
                                        &mut member.provider,
                                        provider_name.clone(),
                                        provider_name.as_str(),
                                    );
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("endpoint_names");
                        ui.add_sized(
                            [240.0, 22.0],
                            egui::TextEdit::singleline(&mut member.endpoint_names).hint_text(pick(
                                lang,
                                "空=provider 下全部 endpoint；或填 default,hk",
                                "empty=all provider endpoints; or enter default,hk",
                            )),
                        );
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(idx) = delete_idx {
                members.remove(idx);
            }
        });
}

fn render_config_station_provider_summary(
    ui: &mut egui::Ui,
    lang: Language,
    provider_catalog: &BTreeMap<String, PersistedStationProviderRef>,
    members: &[ConfigStationMemberEditorState],
) {
    let mut provider_names = members
        .iter()
        .map(|member| member.provider.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    provider_names.sort();
    provider_names.dedup();

    if provider_names.is_empty() {
        provider_names = provider_catalog.keys().cloned().collect();
    }

    egui::ScrollArea::vertical()
        .id_salt("config_v2_station_provider_summary")
        .max_height(140.0)
        .show(ui, |ui| {
            if provider_names.is_empty() {
                ui.label(pick(lang, "(无 provider)", "(no providers)"));
                return;
            }
            for provider_name in provider_names {
                let Some(provider) = provider_catalog.get(provider_name.as_str()) else {
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 120, 40),
                        format!("missing provider: {provider_name}"),
                    );
                    continue;
                };
                ui.label(format!(
                    "{}  alias={}  endpoints={}  enabled={}",
                    provider.name,
                    provider.alias.as_deref().unwrap_or("-"),
                    provider.endpoints.len(),
                    provider.enabled
                ));
                if !provider.endpoints.is_empty() {
                    ui.small(
                        provider
                            .endpoints
                            .iter()
                            .map(|endpoint| {
                                format!(
                                    "{}={}",
                                    endpoint.name,
                                    shorten_middle(&endpoint.base_url, 48)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                }
                ui.add_space(4.0);
            }
        });
}

fn config_provider_endpoint_editor_from_spec(
    endpoint: &crate::config::PersistedProviderEndpointSpec,
) -> ConfigProviderEndpointEditorState {
    ConfigProviderEndpointEditorState {
        name: endpoint.name.clone(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
    }
}

fn build_provider_spec_from_config_editor(
    provider_name: &str,
    alias: &str,
    enabled: bool,
    auth_token_env: &str,
    api_key_env: &str,
    endpoints: &[ConfigProviderEndpointEditorState],
) -> Result<PersistedProviderSpec, String> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return Err("provider name is required".to_string());
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut spec_endpoints = Vec::new();
    for (index, endpoint) in endpoints.iter().enumerate() {
        let endpoint_name = endpoint.name.trim();
        if endpoint_name.is_empty() {
            return Err(format!("endpoint #{} name is required", index + 1));
        }
        if !seen.insert(endpoint_name.to_string()) {
            return Err(format!("duplicate endpoint name: {endpoint_name}"));
        }
        let base_url = endpoint.base_url.trim();
        if base_url.is_empty() {
            return Err(format!("endpoint '{}' base_url is required", endpoint_name));
        }
        spec_endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.to_string(),
            base_url: base_url.to_string(),
            enabled: endpoint.enabled,
        });
    }

    Ok(PersistedProviderSpec {
        name: provider_name.to_string(),
        alias: non_empty_trimmed(Some(alias)),
        enabled,
        auth_token_env: non_empty_trimmed(Some(auth_token_env)),
        api_key_env: non_empty_trimmed(Some(api_key_env)),
        endpoints: spec_endpoints,
    })
}

fn merge_provider_spec_into_provider_config(
    existing: Option<&ProviderConfigV2>,
    provider: &PersistedProviderSpec,
) -> ProviderConfigV2 {
    let mut auth = existing
        .map(|provider| provider.auth.clone())
        .unwrap_or_default();
    auth.auth_token_env = provider.auth_token_env.clone();
    auth.api_key_env = provider.api_key_env.clone();

    ProviderConfigV2 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth,
        tags: existing
            .map(|provider| provider.tags.clone())
            .unwrap_or_default(),
        supported_models: existing
            .map(|provider| provider.supported_models.clone())
            .unwrap_or_default(),
        model_mapping: existing
            .map(|provider| provider.model_mapping.clone())
            .unwrap_or_default(),
        endpoints: provider
            .endpoints
            .iter()
            .map(|endpoint| {
                let existing_endpoint =
                    existing.and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                (
                    endpoint.name.clone(),
                    ProviderEndpointV2 {
                        base_url: endpoint.base_url.clone(),
                        enabled: endpoint.enabled,
                        tags: existing_endpoint
                            .map(|endpoint| endpoint.tags.clone())
                            .unwrap_or_default(),
                        supported_models: existing_endpoint
                            .map(|endpoint| endpoint.supported_models.clone())
                            .unwrap_or_default(),
                        model_mapping: existing_endpoint
                            .map(|endpoint| endpoint.model_mapping.clone())
                            .unwrap_or_default(),
                    },
                )
            })
            .collect(),
    }
}

fn render_config_provider_endpoint_editor(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    provider_name: &str,
    endpoints: &mut Vec<ConfigProviderEndpointEditorState>,
) {
    if ui
        .button(pick(lang, "新增 endpoint", "Add endpoint"))
        .clicked()
    {
        endpoints.push(ConfigProviderEndpointEditorState {
            enabled: true,
            ..Default::default()
        });
    }

    egui::ScrollArea::vertical()
        .id_salt(format!(
            "config_v2_provider_endpoints_edit_{selected_service}_{provider_name}"
        ))
        .max_height(180.0)
        .show(ui, |ui| {
            if endpoints.is_empty() {
                ui.label(pick(
                    lang,
                    "(无 endpoint；可先保存空 provider，再逐步补地址)",
                    "(no endpoints yet; you can save an empty provider first and fill URLs later)",
                ));
                return;
            }

            let mut delete_idx = None;
            for (idx, endpoint) in endpoints.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{}", idx + 1));
                        ui.checkbox(&mut endpoint.enabled, pick(lang, "启用", "Enabled"));
                        if ui.button(pick(lang, "删除", "Delete")).clicked() {
                            delete_idx = Some(idx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("name");
                        ui.add_sized(
                            [180.0, 22.0],
                            egui::TextEdit::singleline(&mut endpoint.name),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("base_url");
                        ui.add_sized(
                            [280.0, 22.0],
                            egui::TextEdit::singleline(&mut endpoint.base_url)
                                .hint_text("https://example.com/v1"),
                        );
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(idx) = delete_idx {
                endpoints.remove(idx);
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn render_config_v2_profiles_control_plane(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    profile_catalog: &BTreeMap<String, crate::config::ServiceControlProfile>,
    configured_default_profile: Option<&str>,
    station_names: &[String],
    selected_profile_name: &mut Option<String>,
    new_profile_name: &mut String,
    editor_profile_name: &mut Option<String>,
    editor_station: &mut Option<String>,
    editor_model: &mut String,
    editor_reasoning_effort: &mut String,
    editor_service_tier: &mut String,
    profile_error: &mut Option<String>,
    action_profile_upsert_remote: &mut Option<(String, crate::config::ServiceControlProfile)>,
    action_profile_delete_remote: &mut Option<String>,
    action_profile_set_persisted_default_remote: &mut Option<Option<String>>,
    attached_mode: bool,
    station_control_plane_enabled: bool,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
    preview_station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    preview_provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    preview_runtime_station_catalog: Option<&BTreeMap<String, ConfigOption>>,
) {
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        if attached_mode {
            if station_control_plane_enabled {
                pick(
                    lang,
                    "当前 station 常用字段与下面的 Profiles 都直接管理附着代理；provider/member 深层结构仍建议在原始视图查看。",
                    "Station common fields and the Profiles below manage the attached proxy directly; use Raw view for deeper provider/member structure.",
                )
            } else {
                pick(
                    lang,
                    "下面的 Profiles 直接管理当前附着代理；上面的 station/provider 仍然是本机文件视图。",
                    "Profiles below manage the attached proxy directly; the station/provider form above still reflects the local file on this device.",
                )
            }
        } else {
            pick(
                lang,
                "下面的 Profiles 直接管理当前运行中的代理配置。",
                "Profiles below manage the currently running proxy config directly.",
            )
        },
    );

    ui.horizontal(|ui| {
        ui.label(pick(lang, "新建 profile", "New profile"));
        ui.add_sized(
            [180.0, 22.0],
            egui::TextEdit::singleline(new_profile_name).hint_text(pick(
                lang,
                "例如 fast / deep / cheap",
                "e.g. fast / deep / cheap",
            )),
        );
        if ui.button(pick(lang, "新增", "Add")).clicked() {
            let name = new_profile_name.trim();
            if name.is_empty() {
                *profile_error = Some(
                    pick(
                        lang,
                        "profile 名称不能为空。",
                        "Profile name cannot be empty.",
                    )
                    .to_string(),
                );
            } else if profile_catalog.contains_key(name) {
                *profile_error = Some(
                    pick(lang, "profile 名称已存在。", "Profile name already exists.").to_string(),
                );
            } else {
                *action_profile_upsert_remote = Some((
                    name.to_string(),
                    crate::config::ServiceControlProfile::default(),
                ));
                if configured_default_profile.is_none() {
                    *action_profile_set_persisted_default_remote = Some(Some(name.to_string()));
                }
                *selected_profile_name = Some(name.to_string());
                *editor_profile_name = Some(name.to_string());
                *editor_station = None;
                editor_model.clear();
                editor_reasoning_effort.clear();
                editor_service_tier.clear();
                new_profile_name.clear();
            }
        }
    });

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].label(pick(lang, "Profile 列表", "Profile list"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("config_v2_profiles_scroll")
            .max_height(240.0)
            .show(&mut cols[0], |ui| {
                if profile_catalog.is_empty() {
                    ui.label(pick(lang, "(当前没有 profile)", "(no profiles yet)"));
                } else {
                    for name in profile_catalog.keys() {
                        let is_selected = selected_profile_name.as_deref() == Some(name.as_str());
                        let label = if configured_default_profile == Some(name.as_str()) {
                            format!("{name} [default]")
                        } else {
                            name.clone()
                        };
                        if ui.selectable_label(is_selected, label).clicked() {
                            *selected_profile_name = Some(name.clone());
                        }
                    }
                }
            });

        if editor_profile_name.as_deref() != selected_profile_name.as_deref() {
            let selected_profile = selected_profile_name
                .as_deref()
                .and_then(|name| profile_catalog.get(name));
            *editor_profile_name = selected_profile_name.clone();
            *editor_station = selected_profile.and_then(|profile| profile.station.clone());
            *editor_model = selected_profile
                .and_then(|profile| profile.model.clone())
                .unwrap_or_default();
            *editor_reasoning_effort = selected_profile
                .and_then(|profile| profile.reasoning_effort.clone())
                .unwrap_or_default();
            *editor_service_tier = selected_profile
                .and_then(|profile| profile.service_tier.clone())
                .unwrap_or_default();
        }

        cols[1].label(pick(lang, "Profile 详情", "Profile details"));
        cols[1].add_space(4.0);

        let Some(profile_name) = selected_profile_name.clone() else {
            cols[1].label(pick(lang, "未选择 profile。", "No profile selected."));
            return;
        };

        let Some(profile) = profile_catalog.get(profile_name.as_str()) else {
            cols[1].label(pick(
                lang,
                "profile 不存在（可能已被删除）。",
                "Profile missing.",
            ));
            return;
        };
        let is_default = configured_default_profile == Some(profile_name.as_str());

        cols[1].label(format!("name: {profile_name}"));
        cols[1].label(format!(
            "{}: {}",
            pick(lang, "默认", "Default"),
            if is_default {
                pick(lang, "是", "yes")
            } else {
                pick(lang, "否", "no")
            }
        ));

        cols[1].horizontal(|ui| {
            if ui
                .button(pick(lang, "设为 default_profile", "Set default_profile"))
                .clicked()
            {
                *action_profile_set_persisted_default_remote = Some(Some(profile_name.clone()));
            }
            if ui
                .button(pick(lang, "清除 default_profile", "Clear default_profile"))
                .clicked()
                && is_default
            {
                *action_profile_set_persisted_default_remote = Some(None);
            }
            if ui.button(pick(lang, "删除 profile", "Delete profile")).clicked() {
                *action_profile_delete_remote = Some(profile_name.clone());
            }
        });

        cols[1].horizontal(|ui| {
            ui.label(pick(lang, "station", "station"));
            egui::ComboBox::from_id_salt(format!(
                "config_v2_profile_station_remote_{selected_service}_{profile_name}"
            ))
            .selected_text(
                editor_station
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<自动>", "<auto>")),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(editor_station, None, pick(lang, "<自动>", "<auto>"));
                for station_name in station_names {
                    ui.selectable_value(
                        editor_station,
                        Some(station_name.clone()),
                        station_name.as_str(),
                    );
                }
            });
        });

        cols[1].horizontal(|ui| {
            ui.label("model");
            ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(editor_model));
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_model.clear();
            }
        });

        cols[1].horizontal(|ui| {
            ui.label("reasoning_effort");
            ui.add_sized(
                [220.0, 22.0],
                egui::TextEdit::singleline(editor_reasoning_effort),
            );
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_reasoning_effort.clear();
            }
        });

        cols[1].horizontal(|ui| {
            ui.label("service_tier");
            ui.add_sized(
                [220.0, 22.0],
                egui::TextEdit::singleline(editor_service_tier),
            );
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_service_tier.clear();
            }
        });

        cols[1].add_space(6.0);
        cols[1].small(format_profile_summary(&ControlProfileOption {
            name: profile_name.clone(),
            station: editor_station.clone(),
            model: non_empty_trimmed(Some(editor_model.as_str())),
            reasoning_effort: non_empty_trimmed(Some(editor_reasoning_effort.as_str())),
            service_tier: non_empty_trimmed(Some(editor_service_tier.as_str())),
            is_default,
        }));
        cols[1].small(pick(
            lang,
            "提示：service_tier=priority 通常可视为 fast mode；reasoning_effort 可表达思考模式。",
            "Tip: service_tier=priority usually maps to fast mode; reasoning_effort expresses reasoning mode.",
        ));
        let preview_profile = crate::config::ServiceControlProfile {
            station: editor_station.clone(),
            model: non_empty_trimmed(Some(editor_model.as_str())),
            reasoning_effort: non_empty_trimmed(Some(editor_reasoning_effort.as_str())),
            service_tier: non_empty_trimmed(Some(editor_service_tier.as_str())),
        };
        let profile_preview = build_profile_route_preview(
            &preview_profile,
            configured_active_station,
            effective_active_station,
            preview_station_specs,
            preview_provider_catalog,
            preview_runtime_station_catalog,
        );
        render_profile_route_preview(&mut cols[1], lang, &preview_profile, &profile_preview);
        if editor_station != &profile.station
            || non_empty_trimmed(Some(editor_model.as_str())) != profile.model
            || non_empty_trimmed(Some(editor_reasoning_effort.as_str()))
                != profile.reasoning_effort
            || non_empty_trimmed(Some(editor_service_tier.as_str())) != profile.service_tier
        {
            cols[1].small(pick(
                lang,
                "当前编辑内容尚未写入代理配置。",
                "Current edits have not been written to the proxy config yet.",
            ));
        }
    });

    ui.add_space(6.0);
    if ui
        .button(pick(
            lang,
            "保存并应用 profile 变更",
            "Save & apply profile changes",
        ))
        .clicked()
        && let Some(profile_name) = selected_profile_name.clone()
    {
        *action_profile_upsert_remote = Some((
            profile_name,
            crate::config::ServiceControlProfile {
                station: editor_station.clone(),
                model: non_empty_trimmed(Some(editor_model.as_str())),
                reasoning_effort: non_empty_trimmed(Some(editor_reasoning_effort.as_str())),
                service_tier: non_empty_trimmed(Some(editor_service_tier.as_str())),
            },
        ));
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
                    *ctx.proxy_config_text = t.clone();
                    match parse_proxy_config_document(&t) {
                        Ok(doc) => {
                            ctx.view.config.working = Some(doc);
                            ctx.view.config.load_error = None;
                            *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            ctx.view.config.working = None;
                            ctx.view.config.load_error = Some(format!("parse failed: {e}"));
                            *ctx.last_error = Some(format!("parse failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("read config failed: {e}"));
                }
            }
        }

        if ui.button(pick(ctx.lang, "校验", "Validate")).clicked() {
            match parse_proxy_config_document(ctx.proxy_config_text) {
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
            match parse_proxy_config_document(ctx.proxy_config_text) {
                Ok(cfg) => {
                    let save = save_proxy_config_document(ctx.rt, &cfg);
                    match save {
                        Ok(()) => {
                            let new_path = crate::config::config_file_path();
                            match std::fs::read_to_string(&new_path) {
                                Ok(t) => {
                                    *ctx.proxy_config_text = t.clone();
                                    match parse_proxy_config_document(&t) {
                                        Ok(doc) => {
                                            ctx.view.config.working = Some(doc);
                                            ctx.view.config.load_error = None;
                                            *ctx.last_info =
                                                Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                            *ctx.last_error = None;
                                        }
                                        Err(e) => {
                                            ctx.view.config.working = None;
                                            ctx.view.config.load_error =
                                                Some(format!("parse failed: {e}"));
                                            *ctx.last_info =
                                                Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                            *ctx.last_error =
                                                Some(format!("re-read parse failed: {e}"));
                                        }
                                    }
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
                            ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                            {
                                *ctx.last_error = Some(format!("reload runtime failed: {e}"));
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

fn parse_proxy_config_document(text: &str) -> anyhow::Result<ConfigWorkingDocument> {
    if let Ok(value) = toml::from_str::<toml::Value>(text) {
        let version = value
            .get("version")
            .and_then(|v| v.as_integer())
            .map(|v| v as u32);
        if version == Some(2) {
            let cfg = toml::from_str::<crate::config::ProxyConfigV2>(text)?;
            crate::config::compile_v2_to_runtime(&cfg)?;
            return Ok(ConfigWorkingDocument::V2(cfg));
        }

        if let Ok(cfg) = toml::from_str::<crate::config::ProxyConfig>(text) {
            return Ok(ConfigWorkingDocument::Legacy(cfg));
        }
    }

    let v = serde_json::from_str::<crate::config::ProxyConfig>(text)?;
    Ok(ConfigWorkingDocument::Legacy(v))
}

fn save_proxy_config_document(
    rt: &tokio::runtime::Runtime,
    doc: &ConfigWorkingDocument,
) -> anyhow::Result<()> {
    match doc {
        ConfigWorkingDocument::Legacy(cfg) => rt.block_on(crate::config::save_config(cfg))?,
        ConfigWorkingDocument::V2(cfg) => {
            rt.block_on(crate::config::save_config_v2(cfg))?;
        }
    }
    Ok(())
}

fn sync_codex_auth_into_document(
    doc: &mut ConfigWorkingDocument,
    options: crate::config::SyncCodexAuthFromCodexOptions,
) -> anyhow::Result<crate::config::SyncCodexAuthFromCodexReport> {
    match doc {
        ConfigWorkingDocument::Legacy(cfg) => {
            crate::config::sync_codex_auth_from_codex_cli(cfg, options)
        }
        ConfigWorkingDocument::V2(cfg) => {
            let mut runtime = crate::config::compile_v2_to_runtime(cfg)?;
            let report = crate::config::sync_codex_auth_from_codex_cli(&mut runtime, options)?;
            *cfg =
                crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))?;
            Ok(report)
        }
    }
}

fn working_legacy_config(view: &ConfigViewState) -> Option<&crate::config::ProxyConfig> {
    match view.working.as_ref()? {
        ConfigWorkingDocument::Legacy(cfg) => Some(cfg),
        ConfigWorkingDocument::V2(_) => None,
    }
}

fn working_legacy_config_mut(
    view: &mut ConfigViewState,
) -> Option<&mut crate::config::ProxyConfig> {
    match view.working.as_mut()? {
        ConfigWorkingDocument::Legacy(cfg) => Some(cfg),
        ConfigWorkingDocument::V2(_) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRow {
    session_id: Option<String>,
    observation_scope: SessionObservationScope,
    last_client_name: Option<String>,
    last_client_addr: Option<String>,
    cwd: Option<String>,
    active_count: u64,
    active_started_at_ms_min: Option<u64>,
    last_status: Option<u16>,
    last_duration_ms: Option<u64>,
    last_ended_at_ms: Option<u64>,
    last_model: Option<String>,
    last_reasoning_effort: Option<String>,
    last_service_tier: Option<String>,
    last_provider_id: Option<String>,
    last_config_name: Option<String>,
    last_upstream_base_url: Option<String>,
    last_usage: Option<UsageMetrics>,
    total_usage: Option<UsageMetrics>,
    turns_total: Option<u64>,
    turns_with_usage: Option<u64>,
    binding_profile_name: Option<String>,
    binding_continuity_mode: Option<crate::state::SessionContinuityMode>,
    effective_model: Option<ResolvedRouteValue>,
    effective_reasoning_effort: Option<ResolvedRouteValue>,
    effective_service_tier: Option<ResolvedRouteValue>,
    effective_config_name: Option<ResolvedRouteValue>,
    effective_upstream_base_url: Option<ResolvedRouteValue>,
    override_model: Option<String>,
    override_effort: Option<String>,
    override_config_name: Option<String>,
    override_service_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveRouteField {
    Model,
    Station,
    Upstream,
    Effort,
    ServiceTier,
}

impl EffectiveRouteField {
    const ALL: [Self; 5] = [
        Self::Model,
        Self::Station,
        Self::Upstream,
        Self::Effort,
        Self::ServiceTier,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveRouteExplanation {
    value: String,
    source_label: String,
    reason: String,
}

fn build_session_rows_from_cards(cards: &[SessionIdentityCard]) -> Vec<SessionRow> {
    let mut rows = cards
        .iter()
        .map(|card| SessionRow {
            session_id: card.session_id.clone(),
            observation_scope: card.observation_scope,
            last_client_name: card.last_client_name.clone(),
            last_client_addr: card.last_client_addr.clone(),
            cwd: card.cwd.clone(),
            active_count: card.active_count,
            active_started_at_ms_min: card.active_started_at_ms_min,
            last_status: card.last_status,
            last_duration_ms: card.last_duration_ms,
            last_ended_at_ms: card.last_ended_at_ms,
            last_model: card.last_model.clone(),
            last_reasoning_effort: card.last_reasoning_effort.clone(),
            last_service_tier: card.last_service_tier.clone(),
            last_provider_id: card.last_provider_id.clone(),
            last_config_name: card.last_config_name.clone(),
            last_upstream_base_url: card.last_upstream_base_url.clone(),
            last_usage: card.last_usage.clone(),
            total_usage: card.total_usage.clone(),
            turns_total: card.turns_total,
            turns_with_usage: card.turns_with_usage,
            binding_profile_name: card.binding_profile_name.clone(),
            binding_continuity_mode: card.binding_continuity_mode,
            effective_model: card.effective_model.clone(),
            effective_reasoning_effort: card.effective_reasoning_effort.clone(),
            effective_service_tier: card.effective_service_tier.clone(),
            effective_config_name: card.effective_config_name.clone(),
            effective_upstream_base_url: card.effective_upstream_base_url.clone(),
            override_model: card.override_model.clone(),
            override_effort: card.override_effort.clone(),
            override_config_name: card.override_config_name.clone(),
            override_service_tier: card.override_service_tier.clone(),
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

fn build_session_rows(
    active: Vec<ActiveRequest>,
    recent: &[FinishedRequest],
    model_overrides: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
    config_overrides: &HashMap<String, String>,
    service_tier_overrides: &HashMap<String, String>,
    global_override: Option<&str>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionRow> {
    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionRow> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: req.client_name.clone(),
            last_client_addr: req.client_addr.clone(),
            cwd: req.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: Some(req.started_at_ms),
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: req.model.clone(),
            last_reasoning_effort: req.reasoning_effort.clone(),
            last_service_tier: req.service_tier.clone(),
            last_provider_id: req.provider_id.clone(),
            last_config_name: req.config_name.clone(),
            last_upstream_base_url: req.upstream_base_url.clone(),
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
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
        if entry.last_client_name.is_none() {
            entry.last_client_name = req.client_name;
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = req.client_addr;
        }
        if let Some(effort) = req.reasoning_effort {
            entry.last_reasoning_effort = Some(effort);
        }
        if let Some(service_tier) = req.service_tier {
            entry.last_service_tier = Some(service_tier);
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
        if entry.last_upstream_base_url.is_none() {
            entry.last_upstream_base_url = req.upstream_base_url;
        }
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: r.client_name.clone(),
            last_client_addr: r.client_addr.clone(),
            cwd: r.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: r.model.clone(),
            last_reasoning_effort: r.reasoning_effort.clone(),
            last_service_tier: r.service_tier.clone(),
            last_provider_id: r.provider_id.clone(),
            last_config_name: r.config_name.clone(),
            last_upstream_base_url: r.upstream_base_url.clone(),
            last_usage: r.usage.clone(),
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });

        let should_update = entry
            .last_ended_at_ms
            .is_none_or(|prev| r.ended_at_ms >= prev);
        if should_update {
            entry.last_status = Some(r.status_code);
            entry.last_duration_ms = Some(r.duration_ms);
            entry.last_ended_at_ms = Some(r.ended_at_ms);
            entry.last_client_name = r.client_name.clone().or(entry.last_client_name.clone());
            entry.last_client_addr = r.client_addr.clone().or(entry.last_client_addr.clone());
            entry.last_model = r.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = r
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_service_tier = r.service_tier.clone().or(entry.last_service_tier.clone());
            entry.last_provider_id = r.provider_id.clone().or(entry.last_provider_id.clone());
            entry.last_config_name = r.config_name.clone().or(entry.last_config_name.clone());
            entry.last_upstream_base_url = r
                .upstream_base_url
                .clone()
                .or(entry.last_upstream_base_url.clone());
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
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });

        if entry.turns_total.is_none() {
            entry.turns_total = Some(st.turns_total);
        }
        if entry.last_client_name.is_none() {
            entry.last_client_name = st.last_client_name.clone();
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = st.last_client_addr.clone();
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
        if entry.last_service_tier.is_none() {
            entry.last_service_tier = st.last_service_tier.clone();
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

    for (sid, model) in model_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });
        entry.override_model = Some(model.clone());
    }

    for (sid, eff) in overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });
        entry.override_effort = Some(eff.clone());
    }

    for (sid, cfg_name) in config_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });
        entry.override_config_name = Some(cfg_name.clone());
    }

    for (sid, service_tier) in service_tier_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        });
        entry.override_service_tier = Some(service_tier.clone());
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    for row in &mut rows {
        if row.cwd.is_some() {
            row.observation_scope = SessionObservationScope::HostLocalEnriched;
        }
        apply_effective_route_to_row(row, global_override);
    }
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn format_observed_client_identity(
    client_name: Option<&str>,
    client_addr: Option<&str>,
) -> Option<String> {
    match (
        non_empty_trimmed(client_name),
        non_empty_trimmed(client_addr),
    ) {
        (Some(name), Some(addr)) => Some(format!("{name} @ {addr}")),
        (Some(name), None) => Some(name),
        (None, Some(addr)) => Some(addr),
        (None, None) => None,
    }
}

fn session_observation_scope_short_label(
    lang: Language,
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "obs", "obs"),
        SessionObservationScope::HostLocalEnriched => pick(lang, "host", "host"),
    }
}

fn session_observation_scope_label(lang: Language, scope: SessionObservationScope) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "仅共享观测", "Observed only"),
        SessionObservationScope::HostLocalEnriched => {
            pick(lang, "代理主机 enrich", "Host-local enriched")
        }
    }
}

fn resolve_effective_observed_value(
    override_value: Option<&str>,
    observed_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(override_value) {
        return Some(ResolvedRouteValue {
            value,
            source: RouteValueSource::SessionOverride,
        });
    }
    non_empty_trimmed(observed_value).map(|value| ResolvedRouteValue {
        value,
        source: RouteValueSource::RequestPayload,
    })
}

fn apply_effective_route_to_row(row: &mut SessionRow, global_override: Option<&str>) {
    row.effective_model =
        resolve_effective_observed_value(row.override_model.as_deref(), row.last_model.as_deref());
    row.effective_reasoning_effort = resolve_effective_observed_value(
        row.override_effort.as_deref(),
        row.last_reasoning_effort.as_deref(),
    );
    row.effective_service_tier = resolve_effective_observed_value(
        row.override_service_tier.as_deref(),
        row.last_service_tier.as_deref(),
    );
    row.effective_config_name =
        if let Some(value) = non_empty_trimmed(row.override_config_name.as_deref()) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::SessionOverride,
            })
        } else if let Some(value) = non_empty_trimmed(global_override) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::GlobalOverride,
            })
        } else {
            non_empty_trimmed(row.last_config_name.as_deref()).map(|value| ResolvedRouteValue {
                value,
                source: RouteValueSource::RuntimeFallback,
            })
        };
    row.effective_upstream_base_url = match (
        row.effective_config_name.as_ref(),
        non_empty_trimmed(row.last_config_name.as_deref()),
        non_empty_trimmed(row.last_upstream_base_url.as_deref()),
    ) {
        (Some(config), Some(last_config), Some(upstream)) if config.value == last_config => {
            Some(ResolvedRouteValue {
                value: upstream,
                source: RouteValueSource::RuntimeFallback,
            })
        }
        _ => None,
    };
}

fn route_value_source_label(source: RouteValueSource, lang: Language) -> &'static str {
    match source {
        RouteValueSource::RequestPayload => pick(lang, "请求体", "request payload"),
        RouteValueSource::SessionOverride => pick(lang, "会话覆盖", "session override"),
        RouteValueSource::GlobalOverride => pick(lang, "全局覆盖", "global override"),
        RouteValueSource::ProfileDefault => pick(lang, "profile 默认", "profile default"),
        RouteValueSource::StationMapping => pick(lang, "站点映射", "station mapping"),
        RouteValueSource::RuntimeFallback => pick(lang, "运行时兜底", "runtime fallback"),
    }
}

fn format_resolved_route_value(value: Option<&ResolvedRouteValue>, lang: Language) -> String {
    match value {
        Some(value) => format!(
            "{} [{}]",
            value.value,
            route_value_source_label(value.source, lang)
        ),
        None => "-".to_string(),
    }
}

fn unresolved_route_source_label(lang: Language) -> &'static str {
    pick(lang, "未解析", "unresolved")
}

fn effective_route_field_label(field: EffectiveRouteField, lang: Language) -> &'static str {
    match field {
        EffectiveRouteField::Model => pick(lang, "模型", "model"),
        EffectiveRouteField::Station => pick(lang, "站点", "station"),
        EffectiveRouteField::Upstream => "upstream",
        EffectiveRouteField::Effort => pick(lang, "思考强度", "effort"),
        EffectiveRouteField::ServiceTier => "service_tier",
    }
}

fn effective_route_field_value(
    row: &SessionRow,
    field: EffectiveRouteField,
) -> Option<&ResolvedRouteValue> {
    match field {
        EffectiveRouteField::Model => row.effective_model.as_ref(),
        EffectiveRouteField::Station => row.effective_config_name.as_ref(),
        EffectiveRouteField::Upstream => row.effective_upstream_base_url.as_ref(),
        EffectiveRouteField::Effort => row.effective_reasoning_effort.as_ref(),
        EffectiveRouteField::ServiceTier => row.effective_service_tier.as_ref(),
    }
}

fn binding_profile_reference(row: &SessionRow, lang: Language) -> String {
    match row.binding_profile_name.as_deref() {
        Some(name) => format!("profile {name}"),
        None => pick(lang, "当前绑定 profile", "the bound profile").to_string(),
    }
}

fn runtime_fallback_explanation(
    row: &SessionRow,
    field: EffectiveRouteField,
    value: &ResolvedRouteValue,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => match row.last_config_name.as_deref() {
            Some(last_config) if last_config == value.value => pick(
                lang,
                "当前没有 session pin、global pin 或 profile 默认，沿用最近观测到的站点。",
                "No session pin, global pin, or profile default applies, so the station falls back to the most recently observed value.",
            )
            .to_string(),
            Some(last_config) => format!(
                "{} {}；{} {}。",
                pick(
                    lang,
                    "当前没有 session pin、global pin 或 profile 默认，运行态把站点回填为",
                    "No session pin, global pin, or profile default applies, so runtime filled the station as",
                ),
                value.value,
                pick(
                    lang,
                    "最近观测到的站点仍是",
                    "while the most recently observed station is still",
                ),
                last_config
            ),
            None => format!(
                "{} {}。",
                pick(
                    lang,
                    "当前没有更明确的站点来源，运行态回填为",
                    "No more explicit station source is available, so runtime filled it as",
                ),
                value.value
            ),
        },
        EffectiveRouteField::Upstream => {
            let effective_station = row
                .effective_config_name
                .as_ref()
                .map(|resolved| resolved.value.as_str());
            match (
                effective_station,
                row.last_config_name.as_deref(),
                row.last_upstream_base_url.as_deref(),
            ) {
                (Some(station), Some(last_config), Some(last_upstream))
                    if station == last_config && last_upstream == value.value =>
                {
                    format!(
                        "{} {}，{} {}。",
                        pick(
                            lang,
                            "当前生效站点与最近观测一致，沿用该站点最近命中的 upstream",
                            "The effective station matches the last observed station, so the upstream falls back to the most recently observed target",
                        ),
                        value.value,
                        pick(lang, "所属站点", "for station"),
                        station
                    )
                }
                (Some(station), _, _) => format!(
                    "{} {}，{} {}。",
                    pick(
                        lang,
                        "当前站点可在运行态唯一补全 upstream",
                        "The current station can be completed to a single upstream at runtime",
                    ),
                    value.value,
                    pick(lang, "所属站点", "for station"),
                    station
                ),
                _ => format!(
                    "{} {}。",
                    pick(
                        lang,
                        "运行态补全了当前 upstream",
                        "Runtime completed the current upstream as",
                    ),
                    value.value
                ),
            }
        }
        _ => format!(
            "{} {}，{}。",
            pick(
                lang,
                "当前没有更高优先级的覆盖或默认值，沿用最近观测到的",
                "No higher-priority override or default applies, so the field falls back to the most recently observed",
            ),
            effective_route_field_label(field, lang),
            value.value
        ),
    }
}

fn unresolved_effective_route_reason(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => pick(
            lang,
            "当前没有 session pin、global pin、profile 默认，也没有最近可用的站点记录。",
            "There is no session pin, global pin, profile default, or recent station observation to resolve the current station.",
        )
        .to_string(),
        EffectiveRouteField::Upstream => {
            let effective_station = row
                .effective_config_name
                .as_ref()
                .map(|resolved| resolved.value.as_str());
            match (effective_station, row.last_config_name.as_deref()) {
                (Some(station), Some(last_station))
                    if station != last_station && row.last_upstream_base_url.is_some() =>
                {
                    format!(
                        "{} {}，{} {}，{}。",
                        pick(
                            lang,
                            "当前生效站点已经切到",
                            "The effective station has already switched to",
                        ),
                        station,
                        pick(
                            lang,
                            "但最近观测到的 upstream 仍属于站点",
                            "but the most recently observed upstream still belongs to station",
                        ),
                        last_station,
                        pick(
                            lang,
                            "所以不能直接把它当成当前 upstream",
                            "so it cannot be treated as the current upstream",
                        )
                    )
                }
                (Some(station), _) => format!(
                    "{} {}，{}。",
                    pick(
                        lang,
                        "当前站点是",
                        "The current station is",
                    ),
                    station,
                    pick(
                        lang,
                        "但缺少最近 upstream 观测或唯一映射，因此暂时无法解释 upstream",
                        "but there is no recent upstream observation or unique mapping, so the upstream cannot be explained yet",
                    )
                ),
                (None, _) => pick(
                    lang,
                    "当前连 effective station 都还没有判定，因此无法解释 upstream。",
                    "The effective station itself is still unresolved, so the upstream cannot be explained.",
                )
                .to_string(),
            }
        }
        _ => format!(
            "{} {}。",
            pick(
                lang,
                "当前既没有覆盖、profile 默认，也没有最近请求值，无法判定",
                "There is no override, profile default, or recent request value to resolve",
            ),
            effective_route_field_label(field, lang)
        ),
    }
}

fn explain_effective_route_field(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> EffectiveRouteExplanation {
    let value = effective_route_field_value(row, field);
    let value_label = value
        .map(|resolved| resolved.value.clone())
        .unwrap_or_else(|| "-".to_string());
    let source_label = value
        .map(|resolved| route_value_source_label(resolved.source, lang).to_string())
        .unwrap_or_else(|| unresolved_route_source_label(lang).to_string());
    let field_label = effective_route_field_label(field, lang);

    let reason = match value {
        Some(resolved) => match resolved.source {
            RouteValueSource::SessionOverride => format!(
                "{} {}={}，{}。",
                pick(
                    lang,
                    "当前 session 显式覆盖了",
                    "The current session explicitly overrides",
                ),
                field_label,
                resolved.value,
                pick(
                    lang,
                    "因此它优先于其他来源生效",
                    "so it takes priority over every other source",
                )
            ),
            RouteValueSource::GlobalOverride => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前 session 没有单独站点覆盖，命中了全局 pin，当前站点固定为",
                    "The current session has no dedicated station override and therefore follows the global pin to",
                ),
                resolved.value,
                pick(
                    lang,
                    "所以这里以全局结果为准",
                    "so the global choice is authoritative here",
                )
            ),
            RouteValueSource::ProfileDefault => format!(
                "{} {}，{} {}={}。",
                pick(
                    lang,
                    "当前 session 绑定到",
                    "The current session is bound to",
                ),
                binding_profile_reference(row, lang),
                pick(lang, "其默认", "whose default",),
                field_label,
                resolved.value
            ),
            RouteValueSource::RequestPayload => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前没有 session override 或 profile 默认，沿用最近请求体里的",
                    "There is no session override or profile default, so the field follows the latest request payload for",
                ),
                field_label,
                resolved.value
            ),
            RouteValueSource::StationMapping => {
                let requested_model = row.last_model.as_deref().unwrap_or("-");
                let station = row
                    .effective_config_name
                    .as_ref()
                    .map(|resolved| resolved.value.as_str())
                    .or(row.last_config_name.as_deref())
                    .unwrap_or("-");
                let upstream = row.last_upstream_base_url.as_deref().unwrap_or("-");
                format!(
                    "{} {}，{} {} / {} {}，{} {}。",
                    pick(
                        lang,
                        "最近请求提交的模型是",
                        "The most recent request submitted model",
                    ),
                    requested_model,
                    pick(lang, "但站点", "but station"),
                    station,
                    pick(lang, "upstream", "upstream"),
                    upstream,
                    pick(
                        lang,
                        "的 model mapping 将实际模型改写为",
                        "rewrote the effective model through model mapping to",
                    ),
                    resolved.value
                )
            }
            RouteValueSource::RuntimeFallback => {
                runtime_fallback_explanation(row, field, resolved, lang)
            }
        },
        None => unresolved_effective_route_reason(row, field, lang),
    };

    EffectiveRouteExplanation {
        value: value_label,
        source_label,
        reason,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn history_workdir_from_cwd(cwd: &str, infer_git_root: bool) -> String {
    let trimmed = cwd.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return "-".to_string();
    }
    if infer_git_root {
        crate::sessions::infer_project_root_from_cwd(trimmed)
    } else {
        trimmed.to_string()
    }
}

fn path_mtime_ms(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn session_summary_sort_key_ms(summary: &SessionSummary) -> u64 {
    summary
        .sort_hint_ms
        .unwrap_or_else(|| path_mtime_ms(summary.path.as_path()))
}

fn sort_session_summaries_by_mtime_desc(list: &mut [SessionSummary]) {
    list.sort_by_key(|s| std::cmp::Reverse(session_summary_sort_key_ms(s)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WtItemSkipReason {
    ObservedOnly,
    MissingCwd,
    InvalidWorkdir,
    WorkdirNotFound,
}

fn workdir_status_from_cwd(
    cwd: Option<&str>,
    infer_git_root: bool,
) -> Result<String, WtItemSkipReason> {
    let Some(cwd) = cwd else {
        return Err(WtItemSkipReason::MissingCwd);
    };
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "-" {
        return Err(WtItemSkipReason::MissingCwd);
    }

    let workdir = history_workdir_from_cwd(cwd, infer_git_root);
    let w = workdir.trim();
    if w.is_empty() || w == "-" {
        return Err(WtItemSkipReason::InvalidWorkdir);
    }
    if !std::path::Path::new(w).exists() {
        return Err(WtItemSkipReason::WorkdirNotFound);
    }
    Ok(workdir)
}

pub(super) fn workdir_status_from_summary(
    summary: &SessionSummary,
    infer_git_root: bool,
) -> Result<String, WtItemSkipReason> {
    if !matches!(summary.source, SessionSummarySource::LocalFile) {
        return Err(WtItemSkipReason::ObservedOnly);
    }
    workdir_status_from_cwd(summary.cwd.as_deref(), infer_git_root)
}

fn build_wt_items_from_session_summaries<'a, I>(
    sessions: I,
    infer_git_root: bool,
    resume_cmd_template: &str,
) -> Vec<(String, String)>
where
    I: IntoIterator<Item = &'a SessionSummary>,
{
    let mut out = Vec::new();
    let t = resume_cmd_template.trim();
    for s in sessions.into_iter() {
        let Ok(workdir) = workdir_status_from_summary(s, infer_git_root) else {
            continue;
        };

        let sid = s.id.as_str();
        let cmd = if t.is_empty() {
            format!("codex resume {sid}")
        } else if t.contains("{id}") {
            t.replace("{id}", sid)
        } else {
            format!("{t} {sid}")
        };
        out.push((workdir, cmd));
    }
    out
}

fn open_wt_items(ctx: &mut PageCtx<'_>, items: Vec<(String, String)>) {
    if !cfg!(windows) {
        *ctx.last_error = Some(pick(ctx.lang, "仅支持 Windows", "Windows only").to_string());
        return;
    }

    if items.is_empty() {
        *ctx.last_error = Some(
            pick(
                ctx.lang,
                "没有可打开的会话（cwd 不可用或目录不存在）",
                "No sessions to open (cwd unavailable or missing)",
            )
            .to_string(),
        );
        return;
    }

    let mode = ctx
        .gui_cfg
        .history
        .wt_batch_mode
        .trim()
        .to_ascii_lowercase();
    let shell = ctx.view.history.shell.trim();
    let keep_open = ctx.view.history.keep_open;

    let result = if mode == "windows" {
        let mut last_err: Option<anyhow::Error> = None;
        for (cwd, cmd) in items.iter() {
            if let Err(e) =
                spawn_windows_terminal_wt_new_tab(-1, cwd.as_str(), shell, keep_open, cmd.as_str())
            {
                last_err = Some(e);
                break;
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    } else {
        spawn_windows_terminal_wt_tabs_in_one_window(&items, shell, keep_open)
    };

    match result {
        Ok(()) => {
            *ctx.last_info = Some(
                pick(
                    ctx.lang,
                    "已启动 Windows Terminal",
                    "Started Windows Terminal",
                )
                .to_string(),
            );
        }
        Err(e) => {
            *ctx.last_error = Some(format!("spawn wt failed: {e}"));
        }
    }
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
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
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

#[derive(Debug, Default)]
struct RuntimeStationMaps {
    config_health: HashMap<String, ConfigHealth>,
    health_checks: HashMap<String, HealthCheckStatus>,
    lb_view: HashMap<String, LbConfigView>,
}

fn runtime_station_maps(proxy: &super::proxy_control::ProxyController) -> RuntimeStationMaps {
    match proxy.kind() {
        ProxyModeKind::Running => proxy
            .running()
            .map(|running| RuntimeStationMaps {
                config_health: running.config_health.clone(),
                health_checks: running.health_checks.clone(),
                lb_view: running.lb_view.clone(),
            })
            .unwrap_or_default(),
        ProxyModeKind::Attached => proxy
            .attached()
            .map(|attached| RuntimeStationMaps {
                config_health: attached.config_health.clone(),
                health_checks: attached.health_checks.clone(),
                lb_view: attached.lb_view.clone(),
            })
            .unwrap_or_default(),
        _ => RuntimeStationMaps::default(),
    }
}

fn current_runtime_active_station(proxy: &super::proxy_control::ProxyController) -> Option<String> {
    let snapshot = proxy.snapshot()?;
    snapshot
        .effective_active_station
        .or(snapshot.configured_active_station)
}

fn refresh_config_editor_from_disk_if_running(ctx: &mut PageCtx<'_>) {
    if !matches!(ctx.proxy.kind(), ProxyModeKind::Running) {
        return;
    }
    let new_path = crate::config::config_file_path();
    if let Ok(text) = std::fs::read_to_string(&new_path) {
        *ctx.proxy_config_text = text.clone();
        if let Ok(parsed) = parse_proxy_config_document(&text) {
            ctx.view.config.working = Some(parsed);
        }
    }
}

fn format_runtime_station_health_status(
    health: Option<&ConfigHealth>,
    status: Option<&HealthCheckStatus>,
) -> String {
    if let Some(status) = status {
        if !status.done {
            return if status.cancel_requested {
                format!("cancel {}/{}", status.completed, status.total.max(1))
            } else {
                format!("run {}/{}", status.completed, status.total.max(1))
            };
        }
        if status.canceled {
            return "canceled".to_string();
        }
    }

    let Some(health) = health else {
        return "-".to_string();
    };
    if health.upstreams.is_empty() {
        return format!("0/0 @{}", health.checked_at_ms);
    }
    let ok = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .count();
    let best_ms = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .filter_map(|upstream| upstream.latency_ms)
        .min();
    if ok > 0 {
        if let Some(latency_ms) = best_ms {
            format!("{ok}/{} {latency_ms}ms", health.upstreams.len())
        } else {
            format!("{ok}/{} ok", health.upstreams.len())
        }
    } else {
        let code = health
            .upstreams
            .iter()
            .filter_map(|upstream| upstream.status_code)
            .next();
        match code {
            Some(code) => format!("err {code}"),
            None => "err".to_string(),
        }
    }
}

fn format_runtime_lb_summary(lb: Option<&LbConfigView>) -> String {
    let Some(lb) = lb else {
        return "-".to_string();
    };
    let cooldowns = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.cooldown_remaining_secs.is_some())
        .count();
    let exhausted = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.usage_exhausted)
        .count();
    let failures: u32 = lb
        .upstreams
        .iter()
        .map(|upstream| upstream.failure_count)
        .sum();

    if cooldowns == 0 && exhausted == 0 && failures == 0 {
        return "-".to_string();
    }

    format!("cd={cooldowns} fail={failures} quota={exhausted}")
}

fn runtime_config_state_label(lang: Language, state: RuntimeConfigState) -> &'static str {
    match (lang, state) {
        (Language::Zh, RuntimeConfigState::Normal) => "normal",
        (Language::Zh, RuntimeConfigState::Draining) => "draining",
        (Language::Zh, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (_, RuntimeConfigState::Normal) => "normal",
        (_, RuntimeConfigState::Draining) => "draining",
        (_, RuntimeConfigState::BreakerOpen) => "breaker_open",
    }
}

fn capability_support_short_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "是",
        (Language::Zh, CapabilitySupport::Unsupported) => "否",
        (Language::Zh, CapabilitySupport::Unknown) => "?",
        (_, CapabilitySupport::Supported) => "yes",
        (_, CapabilitySupport::Unsupported) => "no",
        (_, CapabilitySupport::Unknown) => "?",
    }
}

fn capability_support_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "支持",
        (Language::Zh, CapabilitySupport::Unsupported) => "不支持",
        (Language::Zh, CapabilitySupport::Unknown) => "未知",
        (_, CapabilitySupport::Supported) => "supported",
        (_, CapabilitySupport::Unsupported) => "unsupported",
        (_, CapabilitySupport::Unknown) => "unknown",
    }
}

fn format_runtime_config_capability_label(
    lang: Language,
    capabilities: &ConfigCapabilitySummary,
) -> String {
    let model_label = match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => {
            format!("{}:any", pick(lang, "模型", "models"))
        }
        ModelCatalogKind::Declared => {
            format!(
                "{}:{}",
                pick(lang, "模型", "models"),
                capabilities.supported_models.len()
            )
        }
    };
    format!(
        "{model_label} | tier:{} | effort:{}",
        capability_support_short_label(lang, capabilities.supports_service_tier),
        capability_support_short_label(lang, capabilities.supports_reasoning_effort),
    )
}

fn runtime_config_capability_hover_text(
    lang: Language,
    capabilities: &ConfigCapabilitySummary,
) -> String {
    let mut lines = Vec::new();
    match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => lines.push(
            pick(
                lang,
                "模型能力: 未显式声明，当前按 implicit any 处理",
                "Model support: not declared explicitly; current routing treats this station as implicit-any",
            )
            .to_string(),
        ),
        ModelCatalogKind::Declared => {
            if capabilities.supported_models.is_empty() {
                lines.push(
                    pick(
                        lang,
                        "模型能力: 已声明，但没有正向可用模型模式",
                        "Model support: declared, but no positive model patterns are available",
                    )
                    .to_string(),
                );
            } else {
                lines.push(format!(
                    "{}: {}",
                    pick(lang, "模型列表", "Models"),
                    capabilities.supported_models.join(", ")
                ));
            }
        }
    }
    lines.push(format!(
        "{}: {}",
        pick(lang, "Fast/Service tier", "Fast/Service tier"),
        capability_support_label(lang, capabilities.supports_service_tier)
    ));
    lines.push(format!(
        "{}: {}",
        pick(lang, "思考强度", "Reasoning effort"),
        capability_support_label(lang, capabilities.supports_reasoning_effort)
    ));
    lines.push(
        pick(
            lang,
            "来源: supported_models/model_mapping 与 upstream tags",
            "Source: supported_models/model_mapping plus upstream tags",
        )
        .to_string(),
    );
    lines.join("\n")
}

fn format_runtime_config_source(lang: Language, cfg: &ConfigOption) -> String {
    let mut parts = Vec::new();
    if let Some(enabled) = cfg.runtime_enabled_override {
        parts.push(format!(
            "{}={}",
            pick(lang, "启用", "enabled"),
            if enabled { "rt" } else { "rt-off" }
        ));
    }
    if cfg.runtime_level_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "等级", "level")));
    }
    if cfg.runtime_state_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "状态", "state")));
    }
    if parts.is_empty() {
        pick(lang, "配置文件", "config").to_string()
    } else {
        parts.join(", ")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session_row() -> SessionRow {
        SessionRow {
            session_id: Some("sid-1".to_string()),
            observation_scope: SessionObservationScope::ObservedOnly,
            last_client_name: None,
            last_client_addr: None,
            cwd: Some("G:/codes/rust/codex-helper".to_string()),
            active_count: 0,
            active_started_at_ms_min: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_config_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_config_name: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_config_name: None,
            override_service_tier: None,
        }
    }

    #[test]
    fn explain_effective_route_uses_profile_context() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.effective_service_tier = Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::ProfileDefault,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::ServiceTier, Language::Zh);

        assert_eq!(explanation.value, "priority");
        assert_eq!(explanation.source_label, "profile 默认");
        assert!(explanation.reason.contains("profile fast"));
        assert!(explanation.reason.contains("service_tier"));
    }

    #[test]
    fn explain_effective_route_handles_station_mapping_for_model() {
        let mut row = sample_session_row();
        row.last_model = Some("gpt-5.4".to_string());
        row.last_config_name = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_config_name = Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        });
        row.effective_model = Some(ResolvedRouteValue {
            value: "gpt-5.4-fast".to_string(),
            source: RouteValueSource::StationMapping,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::Model, Language::Zh);

        assert_eq!(explanation.source_label, "站点映射");
        assert!(explanation.reason.contains("gpt-5.4"));
        assert!(explanation.reason.contains("right"));
        assert!(explanation.reason.contains("gpt-5.4-fast"));
    }

    #[test]
    fn explain_effective_route_marks_upstream_unresolved_after_station_switch() {
        let mut row = sample_session_row();
        row.last_config_name = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_config_name = Some(ResolvedRouteValue {
            value: "vibe".to_string(),
            source: RouteValueSource::GlobalOverride,
        });

        let explanation =
            explain_effective_route_field(&row, EffectiveRouteField::Upstream, Language::Zh);

        assert_eq!(explanation.value, "-");
        assert_eq!(explanation.source_label, "未解析");
        assert!(explanation.reason.contains("vibe"));
        assert!(explanation.reason.contains("right"));
    }

    #[test]
    fn management_base_url_loopback_detection_handles_localhosts() {
        assert!(management_base_url_is_loopback("http://127.0.0.1:3211"));
        assert!(management_base_url_is_loopback("http://localhost:3211"));
        assert!(management_base_url_is_loopback("http://[::1]:3211"));
        assert!(!management_base_url_is_loopback("http://100.79.12.5:3211"));
        assert!(!management_base_url_is_loopback(
            "https://relay.example.com/admin"
        ));
    }

    #[test]
    fn attached_host_local_session_features_require_loopback_and_capability() {
        assert!(attached_host_local_session_features_available(
            "http://127.0.0.1:3211",
            true,
        ));
        assert!(!attached_host_local_session_features_available(
            "http://127.0.0.1:3211",
            false,
        ));
        assert!(!attached_host_local_session_features_available(
            "http://100.79.12.5:3211",
            true,
        ));
        assert!(!attached_host_local_session_features_available(
            "https://relay.example.com/admin",
            true,
        ));
    }

    #[test]
    fn remote_local_only_warning_absent_for_loopback_attach() {
        let warning = remote_local_only_warning_message(
            "http://127.0.0.1:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: true,
                cwd_enrichment: true,
            },
            Language::Zh,
            &["cwd", "transcript"],
        );
        assert!(warning.is_none());
    }

    #[test]
    fn remote_local_only_warning_mentions_host_only_capabilities() {
        let warning = remote_local_only_warning_message(
            "http://100.79.12.5:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: false,
                cwd_enrichment: true,
            },
            Language::En,
            &["cwd", "transcript"],
        )
        .expect("remote warning");
        assert!(warning.contains("cwd / transcript"));
        assert!(warning.contains("session history / cwd enrichment"));
        assert!(warning.contains("proxy host"));
    }

    #[test]
    fn local_profile_preview_catalogs_from_text_extracts_v2_station_provider_structure() {
        let text = r#"
version = 2

[codex]
active_station = "primary"

[codex.providers.right]
alias = "Right"
[codex.providers.right.auth]
auth_token_env = "RIGHT_API_KEY"
[codex.providers.right.endpoints.main]
base_url = "https://right.example.com/v1"

[codex.stations.primary]
alias = "Primary"
level = 3

[[codex.stations.primary.members]]
provider = "right"
preferred = true
"#;

        let (stations, providers) =
            local_profile_preview_catalogs_from_text(text, "codex").expect("catalog");

        let station = stations.get("primary").expect("primary station");
        assert_eq!(station.alias.as_deref(), Some("Primary"));
        assert_eq!(station.level, 3);
        assert_eq!(station.members.len(), 1);
        assert_eq!(station.members[0].provider, "right");

        let provider = providers.get("right").expect("right provider");
        assert_eq!(provider.alias.as_deref(), Some("Right"));
        assert_eq!(provider.endpoints.len(), 1);
        assert_eq!(provider.endpoints[0].name, "main");
    }

    #[test]
    fn build_profile_route_preview_resolves_station_source_in_order() {
        let explicit = build_profile_route_preview(
            &crate::config::ServiceControlProfile {
                station: Some("beta".to_string()),
                ..Default::default()
            },
            Some("alpha"),
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(
            explicit.station_source,
            ProfilePreviewStationSource::Profile
        );
        assert_eq!(explicit.resolved_station_name.as_deref(), Some("beta"));

        let configured = build_profile_route_preview(
            &crate::config::ServiceControlProfile::default(),
            Some("alpha"),
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(
            configured.station_source,
            ProfilePreviewStationSource::ConfiguredActive
        );
        assert_eq!(configured.resolved_station_name.as_deref(), Some("alpha"));

        let auto = build_profile_route_preview(
            &crate::config::ServiceControlProfile::default(),
            None,
            Some("gamma"),
            None,
            None,
            None,
        );
        assert_eq!(auto.station_source, ProfilePreviewStationSource::Auto);
        assert_eq!(auto.resolved_station_name.as_deref(), Some("gamma"));
    }

    #[test]
    fn build_profile_route_preview_collects_member_routes_and_capability_checks() {
        let station_specs = BTreeMap::from([(
            "primary".to_string(),
            PersistedStationSpec {
                name: "primary".to_string(),
                alias: Some("Primary".to_string()),
                enabled: true,
                level: 2,
                members: vec![GroupMemberRefV2 {
                    provider: "right".to_string(),
                    endpoint_names: Vec::new(),
                    preferred: true,
                }],
            },
        )]);
        let provider_catalog = BTreeMap::from([(
            "right".to_string(),
            PersistedStationProviderRef {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: true,
                endpoints: vec![
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "hk".to_string(),
                        base_url: "https://hk.example.com/v1".to_string(),
                        enabled: true,
                    },
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "us".to_string(),
                        base_url: "https://us.example.com/v1".to_string(),
                        enabled: true,
                    },
                ],
            },
        )]);
        let runtime_catalog = BTreeMap::from([(
            "primary".to_string(),
            ConfigOption {
                name: "primary".to_string(),
                alias: Some("Primary".to_string()),
                enabled: true,
                level: 2,
                configured_enabled: true,
                configured_level: 2,
                runtime_enabled_override: None,
                runtime_level_override: None,
                runtime_state: RuntimeConfigState::Normal,
                runtime_state_override: None,
                capabilities: ConfigCapabilitySummary {
                    model_catalog_kind: ModelCatalogKind::Declared,
                    supported_models: vec!["gpt-5.4".to_string()],
                    supports_service_tier: CapabilitySupport::Supported,
                    supports_reasoning_effort: CapabilitySupport::Unsupported,
                },
            },
        )]);
        let preview = build_profile_route_preview(
            &crate::config::ServiceControlProfile {
                station: Some("primary".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("priority".to_string()),
            },
            None,
            None,
            Some(&station_specs),
            Some(&provider_catalog),
            Some(&runtime_catalog),
        );

        assert!(preview.station_exists);
        assert_eq!(preview.station_alias.as_deref(), Some("Primary"));
        assert_eq!(preview.members.len(), 1);
        assert!(preview.members[0].uses_all_endpoints);
        assert_eq!(
            preview.members[0].endpoint_names,
            vec!["hk".to_string(), "us".to_string()]
        );
        assert_eq!(preview.model_supported, Some(true));
        assert_eq!(preview.service_tier_supported, Some(true));
        assert_eq!(preview.reasoning_supported, Some(false));
    }
}
