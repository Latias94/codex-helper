mod app;
mod autostart;
mod config;
mod i18n;
mod pages;
mod proxy_control;
mod single_instance;
mod tray;
mod util;

pub fn run() -> eframe::Result<()> {
    app::run()
}
