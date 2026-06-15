//! `ox-monitor-tui` — Terminal dashboard for OxyMake.
//!
//! Provides a ratatui-based TUI that displays live pipeline progress,
//! running jobs, and event logs.  Data comes from the SQLite state.db
//! (read-only, WAL mode) written by the OxyMake engine, or from NDJSON
//! events piped from `ox run --json`.
//!
//! # Architecture
//!
//! ```text
//! state.db ──(poll 500ms)──> App ──(render)──> Terminal
//!                             ^
//!                             │
//!                         Key events
//! ```
//!
//! Alternatively:
//!
//! ```text
//! ox run --json ──(stdin)──> App ──(render)──> Terminal
//! ```
//!
//! The [`App`] struct holds all displayable state.  Each tick it polls
//! the database (or reads from stdin), then the UI module renders a
//! frame from that snapshot.

pub mod app;
pub mod format;
pub mod ui;

use std::io::{self, BufRead};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ox_state::db::StateDb;

use app::App;

/// Run the TUI main loop, polling `db_path` for state updates.
///
/// Opens a read-only connection to the SQLite state database and
/// refreshes the app state every 500ms.
///
/// Returns when the user presses `q`.
pub async fn run_tui(db_path: &Path) -> Result<()> {
    let db = StateDb::open(db_path)?;
    let mut terminal = ratatui::init();
    let mut app = App::new();

    let result = run_loop_db(&mut terminal, &mut app, &db);

    ratatui::restore();
    result
}

/// Run the TUI main loop, reading NDJSON events from stdin.
///
/// Each JSON line is parsed as an event and applied to the app state.
/// The UI renders after each event or on a 250ms poll timeout.
///
/// Returns when the user presses `q` or stdin is closed.
pub async fn run_tui_stdin() -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new();

    let result = run_loop_stdin(&mut terminal, &mut app);

    ratatui::restore();
    result
}

/// Run the TUI main loop with mock data (for development).
///
/// Returns when the user presses `q`.
pub async fn run_tui_mock() -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::with_mock_data();

    let result = run_loop_mock(&mut terminal, &mut app);

    ratatui::restore();
    result
}

/// Inner loop for database-backed mode.
fn run_loop_db(terminal: &mut ratatui::DefaultTerminal, app: &mut App, db: &StateDb) -> Result<()> {
    let mut last_refresh = Instant::now() - Duration::from_secs(1); // Force immediate refresh

    while app.running {
        // Refresh from database every 500ms.
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            if let Err(e) = app.refresh_from_db(db) {
                // Log the error as an event but keep running.
                app.recent_events.insert(
                    0,
                    app::EventLine {
                        timestamp: String::new(),
                        icon: '\u{2717}',
                        message: format!("DB refresh error: {e}"),
                    },
                );
            }
            last_refresh = Instant::now();
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key.code);
            }
        }
    }
    Ok(())
}

/// Inner loop for NDJSON stdin mode.
fn run_loop_stdin(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    // Use a channel + background thread for non-blocking stdin reads.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let _reader_thread = std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    while app.running {
        // Drain available NDJSON lines (non-blocking).
        while let Ok(line) = rx.try_recv() {
            if !line.trim().is_empty() {
                let _ = app.apply_ndjson_event(&line);
            }
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key.code);
            }
        }
    }
    Ok(())
}

/// Inner loop for mock mode (no data source).
fn run_loop_mock(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while app.running {
        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key.code);
            }
        }
    }
    Ok(())
}

/// Process a single key press.
fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') => app.running = false,
        KeyCode::Up => app.prev_panel(),
        KeyCode::Down => app.next_panel(),
        KeyCode::Tab => app.next_panel(),
        KeyCode::BackTab => app.prev_panel(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_key_quit() {
        let mut app = App::new();
        assert!(app.running);
        handle_key(&mut app, KeyCode::Char('q'));
        assert!(!app.running);
    }

    #[test]
    fn handle_key_navigation() {
        let mut app = App::new();
        assert_eq!(app.selected_panel, app::Panel::Pipeline);

        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_panel, app::Panel::RunningJobs);

        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_panel, app::Panel::Events);

        // Wraps around.
        handle_key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_panel, app::Panel::Pipeline);

        // Backwards.
        handle_key(&mut app, KeyCode::Up);
        assert_eq!(app.selected_panel, app::Panel::Events);
    }
}
