#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)]

#[path = "../codex_integration.rs"]
mod codex_integration;
#[path = "../config.rs"]
mod config;
#[path = "../dashboard_core/mod.rs"]
mod dashboard_core;
#[path = "../filter.rs"]
mod filter;
#[path = "../healthcheck.rs"]
mod healthcheck;
#[path = "../lb.rs"]
mod lb;
#[path = "../logging.rs"]
mod logging;
#[path = "../model_routing.rs"]
mod model_routing;
#[path = "../notify.rs"]
mod notify;
#[path = "../proxy/mod.rs"]
mod proxy;
#[path = "../sessions.rs"]
mod sessions;
#[path = "../state.rs"]
mod state;
#[path = "../usage.rs"]
mod usage;
#[path = "../usage_providers.rs"]
mod usage_providers;

#[path = "../gui/mod.rs"]
mod gui;

fn main() -> eframe::Result<()> {
    gui::run()
}
