use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};

use super::i18n::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Hide,
    Toggle,
    OpenSetup,
    StartProxy,
    StopProxy,
    ReloadSettings,
    SwitchOn,
    SwitchOff,
    /// Apply API v1 global override. In route-graph mode this targets route target.
    ApplyPinnedStation {
        name: Option<String>,
    },
    OpenSettingsFile,
    OpenLogs,
    Quit,
}

#[derive(Debug, Clone)]
pub struct TrayMenuModel {
    pub proxy_kind: super::proxy_control::ProxyModeKind,
    pub base_url: Option<String>,
    pub service_name: Option<String>,
    pub port: Option<u16>,
    pub supports_v1: bool,
    pub supports_global_route_target_override: bool,
    pub active_display: Option<String>,
    pub stations: Vec<crate::dashboard_core::StationOption>,
    pub global_station_override: Option<String>,
    pub global_route_target_override: Option<String>,
}

pub struct TrayController {
    tray: TrayIcon,
    id_show: MenuId,
    id_hide: MenuId,
    id_toggle: MenuId,
    id_open_setup: MenuId,
    id_start: MenuId,
    id_stop: MenuId,
    id_reload: MenuId,
    id_switch_on: MenuId,
    id_switch_off: MenuId,
    id_open_settings: MenuId,
    id_open_logs: MenuId,
    id_quit: MenuId,
    dynamic_actions: std::collections::HashMap<MenuId, TrayAction>,
    last_menu_sig: Option<String>,
}

impl TrayController {
    pub fn try_new(lang: Language) -> anyhow::Result<Self> {
        let icon = default_icon()?;

        let id_show = MenuId::new("codex-helper-gui.tray.show");
        let id_hide = MenuId::new("codex-helper-gui.tray.hide");
        let id_toggle = MenuId::new("codex-helper-gui.tray.toggle");
        let id_open_setup = MenuId::new("codex-helper-gui.tray.open_setup");
        let id_start = MenuId::new("codex-helper-gui.tray.start_proxy");
        let id_stop = MenuId::new("codex-helper-gui.tray.stop_proxy");
        let id_reload = MenuId::new("codex-helper-gui.tray.reload_settings");
        let id_switch_on = MenuId::new("codex-helper-gui.tray.switch_on");
        let id_switch_off = MenuId::new("codex-helper-gui.tray.switch_off");
        let id_open_settings = MenuId::new("codex-helper-gui.tray.open_settings");
        let id_open_logs = MenuId::new("codex-helper-gui.tray.open_logs");
        let id_quit = MenuId::new("codex-helper-gui.tray.quit");

        let menu = build_menu_base(
            lang,
            &id_show,
            &id_hide,
            &id_toggle,
            &id_open_setup,
            &id_start,
            &id_stop,
            &id_reload,
            &id_switch_on,
            &id_switch_off,
            &id_open_settings,
            &id_open_logs,
            &id_quit,
            None,
        )?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("codex-helper")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()?;

        Ok(Self {
            tray,
            id_show,
            id_hide,
            id_toggle,
            id_open_setup,
            id_start,
            id_stop,
            id_reload,
            id_switch_on,
            id_switch_off,
            id_open_settings,
            id_open_logs,
            id_quit,
            dynamic_actions: std::collections::HashMap::new(),
            last_menu_sig: None,
        })
    }

    pub fn update_menu(&mut self, lang: Language, model: TrayMenuModel) -> anyhow::Result<()> {
        let sig = compute_menu_sig(&model);
        if self.last_menu_sig.as_deref() == Some(sig.as_str()) {
            return Ok(());
        }
        self.last_menu_sig = Some(sig);

        let mut dynamic_actions = std::collections::HashMap::new();
        let menu = build_menu_base(
            lang,
            &self.id_show,
            &self.id_hide,
            &self.id_toggle,
            &self.id_open_setup,
            &self.id_start,
            &self.id_stop,
            &self.id_reload,
            &self.id_switch_on,
            &self.id_switch_off,
            &self.id_open_settings,
            &self.id_open_logs,
            &self.id_quit,
            Some((&model, &mut dynamic_actions)),
        )?;

        self.tray.set_menu(Some(Box::new(menu)));
        self.dynamic_actions = dynamic_actions;
        Ok(())
    }

    pub fn drain_actions(&self) -> Vec<TrayAction> {
        let mut out = Vec::new();
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                out.push(TrayAction::Show);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            out.extend(self.map_event(&event));
        }
        out
    }

    fn map_event(&self, event: &MenuEvent) -> Option<TrayAction> {
        let id = event.id();
        if id == &self.id_show {
            Some(TrayAction::Show)
        } else if id == &self.id_hide {
            Some(TrayAction::Hide)
        } else if id == &self.id_toggle {
            Some(TrayAction::Toggle)
        } else if id == &self.id_open_setup {
            Some(TrayAction::OpenSetup)
        } else if id == &self.id_start {
            Some(TrayAction::StartProxy)
        } else if id == &self.id_stop {
            Some(TrayAction::StopProxy)
        } else if id == &self.id_reload {
            Some(TrayAction::ReloadSettings)
        } else if id == &self.id_switch_on {
            Some(TrayAction::SwitchOn)
        } else if id == &self.id_switch_off {
            Some(TrayAction::SwitchOff)
        } else if id == &self.id_open_settings {
            Some(TrayAction::OpenSettingsFile)
        } else if id == &self.id_open_logs {
            Some(TrayAction::OpenLogs)
        } else if id == &self.id_quit {
            Some(TrayAction::Quit)
        } else {
            self.dynamic_actions.get(id).cloned()
        }
    }
}

fn compute_menu_sig(m: &TrayMenuModel) -> String {
    let mut s = String::new();
    s.push_str("k=");
    s.push_str(match m.proxy_kind {
        super::proxy_control::ProxyModeKind::Stopped => "stopped",
        super::proxy_control::ProxyModeKind::Starting => "starting",
        super::proxy_control::ProxyModeKind::Running => "running",
        super::proxy_control::ProxyModeKind::Attached => "attached",
    });
    s.push_str("|svc=");
    s.push_str(m.service_name.as_deref().unwrap_or("-"));
    s.push_str("|port=");
    s.push_str(&m.port.unwrap_or(0).to_string());
    s.push_str("|url=");
    s.push_str(m.base_url.as_deref().unwrap_or("-"));
    s.push_str("|v1=");
    s.push_str(if m.supports_v1 { "1" } else { "0" });
    s.push_str("|route_target=");
    s.push_str(if m.supports_global_route_target_override {
        "1"
    } else {
        "0"
    });
    s.push_str("|active=");
    s.push_str(m.active_display.as_deref().unwrap_or("-"));
    s.push_str("|pinned=");
    s.push_str(m.global_station_override.as_deref().unwrap_or("<auto>"));
    s.push_str("|global_route=");
    s.push_str(
        m.global_route_target_override
            .as_deref()
            .unwrap_or("<auto>"),
    );
    s.push_str("|cfgs=");
    for c in m.stations.iter() {
        s.push_str(&c.name);
        s.push(',');
        s.push_str(if c.enabled { "1" } else { "0" });
        s.push(',');
        s.push_str(&c.level.to_string());
        s.push('|');
    }
    s
}

fn stable_id_u64(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

fn menu_id(prefix: &str, key: &str) -> MenuId {
    let h = stable_id_u64(key);
    MenuId::new(format!("{prefix}.{h:016x}"))
}

#[allow(clippy::too_many_arguments)]
fn build_menu_base(
    lang: Language,
    id_show: &MenuId,
    id_hide: &MenuId,
    id_toggle: &MenuId,
    id_open_setup: &MenuId,
    id_start: &MenuId,
    id_stop: &MenuId,
    id_reload: &MenuId,
    id_switch_on: &MenuId,
    id_switch_off: &MenuId,
    id_open_settings: &MenuId,
    id_open_logs: &MenuId,
    id_quit: &MenuId,
    dynamic: Option<(
        &TrayMenuModel,
        &mut std::collections::HashMap<MenuId, TrayAction>,
    )>,
) -> anyhow::Result<Menu> {
    let menu = Menu::new();

    if let Some((model, dynamic_actions)) = dynamic {
        let status = format!(
            "{}: {}{}",
            pick(lang, "状态", "Status"),
            match model.proxy_kind {
                super::proxy_control::ProxyModeKind::Running => pick(lang, "运行中", "Running"),
                super::proxy_control::ProxyModeKind::Attached => pick(lang, "已附着", "Attached"),
                super::proxy_control::ProxyModeKind::Starting => pick(lang, "启动中", "Starting"),
                super::proxy_control::ProxyModeKind::Stopped => pick(lang, "未运行", "Stopped"),
            },
            model.port.map(|p| format!(" :{p}")).unwrap_or_default()
        );
        menu.append(&MenuItem::new(status, false, None))?;

        if let Some(svc) = model.service_name.as_deref() {
            menu.append(&MenuItem::new(
                format!("{}: {svc}", pick(lang, "服务", "Service")),
                false,
                None,
            ))?;
        }

        if let Some(active) = model.active_display.as_deref() {
            menu.append(&MenuItem::new(
                format!(
                    "{}: {active}",
                    pick(lang, "active_station", "active_station")
                ),
                false,
                None,
            ))?;
        }

        if model.proxy_kind != super::proxy_control::ProxyModeKind::Stopped {
            let pinned = if model.supports_global_route_target_override {
                model
                    .global_route_target_override
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"))
            } else {
                model
                    .global_station_override
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"))
            };
            menu.append(&MenuItem::new(
                format!(
                    "{}: {pinned}",
                    if model.supports_global_route_target_override {
                        pick(lang, "Global route target", "Global route target")
                    } else {
                        pick(lang, "Pinned station", "Pinned station")
                    }
                ),
                false,
                None,
            ))?;
        }

        menu.append(&PredefinedMenuItem::separator())?;

        let quick = Submenu::new(pick(lang, "快速切换", "Quick switch"), true);

        // Runtime-only global station/route target override.
        let can_pinned = (matches!(
            model.proxy_kind,
            super::proxy_control::ProxyModeKind::Running
                | super::proxy_control::ProxyModeKind::Attached
        )) && model.supports_v1
            && !model.stations.is_empty();
        let pinned_menu = Submenu::new(
            if model.supports_global_route_target_override {
                pick(lang, "全局 route target", "Global route target")
            } else {
                pick(
                    lang,
                    "全局站点覆盖(Pinned)",
                    "Global station override (pinned)",
                )
            },
            can_pinned,
        );
        if can_pinned {
            let cur = if model.supports_global_route_target_override {
                model.global_route_target_override.as_deref()
            } else {
                model.global_station_override.as_deref()
            };
            let id_auto = MenuId::new("codex-helper-gui.tray.pinned.auto");
            let auto_item = CheckMenuItem::with_id(
                id_auto.clone(),
                pick(lang, "<自动>", "<auto>"),
                true,
                cur.is_none(),
                None,
            );
            dynamic_actions.insert(id_auto, TrayAction::ApplyPinnedStation { name: None });
            pinned_menu.append(&auto_item)?;
            pinned_menu.append(&PredefinedMenuItem::separator())?;
            for station in model.stations.iter().filter(|c| c.enabled) {
                let label = format_station_label(station);
                let id = menu_id("codex-helper-gui.tray.pinned", station.name.as_str());
                let checked = cur == Some(station.name.as_str());
                let item = CheckMenuItem::with_id(id.clone(), label, true, checked, None);
                dynamic_actions.insert(
                    id,
                    TrayAction::ApplyPinnedStation {
                        name: Some(station.name.clone()),
                    },
                );
                pinned_menu.append(&item)?;
            }
        }
        quick.append(&pinned_menu)?;

        menu.append(&quick)?;
        menu.append(&PredefinedMenuItem::separator())?;
    }

    let show = MenuItem::with_id(id_show.clone(), pick(lang, "显示", "Show"), true, None);
    let hide = MenuItem::with_id(
        id_hide.clone(),
        pick(lang, "最小化", "Minimize"),
        true,
        None,
    );
    let toggle = MenuItem::with_id(
        id_toggle.clone(),
        pick(lang, "显示/最小化", "Show/Minimize"),
        true,
        None,
    );
    let open_setup = MenuItem::with_id(
        id_open_setup.clone(),
        pick(lang, "打开快速设置", "Open setup"),
        true,
        None,
    );
    let start = MenuItem::with_id(
        id_start.clone(),
        pick(lang, "启动代理", "Start proxy"),
        true,
        None,
    );
    let stop = MenuItem::with_id(
        id_stop.clone(),
        pick(lang, "停止代理", "Stop proxy"),
        true,
        None,
    );
    let reload = MenuItem::with_id(
        id_reload.clone(),
        pick(lang, "重载设置", "Reload settings"),
        true,
        None,
    );
    let switch_on = MenuItem::with_id(
        id_switch_on.clone(),
        pick(
            lang,
            "启用客户端代理 (switch on)",
            "Enable client proxy (switch on)",
        ),
        true,
        None,
    );
    let switch_off = MenuItem::with_id(
        id_switch_off.clone(),
        pick(
            lang,
            "恢复客户端配置 (switch off)",
            "Restore client config (switch off)",
        ),
        true,
        None,
    );
    let open_settings = MenuItem::with_id(
        id_open_settings.clone(),
        pick(lang, "打开设置文件", "Open settings file"),
        true,
        None,
    );
    let open_logs = MenuItem::with_id(
        id_open_logs.clone(),
        pick(lang, "打开日志目录", "Open logs folder"),
        true,
        None,
    );
    let quit = MenuItem::with_id(id_quit.clone(), pick(lang, "退出", "Quit"), true, None);

    menu.append_items(&[
        &show,
        &hide,
        &toggle,
        &PredefinedMenuItem::separator(),
        &open_setup,
        &PredefinedMenuItem::separator(),
        &start,
        &stop,
        &reload,
        &PredefinedMenuItem::separator(),
        &switch_on,
        &switch_off,
        &PredefinedMenuItem::separator(),
        &open_settings,
        &open_logs,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    Ok(menu)
}

fn format_station_label(station: &crate::dashboard_core::StationOption) -> String {
    let mut label = format!("L{} {}", station.level.clamp(1, 10), station.name);
    if let Some(alias) = station.alias.as_deref()
        && !alias.trim().is_empty()
    {
        label.push_str(&format!(" ({alias})"));
    }
    if !station.enabled {
        label.push_str(" [off]");
    }
    label
}

fn default_icon() -> anyhow::Result<Icon> {
    // A tiny generated 32x32 icon (RGBA), avoiding external assets for now.
    let w: u32 = 32;
    let h: u32 = 32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];

    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let diag = x == y || x + y == w - 1;
            let (r, g, b, a) = if border {
                (40, 160, 90, 255)
            } else if diag {
                (240, 240, 240, 255)
            } else {
                (24, 28, 34, 255)
            };
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = a;
        }
    }

    Ok(Icon::from_rgba(rgba, w, h)?)
}

fn pick(lang: Language, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}
