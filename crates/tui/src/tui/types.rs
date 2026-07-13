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
    ProviderEndpoints,
    Providers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Page {
    Dashboard,
    Stations,
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
    StationInfo,
    SessionTranscript,
    StartupAlert,
}

pub(in crate::tui) fn page_titles(
    lang: Language,
    uses_route_graph_routing: bool,
) -> [&'static str; 10] {
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
        Page::Stations => 1,
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
