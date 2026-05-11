#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Focus {
    Sessions,
    Requests,
    Stations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum StatsFocus {
    Stations,
    Providers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Page {
    Dashboard,
    Stations,
    Sessions,
    Requests,
    Stats,
    Settings,
    History,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Overlay {
    None,
    Help,
    EffortMenu,
    ModelMenuSession,
    ModelInputSession,
    ServiceTierMenuSession,
    ServiceTierInputSession,
    ProfileMenuSession,
    ProfileMenuDefaultRuntime,
    ProfileMenuDefaultPersisted,
    ProviderMenuSession,
    ProviderMenuGlobal,
    RoutingMenu,
    StationInfo,
    SessionTranscript,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum ServiceTierChoice {
    Clear,
    Default,
    Priority,
    Flex,
}

impl ServiceTierChoice {
    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            ServiceTierChoice::Clear => "Clear (use request/binding value)",
            ServiceTierChoice::Default => "default",
            ServiceTierChoice::Priority => "priority (fast)",
            ServiceTierChoice::Flex => "flex",
        }
    }

    pub(in crate::tui) fn value(self) -> Option<&'static str> {
        match self {
            ServiceTierChoice::Clear => None,
            ServiceTierChoice::Default => Some("default"),
            ServiceTierChoice::Priority => Some("priority"),
            ServiceTierChoice::Flex => Some("flex"),
        }
    }
}

pub(in crate::tui) fn page_titles(
    lang: Language,
    uses_route_graph_routing: bool,
) -> [&'static str; 8] {
    [
        crate::tui::i18n::pick(lang, "1 总览", "1 Dashboard"),
        if uses_route_graph_routing {
            crate::tui::i18n::pick(lang, "2 路由", "2 Routing")
        } else {
            crate::tui::i18n::pick(lang, "2 站点", "2 Stations")
        },
        crate::tui::i18n::pick(lang, "3 会话", "3 Sessions"),
        crate::tui::i18n::pick(lang, "4 请求", "4 Requests"),
        crate::tui::i18n::pick(lang, "5 统计", "5 Stats"),
        crate::tui::i18n::pick(lang, "6 设置", "6 Settings"),
        crate::tui::i18n::pick(lang, "7 历史", "7 History"),
        crate::tui::i18n::pick(lang, "8 最近", "8 Recent"),
    ]
}

pub(in crate::tui) fn page_index(page: Page) -> usize {
    match page {
        Page::Dashboard => 0,
        Page::Stations => 1,
        Page::Sessions => 2,
        Page::Requests => 3,
        Page::Stats => 4,
        Page::Settings => 5,
        Page::History => 6,
        Page::Recent => 7,
    }
}
use crate::tui::Language;
