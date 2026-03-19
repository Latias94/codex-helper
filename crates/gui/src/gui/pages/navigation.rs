use super::remote_attach::remote_safe_surface_status_line;
use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct NavItemDef {
    pub(super) page: Page,
    pub(super) zh: &'static str,
    pub(super) en: &'static str,
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
pub(super) struct NavGroupDef {
    pub(super) title_zh: &'static str,
    pub(super) title_en: &'static str,
    pub(super) items: &'static [NavItemDef],
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
        page: Page::ProxySettings,
        zh: "代理设置",
        en: "Proxy Settings",
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
        title_zh: "代理设置工作区",
        title_en: "Proxy Settings Workspace",
        items: &NAV_WORKSPACE_ITEMS,
    },
];

pub(super) fn page_nav_groups() -> &'static [NavGroupDef] {
    &NAV_GROUPS
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

pub(super) fn render_nav(
    ui: &mut egui::Ui,
    lang: Language,
    current: &mut Page,
    proxy: &super::super::proxy_control::ProxyController,
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
