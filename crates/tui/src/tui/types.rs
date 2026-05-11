use crate::tui::Language;
use crate::tui::i18n::{self, msg};

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
    fn label_en(self) -> &'static str {
        match self {
            EffortChoice::Clear => "Clear (use request value)",
            EffortChoice::Low => "low",
            EffortChoice::Medium => "medium",
            EffortChoice::High => "high",
            EffortChoice::XHigh => "xhigh",
        }
    }

    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        i18n::label(lang, self.label_en())
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
    fn label_en(self) -> &'static str {
        match self {
            ServiceTierChoice::Clear => "Clear (use request/binding value)",
            ServiceTierChoice::Default => "default",
            ServiceTierChoice::Priority => "priority (fast)",
            ServiceTierChoice::Flex => "flex",
        }
    }

    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        i18n::label(lang, self.label_en())
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
        i18n::text(lang, msg::PAGE_DASHBOARD),
        if uses_route_graph_routing {
            i18n::text(lang, msg::PAGE_ROUTING)
        } else {
            i18n::text(lang, msg::PAGE_STATIONS)
        },
        i18n::text(lang, msg::PAGE_SESSIONS),
        i18n::text(lang, msg::PAGE_REQUESTS),
        i18n::text(lang, msg::PAGE_STATS),
        i18n::text(lang, msg::PAGE_SETTINGS),
        i18n::text(lang, msg::PAGE_HISTORY),
        i18n::text(lang, msg::PAGE_RECENT),
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
