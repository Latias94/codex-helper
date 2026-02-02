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
    ReloadConfig,
    SwitchOn,
    SwitchOff,
    /// Persist `active` to local config and (if possible) reload proxy runtime.
    SetActiveConfig {
        service: crate::config::ServiceKind,
        name: String,
    },
    /// Apply API v1 global override (pinned). `None` means <auto>.
    ApplyPinnedConfig {
        name: Option<String>,
    },
    /// Select and apply a routing preset (best-effort).
    ApplyRoutingProfile {
        name: String,
    },
    OpenConfig,
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
    pub active_display: Option<String>,
    pub configs: Vec<crate::dashboard_core::ConfigOption>,
    pub global_override: Option<String>,
    pub routing_profiles: Vec<super::config::RoutingProfile>,
    pub selected_routing_profile: Option<String>,
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
    id_open_config: MenuId,
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
        let id_reload = MenuId::new("codex-helper-gui.tray.reload_config");
        let id_switch_on = MenuId::new("codex-helper-gui.tray.switch_on");
        let id_switch_off = MenuId::new("codex-helper-gui.tray.switch_off");
        let id_open_config = MenuId::new("codex-helper-gui.tray.open_config");
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
            &id_open_config,
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
            id_open_config,
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
            &self.id_open_config,
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
        loop {
            let Ok(event) = TrayIconEvent::receiver().try_recv() else {
                break;
            };
            match event {
                TrayIconEvent::DoubleClick { .. } => out.push(TrayAction::Show),
                _ => {}
            }
        }
        loop {
            let Ok(event) = MenuEvent::receiver().try_recv() else {
                break;
            };
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
            Some(TrayAction::ReloadConfig)
        } else if id == &self.id_switch_on {
            Some(TrayAction::SwitchOn)
        } else if id == &self.id_switch_off {
            Some(TrayAction::SwitchOff)
        } else if id == &self.id_open_config {
            Some(TrayAction::OpenConfig)
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
    s.push_str("|v1=");
    s.push_str(if m.supports_v1 { "1" } else { "0" });
    s.push_str("|active=");
    s.push_str(m.active_display.as_deref().unwrap_or("-"));
    s.push_str("|pinned=");
    s.push_str(m.global_override.as_deref().unwrap_or("<auto>"));
    s.push_str("|cfgs=");
    for c in m.configs.iter() {
        s.push_str(&c.name);
        s.push(',');
        s.push_str(if c.enabled { "1" } else { "0" });
        s.push(',');
        s.push_str(&c.level.to_string());
        s.push('|');
    }
    s.push_str("|profiles=");
    for p in m.routing_profiles.iter() {
        s.push_str(&p.name);
        s.push(',');
        s.push_str(p.service.as_str());
        s.push(',');
        if let Some(port) = p.port {
            s.push_str(&port.to_string());
        } else {
            s.push_str("-");
        }
        s.push('|');
    }
    s.push_str("|sel=");
    s.push_str(m.selected_routing_profile.as_deref().unwrap_or("-"));
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
    id_open_config: &MenuId,
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
                format!("{}: {active}", pick(lang, "active", "active")),
                false,
                None,
            ))?;
        }

        if model.proxy_kind != super::proxy_control::ProxyModeKind::Stopped {
            let pinned = model
                .global_override
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"));
            menu.append(&MenuItem::new(
                format!("{}: {pinned}", pick(lang, "Pinned", "Pinned")),
                false,
                None,
            ))?;
        }

        menu.append(&PredefinedMenuItem::separator())?;

        let quick = Submenu::new(pick(lang, "快速切换", "Quick switch"), true);

        // Active config quick switch (persistent).
        let can_set_active = matches!(
            model.proxy_kind,
            super::proxy_control::ProxyModeKind::Running
        ) && model
            .service_name
            .as_deref()
            .is_some_and(|s| s == "codex" || s == "claude")
            && !model.configs.is_empty();
        let active_menu = Submenu::new(
            pick(lang, "默认配置(active)", "Default (active)"),
            can_set_active,
        );
        if can_set_active {
            let svc = if model.service_name.as_deref() == Some("claude") {
                crate::config::ServiceKind::Claude
            } else {
                crate::config::ServiceKind::Codex
            };
            let active = model.active_display.as_deref().unwrap_or("");
            for cfg in model.configs.iter() {
                let label = format_config_label(cfg);
                let id = menu_id(
                    if svc == crate::config::ServiceKind::Claude {
                        "codex-helper-gui.tray.active.claude"
                    } else {
                        "codex-helper-gui.tray.active.codex"
                    },
                    cfg.name.as_str(),
                );
                let checked = cfg.name == active;
                let item = CheckMenuItem::with_id(id.clone(), label, true, checked, None);
                dynamic_actions.insert(
                    id,
                    TrayAction::SetActiveConfig {
                        service: svc,
                        name: cfg.name.clone(),
                    },
                );
                active_menu.append(&item)?;
            }
        }
        quick.append(&active_menu)?;

        // Pinned (runtime-only).
        let can_pinned = (matches!(
            model.proxy_kind,
            super::proxy_control::ProxyModeKind::Running
                | super::proxy_control::ProxyModeKind::Attached
        )) && model.supports_v1
            && !model.configs.is_empty();
        let pinned_menu = Submenu::new(
            pick(lang, "全局覆盖(Pinned)", "Global override (pinned)"),
            can_pinned,
        );
        if can_pinned {
            let cur = model.global_override.as_deref();
            let id_auto = MenuId::new("codex-helper-gui.tray.pinned.auto");
            let auto_item = CheckMenuItem::with_id(
                id_auto.clone(),
                pick(lang, "<自动>", "<auto>"),
                true,
                cur.is_none(),
                None,
            );
            dynamic_actions.insert(id_auto, TrayAction::ApplyPinnedConfig { name: None });
            pinned_menu.append(&auto_item)?;
            pinned_menu.append(&PredefinedMenuItem::separator())?;
            for cfg in model.configs.iter().filter(|c| c.enabled) {
                let label = format_config_label(cfg);
                let id = menu_id("codex-helper-gui.tray.pinned", cfg.name.as_str());
                let checked = cur == Some(cfg.name.as_str());
                let item = CheckMenuItem::with_id(id.clone(), label, true, checked, None);
                dynamic_actions.insert(
                    id,
                    TrayAction::ApplyPinnedConfig {
                        name: Some(cfg.name.clone()),
                    },
                );
                pinned_menu.append(&item)?;
            }
        }
        quick.append(&pinned_menu)?;

        // Routing profiles (best-effort apply).
        let profiles_menu = Submenu::new(pick(lang, "路由预设", "Routing presets"), true);
        if model.routing_profiles.is_empty() {
            profiles_menu.append(&MenuItem::new(pick(lang, "（无）", "(none)"), false, None))?;
        } else {
            let sel = model.selected_routing_profile.as_deref();
            for p in model.routing_profiles.iter() {
                let mut label = p.name.clone();
                if !p.service.trim().is_empty() {
                    label.push_str(&format!(" [{}]", p.service));
                }
                if let Some(port) = p.port {
                    label.push_str(&format!(":{port}"));
                }
                let id = menu_id("codex-helper-gui.tray.routing", p.name.as_str());
                let item = CheckMenuItem::with_id(
                    id.clone(),
                    label,
                    true,
                    sel == Some(p.name.as_str()),
                    None,
                );
                dynamic_actions.insert(
                    id,
                    TrayAction::ApplyRoutingProfile {
                        name: p.name.clone(),
                    },
                );
                profiles_menu.append(&item)?;
            }
        }
        quick.append(&profiles_menu)?;

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
        pick(lang, "重载配置", "Reload config"),
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
    let open_config = MenuItem::with_id(
        id_open_config.clone(),
        pick(lang, "打开配置文件", "Open config file"),
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
        &open_config,
        &open_logs,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    Ok(menu)
}

fn format_config_label(cfg: &crate::dashboard_core::ConfigOption) -> String {
    let mut label = format!("L{} {}", cfg.level.clamp(1, 10), cfg.name);
    if let Some(alias) = cfg.alias.as_deref()
        && !alias.trim().is_empty()
    {
        label.push_str(&format!(" ({alias})"));
    }
    if !cfg.enabled {
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
