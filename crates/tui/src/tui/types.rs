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
