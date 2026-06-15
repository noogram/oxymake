//! UI rendering — pure function from [`App`] to terminal frame.
//!
//! Each public function draws one panel.  [`render`] composes them
//! into the full layout.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, List, ListItem, Row, Table},
};

use crate::app::{App, Panel};
use crate::format::{fmt_duration, fmt_progress};

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

const HIGHLIGHT: Color = Color::Cyan;
const SUCCESS: Color = Color::Green;
const FAILURE: Color = Color::Red;
const RUNNING_COLOR: Color = Color::Yellow;
const DIMMED: Color = Color::DarkGray;

// ---------------------------------------------------------------------------
// Top-level render
// ---------------------------------------------------------------------------

/// Render the full dashboard into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // pipeline
            Constraint::Min(4),    // running jobs
            Constraint::Min(4),    // events
        ])
        .split(frame.area());

    render_header(frame, app, chunks[0]);
    render_pipeline(frame, app, chunks[1]);
    render_running_jobs(frame, app, chunks[2]);
    render_events(frame, app, chunks[3]);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let done = app.job_counts.done();
    let total = app.job_counts.total();
    let progress_text = fmt_progress(done, total);
    let elapsed_text = fmt_duration(app.elapsed);

    let header_text = format!(
        " Run: #{} | {} | {}",
        app.run_id, elapsed_text, progress_text,
    );

    let pct = app.job_counts.progress_fraction();
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(" OxyMake ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(HIGHLIGHT)),
        )
        .gauge_style(Style::default().fg(HIGHLIGHT).bg(Color::Black))
        .percent((pct * 100.0) as u16)
        .label(header_text);

    frame.render_widget(gauge, area);
}

// ---------------------------------------------------------------------------
// Pipeline panel
// ---------------------------------------------------------------------------

fn render_pipeline(frame: &mut Frame, app: &App, area: Rect) {
    let is_selected = app.selected_panel == Panel::Pipeline;
    let border_style = panel_border_style(is_selected);

    let block = Block::default()
        .title(" Pipeline ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let rows: Vec<Row> = app
        .pipeline_stats
        .iter()
        .map(|stage| {
            let bar = progress_bar(stage.progress_fraction(), 20);
            let counts = format!("{}/{}", stage.completed, stage.total);
            let status = stage.status_label();
            let status_style = if stage.completed == stage.total {
                Style::default().fg(SUCCESS)
            } else if stage.running > 0 {
                Style::default().fg(RUNNING_COLOR)
            } else {
                Style::default().fg(DIMMED)
            };

            Row::new(vec![
                Cell::from(stage.name.clone()),
                Cell::from(bar),
                Cell::from(counts),
                Cell::from(Span::styled(status, status_style)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(22),
        Constraint::Length(12),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths).block(block);
    frame.render_widget(table, area);
}

/// Build a simple ASCII progress bar of the given `width`.
fn progress_bar(fraction: f64, width: usize) -> String {
    let filled = ((fraction * width as f64).round() as usize).min(width);
    let empty = width - filled;
    let mut bar = String::with_capacity(width);
    for _ in 0..filled {
        bar.push('\u{2588}'); // full block
    }
    for _ in 0..empty {
        bar.push('\u{2591}'); // light shade
    }
    bar
}

// ---------------------------------------------------------------------------
// Running Jobs panel
// ---------------------------------------------------------------------------

fn render_running_jobs(frame: &mut Frame, app: &App, area: Rect) {
    let is_selected = app.selected_panel == Panel::RunningJobs;
    let border_style = panel_border_style(is_selected);

    let block = Block::default()
        .title(format!(" Running Jobs ({}) ", app.running_jobs.len()))
        .borders(Borders::ALL)
        .border_style(border_style);

    let rows: Vec<Row> = app
        .running_jobs
        .iter()
        .map(|job| {
            Row::new(vec![
                Cell::from(job.id.clone()),
                Cell::from(fmt_duration(job.duration)),
                Cell::from(job.resources.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(30),
        Constraint::Length(10),
        Constraint::Length(16),
    ];

    let table = Table::new(rows, widths).block(block);
    frame.render_widget(table, area);
}

// ---------------------------------------------------------------------------
// Events panel
// ---------------------------------------------------------------------------

fn render_events(frame: &mut Frame, app: &App, area: Rect) {
    let is_selected = app.selected_panel == Panel::Events;
    let border_style = panel_border_style(is_selected);

    let block = Block::default()
        .title(" Events ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let items: Vec<ListItem> = app
        .recent_events
        .iter()
        .map(|evt| {
            let style = match evt.icon {
                '\u{2713}' => Style::default().fg(SUCCESS),
                '\u{2717}' => Style::default().fg(FAILURE),
                '\u{2192}' => Style::default().fg(RUNNING_COLOR),
                _ => Style::default().fg(DIMMED),
            };
            let line = Line::from(vec![
                Span::styled(format!("{} ", evt.timestamp), Style::default().fg(DIMMED)),
                Span::styled(format!("{} ", evt.icon), style),
                Span::styled(evt.message.clone(), style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn panel_border_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(HIGHLIGHT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIMMED)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn render_empty_app_does_not_panic() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::new();
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn render_mock_data_does_not_panic() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::with_mock_data();
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn progress_bar_edges() {
        assert_eq!(progress_bar(0.0, 10).chars().count(), 10);
        assert_eq!(progress_bar(1.0, 10).chars().count(), 10);
        assert_eq!(
            progress_bar(0.5, 10)
                .chars()
                .filter(|&c| c == '\u{2588}')
                .count(),
            5
        );
    }

    #[test]
    fn render_with_real_db_data_does_not_panic() {
        use ox_state::db::{JobRecord, StateDb};
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        let db = StateDb::open(tmp.path()).unwrap();
        let sid = db.create_session(1, "host", None).unwrap();

        let jobs: Vec<JobRecord> = (0..5)
            .map(|i| JobRecord {
                id: format!("j{i}"),
                rule_name: "build".into(),
                wildcards: "{}".into(),
                cache_key: None,
                run_id: None,
            })
            .collect();
        db.register_jobs(&jobs).unwrap();
        db.claim_job("j0", &sid).unwrap();
        db.complete_job("j0", &sid, 0, "{}").unwrap();
        db.claim_job("j1", &sid).unwrap(); // running

        let mut app = App::new();
        app.refresh_from_db(&db).unwrap();

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }
}
