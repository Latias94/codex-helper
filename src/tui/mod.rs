mod input;
mod model;
mod state;
mod terminal;
mod types;
mod view;

pub use model::ProviderOption;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::watch;

use crate::state::ProxyState;

use self::model::{Palette, refresh_snapshot};
use self::state::UiState;
use self::terminal::TerminalGuard;

pub async fn run_dashboard(
    state: Arc<ProxyState>,
    service_name: &'static str,
    port: u16,
    providers: Vec<ProviderOption>,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let refresh_ms = std::env::var("CODEX_HELPER_TUI_REFRESH_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(500)
        .clamp(100, 5_000);

    let mut term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut ui = UiState::default();
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut snapshot = refresh_snapshot(&state).await;
    ui.clamp_selection(&snapshot);

    let mut should_redraw = true;
    loop {
        if should_redraw {
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
                snapshot = refresh_snapshot(&state).await;
                ui.clamp_selection(&snapshot);
                should_redraw = true;
            }
            changed = shutdown_rx.changed() => {
                let _ = changed;
                ui.should_exit = true;
                should_redraw = true;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        if input::handle_key_event(&state, &providers, &mut ui, &snapshot, key).await {
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
