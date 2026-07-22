use crate::tui::Language;
use crate::tui::i18n::{self, msg};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Focus {
    Sessions,
    Requests,
    Providers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum StatsFocus {
    Pools,
    Projects,
    ProviderEndpoints,
    Providers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Page {
    Dashboard,
    Routing,
    Sessions,
    Requests,
    Stats,
    ServiceStatus,
    Settings,
    History,
    Recent,
    Fleet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Overlay {
    None,
    Help,
    ProviderInfo,
    SessionTranscript,
    StartupAlert,
    RoutingActions,
    RoutingConfirmation,
    SessionAffinityActions,
    SessionAffinityConfirmation,
    SessionProfileMenu,
    SessionModelMenu,
    SessionEffortMenu,
    SessionServiceTierMenu,
    SessionBindingInput,
    ConfiguredDefaultProfileMenu,
    RuntimeDefaultProfileMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum SessionBindingInputKind {
    Model,
    ServiceTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum SessionEffortChoice {
    Clear,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl SessionEffortChoice {
    pub(in crate::tui) const ALL: [Self; 6] = [
        Self::Clear,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
    ];

    pub(in crate::tui) fn value(self) -> Option<&'static str> {
        match self {
            Self::Clear => None,
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
        }
    }

    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        match (lang, self) {
            (Language::Zh, Self::Clear) => "清除（使用请求值）",
            (Language::En, Self::Clear) => "Clear (use request value)",
            (_, Self::Minimal) => "minimal",
            (_, Self::Low) => "low",
            (_, Self::Medium) => "medium",
            (_, Self::High) => "high",
            (_, Self::XHigh) => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum SessionServiceTierChoice {
    Clear,
    Default,
    Fast,
    Flex,
}

impl SessionServiceTierChoice {
    pub(in crate::tui) const ALL: [Self; 4] = [Self::Clear, Self::Default, Self::Fast, Self::Flex];

    pub(in crate::tui) fn value(self) -> Option<&'static str> {
        match self {
            Self::Clear => None,
            Self::Default => Some("default"),
            Self::Fast => Some("fast"),
            Self::Flex => Some("flex"),
        }
    }

    pub(in crate::tui) fn label(self, lang: Language) -> &'static str {
        match (lang, self) {
            (Language::Zh, Self::Clear) => "清除（使用请求值）",
            (Language::En, Self::Clear) => "Clear (use request value)",
            (_, Self::Default) => "default",
            (Language::Zh, Self::Fast) => "fast（上游 priority）",
            (Language::En, Self::Fast) => "fast (upstream priority)",
            (_, Self::Flex) => "flex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum RoutingActionChoice {
    PreferNewSessions,
    ClearNewSessionPreference,
    EnableEndpoint,
    DrainEndpoint,
    DisableEndpoint,
}

impl RoutingActionChoice {
    pub(in crate::tui) const ALL: [Self; 5] = [
        Self::PreferNewSessions,
        Self::ClearNewSessionPreference,
        Self::EnableEndpoint,
        Self::DrainEndpoint,
        Self::DisableEndpoint,
    ];
}

pub(in crate::tui) fn page_titles(lang: Language) -> [&'static str; 10] {
    [
        i18n::text(lang, msg::PAGE_DASHBOARD),
        i18n::text(lang, msg::PAGE_ROUTING),
        i18n::text(lang, msg::PAGE_SESSIONS),
        i18n::text(lang, msg::PAGE_REQUESTS),
        i18n::text(lang, msg::PAGE_STATS),
        i18n::text(lang, msg::PAGE_SERVICE_STATUS),
        i18n::text(lang, msg::PAGE_SETTINGS),
        i18n::text(lang, msg::PAGE_HISTORY),
        i18n::text(lang, msg::PAGE_RECENT),
        i18n::text(lang, msg::PAGE_FLEET),
    ]
}

pub(in crate::tui) fn page_index(page: Page) -> usize {
    match page {
        Page::Dashboard => 0,
        Page::Routing => 1,
        Page::Sessions => 2,
        Page::Requests => 3,
        Page::Stats => 4,
        Page::ServiceStatus => 5,
        Page::Settings => 6,
        Page::History => 7,
        Page::Recent => 8,
        Page::Fleet => 9,
    }
}
