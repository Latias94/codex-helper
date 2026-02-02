use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

use super::i18n::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Hide,
    Toggle,
    StartProxy,
    StopProxy,
    ReloadConfig,
    OpenConfig,
    OpenLogs,
    Quit,
}

pub struct TrayController {
    _tray: TrayIcon,
    id_show: MenuId,
    id_hide: MenuId,
    id_toggle: MenuId,
    id_start: MenuId,
    id_stop: MenuId,
    id_reload: MenuId,
    id_open_config: MenuId,
    id_open_logs: MenuId,
    id_quit: MenuId,
}

impl TrayController {
    pub fn try_new(lang: Language) -> anyhow::Result<Self> {
        let icon = default_icon()?;

        let id_show = MenuId::new("codex-helper-gui.tray.show");
        let id_hide = MenuId::new("codex-helper-gui.tray.hide");
        let id_toggle = MenuId::new("codex-helper-gui.tray.toggle");
        let id_start = MenuId::new("codex-helper-gui.tray.start_proxy");
        let id_stop = MenuId::new("codex-helper-gui.tray.stop_proxy");
        let id_reload = MenuId::new("codex-helper-gui.tray.reload_config");
        let id_open_config = MenuId::new("codex-helper-gui.tray.open_config");
        let id_open_logs = MenuId::new("codex-helper-gui.tray.open_logs");
        let id_quit = MenuId::new("codex-helper-gui.tray.quit");

        let menu = Menu::new();
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
            &start,
            &stop,
            &reload,
            &PredefinedMenuItem::separator(),
            &open_config,
            &open_logs,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("codex-helper")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()?;

        Ok(Self {
            _tray: tray,
            id_show,
            id_hide,
            id_toggle,
            id_start,
            id_stop,
            id_reload,
            id_open_config,
            id_open_logs,
            id_quit,
        })
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
        } else if id == &self.id_start {
            Some(TrayAction::StartProxy)
        } else if id == &self.id_stop {
            Some(TrayAction::StopProxy)
        } else if id == &self.id_reload {
            Some(TrayAction::ReloadConfig)
        } else if id == &self.id_open_config {
            Some(TrayAction::OpenConfig)
        } else if id == &self.id_open_logs {
            Some(TrayAction::OpenLogs)
        } else if id == &self.id_quit {
            Some(TrayAction::Quit)
        } else {
            None
        }
    }
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
