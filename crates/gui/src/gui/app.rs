use eframe::egui;

use tracing_subscriber::EnvFilter;

use super::config::GuiConfig;
use super::i18n::{Language, pick};
use super::pages::{Page, PageCtx, ViewState};
use super::proxy_control::ProxyController;
use super::single_instance::{AcquireResult, SingleInstance};
use super::tray::{TrayAction, TrayController};
use super::util::open_in_file_manager;

type LogGuard = tracing_appender::non_blocking::WorkerGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupBehavior {
    Show,
    Minimized,
    MinimizeToTray,
}

impl StartupBehavior {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "show" | "visible" => Self::Show,
            "minimized" | "minimize" => Self::Minimized,
            "tray" | "minimize_to_tray" | "minimized_to_tray" => Self::MinimizeToTray,
            _ => Self::MinimizeToTray,
        }
    }
}

pub fn run() -> eframe::Result<()> {
    let log_guard = init_gui_tracing();

    let single_instance = match SingleInstance::acquire_or_notify() {
        Ok(AcquireResult::Primary(guard)) => Some(guard),
        Ok(AcquireResult::SecondaryNotified) => return Ok(()),
        Err(e) => {
            tracing::warn!("single-instance init failed (continuing): {e}");
            None
        }
    };

    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "codex-helper (GUI)",
        options,
        Box::new(move |_cc| Ok(Box::new(GuiApp::new(single_instance, log_guard)))),
    )
}

struct GuiApp {
    page: Page,
    view: ViewState,
    gui_cfg: GuiConfig,
    proxy_config_path: std::path::PathBuf,
    proxy_config_text: String,
    last_error: Option<String>,
    last_info: Option<String>,
    rt: tokio::runtime::Runtime,
    proxy: ProxyController,
    tray: Option<TrayController>,
    pending_startup: Option<StartupBehavior>,
    allow_close_once: bool,
    single_instance: Option<SingleInstance>,
    did_auto_connect: bool,
    did_load_fonts: bool,
    _log_guard: Option<LogGuard>,
}

impl GuiApp {
    fn new(single_instance: Option<SingleInstance>, log_guard: Option<LogGuard>) -> Self {
        let args = std::env::args().collect::<Vec<_>>();
        let gui_cfg = GuiConfig::load_or_default();
        let arg_minimized = args
            .iter()
            .any(|a| a == "--autostart" || a == "--minimized");
        let startup_behavior = if arg_minimized {
            StartupBehavior::MinimizeToTray
        } else {
            StartupBehavior::parse(&gui_cfg.window.startup_behavior)
        };
        let pending_startup = match startup_behavior {
            StartupBehavior::Show => None,
            _ => Some(startup_behavior),
        };

        let proxy_config_path = crate::config::config_file_path();
        let proxy_config_text = std::fs::read_to_string(&proxy_config_path).unwrap_or_default();

        let initial_page = {
            let config_ready = proxy_config_path.exists() && !proxy_config_text.trim().is_empty();
            let svc = gui_cfg.service_kind();
            let switched = match svc {
                crate::config::ServiceKind::Claude => {
                    crate::codex_integration::claude_switch_status()
                        .map(|s| s.enabled)
                        .unwrap_or(false)
                }
                _ => crate::codex_integration::codex_switch_status()
                    .map(|s| s.enabled)
                    .unwrap_or(false),
            };
            if !config_ready || !switched {
                Page::Setup
            } else {
                Page::Overview
            }
        };
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let proxy = ProxyController::new(gui_cfg.proxy.default_port, gui_cfg.service_kind());
        Self {
            page: initial_page,
            view: ViewState::default(),
            gui_cfg,
            proxy_config_path,
            proxy_config_text,
            last_error: None,
            last_info: None,
            rt,
            proxy,
            tray: None,
            pending_startup,
            allow_close_once: false,
            single_instance,
            did_auto_connect: false,
            did_load_fonts: false,
            _log_guard: log_guard,
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_pixels_per_point(1.0);

        if !self.did_load_fonts {
            self.did_load_fonts = true;
            install_fonts(ctx);
        }

        let lang: Language = self.gui_cfg.language_enum();
        let refresh =
            std::time::Duration::from_millis(self.gui_cfg.ui.refresh_ms.clamp(100, 5_000));
        ctx.request_repaint_after(refresh);

        if self
            .single_instance
            .as_ref()
            .is_some_and(|si| si.check_show_requested())
        {
            self.pending_startup = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // Lazy-init tray: needs an event loop on this thread.
        if self.tray.is_none() && self.gui_cfg.tray.enabled {
            match TrayController::try_new(lang) {
                Ok(t) => self.tray = Some(t),
                Err(e) => {
                    self.gui_cfg.tray.enabled = false;
                    let _ = self.gui_cfg.save();
                    self.last_error = Some(format!("tray init failed: {e}"));
                }
            }
        } else if self.tray.is_some() && !self.gui_cfg.tray.enabled {
            self.tray = None;
        }

        // Auto attach-or-start:
        // 1) Probe configured port; attach if proxy exists.
        // 2) If probe fails and fallback enabled, scan 3210-3220; attach best candidate if any.
        // 3) Otherwise start proxy (honors on-port-in-use setting, may prompt).
        if !self.did_auto_connect
            && self.gui_cfg.proxy.auto_attach_or_start
            && matches!(
                self.proxy.kind(),
                super::proxy_control::ProxyModeKind::Stopped
            )
        {
            self.did_auto_connect = true;

            let preferred_port = self
                .gui_cfg
                .attach
                .last_port
                .unwrap_or(self.gui_cfg.proxy.default_port);

            let mut attach_port: Option<u16> = None;
            if self
                .proxy
                .probe_local_proxy(&self.rt, preferred_port)
                .is_some()
            {
                attach_port = Some(preferred_port);
            } else if self.gui_cfg.proxy.discovery_scan_fallback {
                if self.proxy.scan_local_proxies(&self.rt, 3210..=3220).is_ok() {
                    let discovered = self.proxy.discovered_proxies();
                    if !discovered.is_empty() {
                        // Best-effort selection:
                        // - prefer last_port
                        // - prefer matching service
                        // - prefer api v1
                        // - lowest port
                        let desired_service = match self.gui_cfg.service_kind() {
                            crate::config::ServiceKind::Codex => "codex",
                            crate::config::ServiceKind::Claude => "claude",
                        };
                        attach_port = discovered
                            .iter()
                            .find(|p| p.port == preferred_port)
                            .or_else(|| {
                                discovered
                                    .iter()
                                    .find(|p| p.service_name.as_deref() == Some(desired_service))
                            })
                            .or_else(|| discovered.iter().find(|p| p.api_version == Some(1)))
                            .or_else(|| discovered.iter().min_by_key(|p| p.port))
                            .map(|p| p.port);
                    }
                }
            }

            if let Some(port) = attach_port {
                self.proxy.request_attach(port);
                self.gui_cfg.attach.last_port = Some(port);
                let _ = self.gui_cfg.save();
                self.last_info =
                    Some(pick(lang, "自动附着到已运行代理", "Auto-attached to proxy").to_string());
            } else {
                let action = super::proxy_control::PortInUseAction::parse(
                    &self.gui_cfg.attach.on_port_in_use,
                );
                self.proxy.request_start_or_prompt(
                    &self.rt,
                    action,
                    self.gui_cfg.attach.remember_choice,
                );
            }
        }

        if let Some(behavior) = self.pending_startup.take() {
            match behavior {
                StartupBehavior::Show => {}
                StartupBehavior::Minimized => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                StartupBehavior::MinimizeToTray => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
        }

        if let Some(tray) = self.tray.as_ref() {
            for action in tray.drain_actions() {
                match action {
                    TrayAction::Show => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::Hide => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    TrayAction::Toggle => {
                        let is_min = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
                        if is_min {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    }
                    TrayAction::OpenSetup => {
                        self.page = Page::Setup;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::StartProxy => {
                        if matches!(
                            self.proxy.kind(),
                            super::proxy_control::ProxyModeKind::Stopped
                        ) {
                            let action = super::proxy_control::PortInUseAction::parse(
                                &self.gui_cfg.attach.on_port_in_use,
                            );
                            self.proxy.request_start_or_prompt(
                                &self.rt,
                                action,
                                self.gui_cfg.attach.remember_choice,
                            );
                        }
                    }
                    TrayAction::StopProxy => {
                        if let Err(e) = self.proxy.stop(&self.rt) {
                            self.last_error = Some(format!("stop failed: {e}"));
                        }
                    }
                    TrayAction::ReloadConfig => {
                        if let Err(e) = self.proxy.reload_runtime_config(&self.rt) {
                            self.last_error = Some(format!("reload config failed: {e}"));
                        } else {
                            self.last_info =
                                Some(pick(lang, "已重载配置", "Config reloaded").to_string());
                        }
                    }
                    TrayAction::SwitchOn => {
                        let port = self
                            .proxy
                            .snapshot()
                            .and_then(|s| s.port)
                            .unwrap_or(self.proxy.desired_port());
                        let svc = self.gui_cfg.service_kind();
                        let result = match svc {
                            crate::config::ServiceKind::Claude => {
                                crate::codex_integration::claude_switch_on(port)
                            }
                            _ => crate::codex_integration::switch_on(port),
                        };
                        match result {
                            Ok(()) => {
                                self.last_info = Some(
                                    pick(
                                        lang,
                                        "已启用客户端代理（switch on）",
                                        "Client proxy enabled (switch on)",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => self.last_error = Some(format!("switch on failed: {e}")),
                        }
                    }
                    TrayAction::SwitchOff => {
                        let svc = self.gui_cfg.service_kind();
                        let result = match svc {
                            crate::config::ServiceKind::Claude => {
                                crate::codex_integration::claude_switch_off()
                            }
                            _ => crate::codex_integration::switch_off(),
                        };
                        match result {
                            Ok(()) => {
                                self.last_info = Some(
                                    pick(
                                        lang,
                                        "已恢复客户端配置（switch off）",
                                        "Client config restored (switch off)",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => self.last_error = Some(format!("switch off failed: {e}")),
                        }
                    }
                    TrayAction::OpenConfig => {
                        let path = crate::config::config_file_path();
                        if let Err(e) = open_in_file_manager(&path, true) {
                            self.last_error = Some(format!("open config failed: {e}"));
                        }
                    }
                    TrayAction::OpenLogs => {
                        let dir = crate::config::proxy_home_dir().join("logs");
                        if let Err(e) = open_in_file_manager(&dir, false) {
                            self.last_error = Some(format!("open logs failed: {e}"));
                        }
                    }
                    TrayAction::Quit => {
                        self.allow_close_once = true;
                        let _ = self.proxy.stop(&self.rt);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // Close behavior: default "minimize_to_tray".
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.allow_close_once {
                self.allow_close_once = false;
            } else if self.gui_cfg.window.close_behavior == "minimize_to_tray" {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }

        // Keep attach status fresh (read-only mode).
        self.proxy.refresh_attached_if_due(&self.rt, refresh);
        self.proxy.refresh_running_if_due(&self.rt, refresh);

        egui::TopBottomPanel::top("top_nav").show(ctx, |ui| {
            super::pages::nav(ui, lang, &mut self.page);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(msg) = self.last_error.as_deref() {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(200, 60, 60), msg);
                ui.add_space(6.0);
            } else if let Some(msg) = self.last_info.as_deref() {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(60, 160, 90), msg);
                ui.add_space(6.0);
            }

            let mut page_ctx = PageCtx {
                lang,
                view: &mut self.view,
                gui_cfg: &mut self.gui_cfg,
                proxy_config_text: &mut self.proxy_config_text,
                proxy_config_path: &self.proxy_config_path,
                last_error: &mut self.last_error,
                last_info: &mut self.last_info,
                rt: &self.rt,
                proxy: &mut self.proxy,
            };
            super::pages::render(ui, self.page, &mut page_ctx);
        });

        if let Some(next) = self.view.requested_page.take() {
            self.page = next;
        }
    }
}

fn init_gui_tracing() -> Option<LogGuard> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let log_dir = crate::config::proxy_home_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::never(&log_dir, "gui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(false)
        .with_writer(non_blocking)
        .init();
    Some(guard)
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut installed_any = false;

    #[cfg(windows)]
    {
        if let Some((name, data)) = load_windows_cjk_font() {
            fonts.font_data.insert(name.clone(), data);
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, name.clone());
            }
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                // Keep existing monospace font first; add CJK as fallback.
                if !list.contains(&name) {
                    list.push(name.clone());
                }
            }
            installed_any = true;
            tracing::info!("gui fonts: installed windows cjk font: {name}");
        } else {
            tracing::warn!(
                "gui fonts: failed to load any windows cjk font; chinese may render as tofu"
            );
        }
    }

    #[cfg(not(windows))]
    {
        if let Some((name, data)) = load_system_cjk_font() {
            fonts.font_data.insert(name.clone(), data);
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, name.clone());
            }
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                if !list.contains(&name) {
                    list.push(name.clone());
                }
            }
            installed_any = true;
            tracing::info!("gui fonts: installed system cjk font: {name}");
        } else {
            tracing::warn!(
                "gui fonts: failed to load any system cjk font; chinese may render as tofu"
            );
        }
    }

    if installed_any {
        ctx.set_fonts(fonts);
    }
}

#[cfg(windows)]
fn load_windows_cjk_font() -> Option<(String, egui::FontData)> {
    fn try_load(name: &str, filename: &str) -> Option<(String, egui::FontData)> {
        let path = std::path::PathBuf::from(r"C:\Windows\Fonts").join(filename);
        let bytes = std::fs::read(&path).ok()?;
        let data = egui::FontData::from_owned(bytes);
        Some((name.to_string(), data))
    }

    // Prefer TTF/OTF to avoid TTC parsing issues.
    try_load("Deng", "Deng.ttf")
        .or_else(|| try_load("SimHei", "simhei.ttf"))
        .or_else(|| try_load("SimKai", "simkai.ttf"))
        .or_else(|| try_load("STKaiTi", "STKAITI.TTF"))
        .or_else(|| try_load("SimsunExtG", "SimsunExtG.ttf"))
}

#[cfg(not(windows))]
fn load_system_cjk_font() -> Option<(String, egui::FontData)> {
    use fontdb::{Database, Family, Query};

    fn try_load_face(db: &Database, family: &str) -> Option<(String, egui::FontData)> {
        let id = db.query(&Query {
            families: &[Family::Name(family)],
            ..Query::default()
        })?;
        let face = db.face(id)?;
        match &face.source {
            fontdb::Source::Binary(data) => {
                let bytes = data.as_ref().as_ref().to_vec();
                Some((family.to_string(), egui::FontData::from_owned(bytes)))
            }
            #[cfg(feature = "fs")]
            fontdb::Source::File(path) => {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ext != "ttf" && ext != "otf" {
                    return None;
                }
                let bytes = std::fs::read(path).ok()?;
                Some((family.to_string(), egui::FontData::from_owned(bytes)))
            }
            #[cfg(all(feature = "fs", feature = "memmap"))]
            fontdb::Source::SharedFile(path, data) => {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ext != "ttf" && ext != "otf" {
                    return None;
                }
                let bytes = data.as_ref().as_ref().to_vec();
                Some((family.to_string(), egui::FontData::from_owned(bytes)))
            }
        }
    }

    let mut db = Database::new();
    db.load_system_fonts();

    // Best-effort. Many systems ship CJK fonts as .ttc; egui typically needs .ttf/.otf.
    for family in [
        "Noto Sans CJK SC",
        "Noto Sans CJK JP",
        "Noto Sans CJK KR",
        "Source Han Sans SC",
        "Source Han Sans CN",
        "WenQuanYi Zen Hei",
        "WenQuanYi Micro Hei",
        "PingFang SC",
        "Hiragino Sans GB",
    ] {
        if let Some(v) = try_load_face(&db, family) {
            return Some(v);
        }
    }

    None
}
