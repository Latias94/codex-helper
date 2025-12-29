#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Focus {
    Sessions,
    Requests,
    Configs,
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

pub(in crate::tui) fn page_titles() -> [&'static str; 6] {
    [
        "Dashboard",
        "Configs",
        "Sessions",
        "Requests",
        "Stats",
        "Settings",
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
