#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Focus {
    Sessions,
    Requests,
    Configs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum StatsFocus {
    Configs,
    Providers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Page {
    Dashboard,
    Configs,
    Sessions,
    Requests,
    Stats,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Overlay {
    None,
    Help,
    EffortMenu,
    ProviderMenuSession,
    ProviderMenuGlobal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum EffortChoice {
    Clear,
    Low,
    Medium,
    High,
    XHigh,
}

impl EffortChoice {
    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            EffortChoice::Clear => "Clear (use request value)",
            EffortChoice::Low => "low",
            EffortChoice::Medium => "medium",
            EffortChoice::High => "high",
            EffortChoice::XHigh => "xhigh",
        }
    }

    pub(in crate::tui) fn value(self) -> Option<&'static str> {
        match self {
            EffortChoice::Clear => None,
            EffortChoice::Low => Some("low"),
            EffortChoice::Medium => Some("medium"),
            EffortChoice::High => Some("high"),
            EffortChoice::XHigh => Some("xhigh"),
        }
    }
}

pub(in crate::tui) fn page_titles(lang: Language) -> [&'static str; 6] {
    [
        crate::tui::i18n::pick(lang, "1 总览", "1 Dashboard"),
        crate::tui::i18n::pick(lang, "2 配置", "2 Configs"),
        crate::tui::i18n::pick(lang, "3 会话", "3 Sessions"),
        crate::tui::i18n::pick(lang, "4 请求", "4 Requests"),
        crate::tui::i18n::pick(lang, "5 统计", "5 Stats"),
        crate::tui::i18n::pick(lang, "6 设置", "6 Settings"),
    ]
}

pub(in crate::tui) fn page_index(page: Page) -> usize {
    match page {
        Page::Dashboard => 0,
        Page::Configs => 1,
        Page::Sessions => 2,
        Page::Requests => 3,
        Page::Stats => 4,
        Page::Settings => 5,
    }
}
use crate::tui::Language;
