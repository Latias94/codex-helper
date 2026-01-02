mod i18n;
mod input;
mod model;
mod report;
mod state;
mod terminal;
mod types;
mod view;

pub(crate) use i18n::Language;
pub(crate) use i18n::{detect_system_language, parse_language};
#[allow(unused_imports)]
pub use model::{ProviderOption, UpstreamSummary, build_provider_options};

use std::io;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::watch;

use crate::state::ProxyState;

use self::model::{Palette, now_ms, refresh_snapshot};
use self::state::UiState;
use self::terminal::TerminalGuard;

pub async fn run_dashboard(
    state: Arc<ProxyState>,
    service_name: &'static str,
    port: u16,
    providers: Vec<ProviderOption>,
    language: Language,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let refresh_ms = std::env::var("CODEX_HELPER_TUI_REFRESH_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(500)
        .clamp(100, 5_000);

    let io_timeout = Duration::from_millis((refresh_ms / 2).clamp(50, 250));

    let mut term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut ui = UiState {
        service_name,
        port,
        language,
        refresh_ms,
        ..Default::default()
    };
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let mut snapshot = refresh_snapshot(&state, service_name, ui.stats_days).await;
    let mut providers = providers;
    ui.clamp_selection(&snapshot, providers.len());

    let mut should_redraw = true;
    let mut last_drawn_page = ui.page;
    loop {
        if should_redraw {
            if ui.page != last_drawn_page {
                // Defensive: some terminals occasionally leave stale cells when only a small
                // region changes (e.g., switching tabs). A full clear on page switch keeps the
                // UI visually consistent without clearing on every tick.
                terminal.clear()?;
                last_drawn_page = ui.page;
            }
            terminal.draw(|f| {
                view::render_app(
                    f,
                    palette,
                    &mut ui,
                    &snapshot,
                    service_name,
                    port,
                    &providers,
                )
            })?;
            should_redraw = false;
        }

        if ui.should_exit || *shutdown_rx.borrow() {
            let _ = shutdown.send(true);
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                let refresh = refresh_snapshot(&state, service_name, ui.stats_days);
                if let Ok(new_snapshot) = tokio::time::timeout(io_timeout, refresh).await {
                    snapshot = new_snapshot;
                    ui.clamp_selection(&snapshot, providers.len());
                }
                if ui.page == crate::tui::types::Page::Settings
                    && ui
                        .last_runtime_config_refresh_at
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    let url =
                        format!("http://127.0.0.1:{}/__codex_helper/config/runtime", ui.port);
                    let fetch = async {
                        let client = reqwest::Client::new();
                        client
                            .get(&url)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<serde_json::Value>()
                            .await
                    };
                    if let Ok(Ok(v)) = tokio::time::timeout(io_timeout, fetch).await {
                        ui.last_runtime_config_loaded_at_ms =
                            v.get("loaded_at_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_config_source_mtime_ms =
                            v.get("source_mtime_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_retry = v
                            .get("retry")
                            .and_then(|x| serde_json::from_value(x.clone()).ok());
                    }
                    ui.last_runtime_config_refresh_at = Some(Instant::now());
                }
                should_redraw = true;
            }
            changed = shutdown_rx.changed() => {
                let _ = changed;
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            _ = &mut ctrl_c => {
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        if input::handle_key_event(state.clone(), &mut providers, &mut ui, &snapshot, key).await {
                            if ui.needs_snapshot_refresh {
                                snapshot = refresh_snapshot(&state, service_name, ui.stats_days).await;
                                ui.clamp_selection(&snapshot, providers.len());
                                ui.needs_snapshot_refresh = false;
                            }
                            if ui.needs_codex_history_refresh {
                                ui.codex_history_error = None;
                                match crate::sessions::find_codex_sessions_for_current_dir(200).await {
                                    Ok(list) => {
                                        ui.codex_history_sessions = list;
                                        ui.codex_history_loaded_at_ms = Some(now_ms());
                                        ui.selected_codex_history_idx = 0;
                                        ui.codex_history_table.select(if ui.codex_history_sessions.is_empty() {
                                            None
                                        } else {
                                            Some(0)
                                        });
                                        let count = ui.codex_history_sessions.len();
                                        let zh = format!("history: 已加载 {count} 个会话");
                                        let en = format!("history: loaded {count} sessions");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en).to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                    Err(e) => {
                                        ui.codex_history_sessions.clear();
                                        ui.codex_history_loaded_at_ms = Some(now_ms());
                                        ui.codex_history_error = Some(e.to_string());
                                        let zh = format!("history: 加载失败：{e}");
                                        let en = format!("history: load failed: {e}");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en).to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                }
                                ui.needs_codex_history_refresh = false;
                            }
                            should_redraw = true;
                        }
                    }
                    Event::Resize(_, _) => {
                        should_redraw = true;
                    }
                    _ => {}
                }
            }
        }
    }

    terminal.show_cursor()?;
    crossterm::terminal::disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    term_guard.disarm();
    Ok(())
}
