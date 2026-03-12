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
    CapabilitySupport, ControlProfileOption, HostLocalControlPlaneCapabilities, ModelCatalogKind,
    RemoteAdminAccessCapabilities, StationCapabilitySummary, StationOption,
};
use crate::doctor::{DoctorLang, DoctorStatus};
use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, RuntimeConfigState, SessionContinuityMode,
    SessionIdentityCard, SessionObservationScope, SessionStats, StationHealth,
};
use crate::usage::UsageMetrics;

mod components;
mod config_legacy;
mod config_raw;
mod config_v2;
mod doctor;
mod history;
mod overview;
mod requests;
mod session_presentation;
mod settings;
mod setup;
mod stats;
mod stations;
mod sessions;

pub use history::HistoryViewState;
use session_presentation::*;

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
    extends: Option<String>,
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
    pub focused_session_id: Option<String>,
    pub selected_idx: usize,
}

impl Default for RequestsViewState {
    fn default() -> Self {
        Self {
            errors_only: false,
            scope_session: true,
            focused_session_id: None,
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

#[derive(Debug, Clone, Copy)]
struct NavItemDef {
    page: Page,
    zh: &'static str,
    en: &'static str,
}

impl NavItemDef {
    fn label(self, lang: Language) -> &'static str {
        match lang {
            Language::Zh => self.zh,
            Language::En => self.en,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct NavGroupDef {
    title_zh: &'static str,
    title_en: &'static str,
    items: &'static [NavItemDef],
}

impl NavGroupDef {
    fn title(self, lang: Language) -> &'static str {
        match lang {
            Language::Zh => self.title_zh,
            Language::En => self.title_en,
        }
    }
}

const NAV_ENTRY_ITEMS: [NavItemDef; 2] = [
    NavItemDef {
        page: Page::Overview,
        zh: "总览",
        en: "Overview",
    },
    NavItemDef {
        page: Page::Setup,
        zh: "快速设置",
        en: "Setup",
    },
];

const NAV_SESSION_CONSOLE_ITEMS: [NavItemDef; 4] = [
    NavItemDef {
        page: Page::Sessions,
        zh: "会话",
        en: "Sessions",
    },
    NavItemDef {
        page: Page::Requests,
        zh: "请求",
        en: "Requests",
    },
    NavItemDef {
        page: Page::History,
        zh: "历史",
        en: "History",
    },
    NavItemDef {
        page: Page::Stats,
        zh: "统计",
        en: "Stats",
    },
];

const NAV_STATION_HEALTH_ITEMS: [NavItemDef; 2] = [
    NavItemDef {
        page: Page::Stations,
        zh: "站点",
        en: "Stations",
    },
    NavItemDef {
        page: Page::Doctor,
        zh: "诊断",
        en: "Doctor",
    },
];

const NAV_WORKSPACE_ITEMS: [NavItemDef; 2] = [
    NavItemDef {
        page: Page::Config,
        zh: "配置",
        en: "Config",
    },
    NavItemDef {
        page: Page::Settings,
        zh: "设置",
        en: "Settings",
    },
];

const NAV_GROUPS: [NavGroupDef; 4] = [
    NavGroupDef {
        title_zh: "入口",
        title_en: "Entry",
        items: &NAV_ENTRY_ITEMS,
    },
    NavGroupDef {
        title_zh: "会话控制台",
        title_en: "Session Console",
        items: &NAV_SESSION_CONSOLE_ITEMS,
    },
    NavGroupDef {
        title_zh: "站点/健康台",
        title_en: "Station & Health",
        items: &NAV_STATION_HEALTH_ITEMS,
    },
    NavGroupDef {
        title_zh: "配置工作区",
        title_en: "Config Workspace",
        items: &NAV_WORKSPACE_ITEMS,
    },
];

fn page_nav_groups() -> &'static [NavGroupDef] {
    &NAV_GROUPS
}

fn remote_safe_surface_status_line(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    let host_only = format_host_local_capability_summary(caps, lang).unwrap_or_else(|| {
        pick(
            lang,
            "会话历史 / transcript read / cwd enrichment",
            "session history / transcript read / cwd enrichment",
        )
        .to_string()
    });

    Some(match lang {
        Language::Zh => format!(
            "当前是远程附着：会话控制台、站点/健康台和共享观测/配置面仍通过控制面访问；{host_only} 这类 host-local 能力仍只在代理主机本地可用。"
        ),
        Language::En => format!(
            "Remote attach: the Session Console, station/health console, and shared observed/config surfaces remain available through the control plane; host-local capabilities remain on the proxy host only: {host_only}."
        ),
    })
}

fn render_nav_group(ui: &mut egui::Ui, lang: Language, current: &mut Page, group: NavGroupDef) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(group.title(lang)).small().strong());
        for item in group.items {
            if ui
                .selectable_label(*current == item.page, item.label(lang))
                .clicked()
            {
                *current = item.page;
            }
        }
    });
}

pub fn nav(
    ui: &mut egui::Ui,
    lang: Language,
    current: &mut Page,
    proxy: &super::proxy_control::ProxyController,
) {
    ui.vertical(|ui| {
        ui.small(pick(
            lang,
            "导航按控制台/工作区分组，而不是平铺全部页面。",
            "Navigation is grouped by console/workspace instead of a flat page strip.",
        ));
        ui.add_space(4.0);
        for group in page_nav_groups() {
            render_nav_group(ui, lang, current, *group);
        }

        if let Some(att) = proxy.attached()
            && let Some(status) = remote_safe_surface_status_line(
                att.admin_base_url.as_str(),
                &att.host_local_capabilities,
                lang,
            )
        {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), status);
        }
    });
    ui.separator();
}

pub fn render(ui: &mut egui::Ui, page: Page, ctx: &mut PageCtx<'_>) {
    match page {
        Page::Setup => setup::render(ui, ctx),
        Page::Overview => overview::render(ui, ctx),
        Page::Stations => stations::render(ui, ctx),
        Page::Doctor => doctor::render(ui, ctx),
        Page::Config => render_config(ui, ctx),
        Page::Sessions => sessions::render(ui, ctx),
        Page::Requests => requests::render(ui, ctx),
        Page::Stats => stats::render(ui, ctx),
        Page::History => history::render_history(ui, ctx),
        Page::Settings => settings::render(ui, ctx),
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
    capabilities: Option<StationCapabilitySummary>,
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
    runtime_station_catalog: Option<&BTreeMap<String, StationOption>>,
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
        ConfigMode::Form => config_legacy::render(ui, ctx),
        ConfigMode::Raw => config_raw::render(ui, ctx),
    }
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
    station_health: HashMap<String, StationHealth>,
    health_checks: HashMap<String, HealthCheckStatus>,
    lb_view: HashMap<String, LbConfigView>,
}

fn runtime_station_maps(proxy: &super::proxy_control::ProxyController) -> RuntimeStationMaps {
    match proxy.kind() {
        ProxyModeKind::Running => proxy
            .running()
            .map(|running| RuntimeStationMaps {
                station_health: running.station_health.clone(),
                health_checks: running.health_checks.clone(),
                lb_view: running.lb_view.clone(),
            })
            .unwrap_or_default(),
        ProxyModeKind::Attached => proxy
            .attached()
            .map(|attached| RuntimeStationMaps {
                station_health: attached.station_health.clone(),
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
    health: Option<&StationHealth>,
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
        (Language::Zh, RuntimeConfigState::HalfOpen) => "half_open",
        (_, RuntimeConfigState::Normal) => "normal",
        (_, RuntimeConfigState::Draining) => "draining",
        (_, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (_, RuntimeConfigState::HalfOpen) => "half_open",
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
    capabilities: &StationCapabilitySummary,
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
    capabilities: &StationCapabilitySummary,
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

fn format_runtime_station_source(lang: Language, cfg: &StationOption) -> String {
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
        pick(lang, "站点配置", "station config").to_string()
    } else {
        parts.join(", ")
    }
}

fn station_options_from_gui_stations(stations: &[StationOption]) -> Vec<(String, String)> {
    let mut out = stations
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
            host_local_transcript_path: None,
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
            last_station: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station_value: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station: None,
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
        row.last_station = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_station_value = Some(ResolvedRouteValue {
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
        row.last_station = Some("right".to_string());
        row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
        row.effective_station_value = Some(ResolvedRouteValue {
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
    fn page_nav_groups_cover_each_page_once() {
        let all_pages = [
            Page::Setup,
            Page::Overview,
            Page::Stations,
            Page::Doctor,
            Page::Config,
            Page::Sessions,
            Page::Requests,
            Page::Stats,
            Page::History,
            Page::Settings,
        ];

        for page in all_pages {
            let count = page_nav_groups()
                .iter()
                .flat_map(|group| group.items.iter())
                .filter(|item| item.page == page)
                .count();
            assert_eq!(count, 1, "page should appear exactly once: {page:?}");
        }
    }

    #[test]
    fn remote_safe_surface_status_line_absent_for_loopback_attach() {
        let status = remote_safe_surface_status_line(
            "http://127.0.0.1:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: true,
                cwd_enrichment: true,
            },
            Language::Zh,
        );
        assert!(status.is_none());
    }

    #[test]
    fn remote_safe_surface_status_line_mentions_host_only_capabilities() {
        let status = remote_safe_surface_status_line(
            "http://100.79.12.5:3211",
            &HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: false,
                cwd_enrichment: true,
            },
            Language::En,
        )
        .expect("remote surface status");

        assert!(status.contains("Remote attach"));
        assert!(status.contains("Session Console"));
        assert!(status.contains("session history / cwd enrichment"));
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
    fn session_control_posture_warns_when_bound_profile_is_missing() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.binding_continuity_mode = Some(SessionContinuityMode::ManualProfile);

        let posture = session_control_posture(&row, &[], Language::Zh);

        assert_eq!(posture.tone, SessionControlTone::Warning);
        assert!(posture.headline.contains("已缺失"));
        assert!(posture.detail.contains("找不到这个 profile"));
    }

    #[test]
    fn session_control_posture_describes_session_overrides_without_binding() {
        let mut row = sample_session_row();
        row.override_station = Some("right".to_string());
        row.override_service_tier = Some("priority".to_string());

        let posture = session_control_posture(&row, &[], Language::En);

        assert_eq!(posture.tone, SessionControlTone::Neutral);
        assert!(posture.headline.contains("no profile binding"));
        assert!(posture.detail.contains("station"));
        assert!(posture.detail.contains("service_tier"));
    }

    #[test]
    fn route_decision_changed_fields_reports_effective_drift() {
        let mut row = sample_session_row();
        row.effective_model = Some(ResolvedRouteValue {
            value: "gpt-5.4-fast".to_string(),
            source: RouteValueSource::SessionOverride,
        });
        row.effective_station_value = Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        });
        row.last_route_decision = Some(RouteDecisionProvenance {
            decided_at_ms: 123,
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.4".to_string(),
                source: RouteValueSource::ProfileDefault,
            }),
            effective_station: Some(ResolvedRouteValue {
                value: "right".to_string(),
                source: RouteValueSource::RuntimeFallback,
            }),
            ..Default::default()
        });

        let changed = route_decision_changed_fields(&row, Language::En);

        assert_eq!(changed, vec!["model".to_string()]);
    }

    #[test]
    fn session_route_decision_status_line_mentions_changed_fields() {
        let mut row = sample_session_row();
        row.effective_service_tier = Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::SessionOverride,
        });
        row.last_route_decision = Some(RouteDecisionProvenance {
            decided_at_ms: 456,
            effective_service_tier: Some(ResolvedRouteValue {
                value: "default".to_string(),
                source: RouteValueSource::ProfileDefault,
            }),
            ..Default::default()
        });

        let status = session_route_decision_status_line(&row, Language::En);

        assert!(status.contains("snapshot"));
        assert!(status.contains("service_tier"));
    }

    #[test]
    fn build_session_rows_from_cards_preserves_last_route_decision() {
        let rows = build_session_rows_from_cards(&[SessionIdentityCard {
            session_id: Some("sid-1".to_string()),
            last_route_decision: Some(RouteDecisionProvenance {
                decided_at_ms: 789,
                provider_id: Some("right".to_string()),
                effective_model: Some(ResolvedRouteValue {
                    value: "gpt-5.4-fast".to_string(),
                    source: RouteValueSource::StationMapping,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }]);

        assert_eq!(rows.len(), 1);
        let decision = rows[0]
            .last_route_decision
            .as_ref()
            .expect("route decision");
        assert_eq!(decision.decided_at_ms, 789);
        assert_eq!(decision.provider_id.as_deref(), Some("right"));
        assert_eq!(
            decision
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.4-fast")
        );
    }

    #[test]
    fn session_list_control_label_prefers_profile_binding() {
        let mut row = sample_session_row();
        row.binding_profile_name = Some("fast".to_string());
        row.override_station = Some("right".to_string());

        assert_eq!(session_list_control_label(&row), "pf:fast");
    }

    #[test]
    fn focus_session_in_sessions_resets_filters_and_focuses_sid() {
        let mut state = SessionsViewState {
            active_only: true,
            errors_only: true,
            overrides_only: true,
            lock_order: true,
            search: "old".to_string(),
            default_profile_selection: None,
            selected_session_id: None,
            selected_idx: 9,
            ordered_session_ids: Vec::new(),
            last_active_set: HashSet::new(),
            editor: SessionOverrideEditor::default(),
        };

        focus_session_in_sessions(&mut state, "sid-history".to_string());

        assert!(!state.active_only);
        assert!(!state.errors_only);
        assert!(!state.overrides_only);
        assert_eq!(state.search, "sid-history");
        assert_eq!(state.selected_session_id.as_deref(), Some("sid-history"));
        assert_eq!(state.selected_idx, 0);
        assert!(state.lock_order);
    }

    #[test]
    fn prepare_select_requests_for_session_sets_explicit_focus() {
        let mut state = RequestsViewState {
            errors_only: true,
            scope_session: false,
            focused_session_id: None,
            selected_idx: 7,
        };

        prepare_select_requests_for_session(&mut state, "sid-req".to_string());

        assert!(!state.errors_only);
        assert!(state.scope_session);
        assert_eq!(state.focused_session_id.as_deref(), Some("sid-req"));
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn request_history_summary_from_request_builds_observed_bridge() {
        let request = FinishedRequest {
            id: 7,
            session_id: Some("sid-req".to_string()),
            client_name: Some("Tablet".to_string()),
            client_addr: Some("100.64.0.13".to_string()),
            cwd: Some("/remote/recent".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("vibe".to_string()),
            provider_id: Some("vibe".to_string()),
            upstream_base_url: Some("https://api.example.com/v1".to_string()),
            route_decision: None,
            usage: None,
            retry: None,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 500,
            ttfb_ms: Some(120),
            ended_at_ms: 9_000,
        };

        let summary =
            request_history_summary_from_request(&request, None, Language::En).expect("summary");

        assert_eq!(summary.id, "sid-req");
        assert_eq!(summary.source, SessionSummarySource::ObservedOnly);
        assert_eq!(summary.sort_hint_ms, Some(9_000));
        assert!(
            summary.first_user_message.as_deref().is_some_and(
                |msg| msg.contains("station=vibe") && msg.contains("path=/v1/responses")
            )
        );
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
            StationOption {
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
                capabilities: StationCapabilitySummary {
                    model_catalog_kind: ModelCatalogKind::Declared,
                    supported_models: vec!["gpt-5.4".to_string()],
                    supports_service_tier: CapabilitySupport::Supported,
                    supports_reasoning_effort: CapabilitySupport::Unsupported,
                },
            },
        )]);
        let preview = build_profile_route_preview(
            &crate::config::ServiceControlProfile {
                extends: None,
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
