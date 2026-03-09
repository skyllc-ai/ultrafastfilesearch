//! UFFS (Ultra Fast File Search) TUI
//!
//! Interactive terminal user interface for file search.
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for info-level terminal output:
//! ```bash
//! uffs_tui -v --index myindex.parquet
//! ```
//!
//! For finer control, use environment variables:
//! - `RUST_LOG`: Terminal log level (default: `error`, or `info` with `-v`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/uffs/logs`)

#![expect(
    unused_crate_dependencies,
    reason = "tokio is a transitive runtime dependency not directly referenced"
)]
#![expect(
    clippy::option_if_let_else,
    reason = "if-let chains clearer for loading with error handling"
)]

use std::io;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::{EnvFilter, Layer};
use uffs_mft::MftReader;

/// Application state and search logic.
mod app;

use app::App;

/// UFFS (Ultra Fast File Search) Terminal UI
#[derive(Parser)]
#[command(name = "uffs_tui")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Index file to load on startup
    #[arg(short, long)]
    index: Option<PathBuf>,
}

/// Initialize logging with terminal + file support.
///
/// If `verbose` is true and `RUST_LOG` is not set, uses `info` level for
/// terminal. Otherwise, terminal logging is controlled by `RUST_LOG` (default:
/// `error`). File logging is controlled by `RUST_LOG_FILE` (default: `info`).
#[expect(
    clippy::single_call_fn,
    reason = "separated from main for readability; logging setup is a distinct concern"
)]
fn init_logging(verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use std::fs;

    // Get log directory (default: ~/bin/uffs/logs)
    let log_dir = std::env::var("UFFS_LOG_DIR").map_or_else(
        |_| {
            dirs_next::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("bin")
                .join("uffs")
                .join("logs")
        },
        PathBuf::from,
    );

    // Create log directory if it doesn't exist
    drop(fs::create_dir_all(&log_dir));

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "uffs_tui_log_");
    let (non_blocking, guard): (NonBlocking, _) = NonBlocking::new(file_appender);

    // Terminal filter: -v sets info if RUST_LOG not explicitly set
    // Note: TUI uses stderr for logging to avoid interfering with the UI
    let terminal_default = if verbose { "info" } else { "error" };
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| terminal_default.to_owned()));

    // File filter (default: info)
    let file_filter =
        EnvFilter::new(std::env::var("RUST_LOG_FILE").unwrap_or_else(|_| "info".to_owned()));

    // Timer format
    let timer = UtcTime::rfc_3339();

    // Terminal layer (to stderr to avoid TUI interference, with ANSI colors,
    // file/line info)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_timer(timer.clone())
        .with_ansi(true)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(terminal_filter);

    // File layer (no ANSI colors, but with full context)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_timer(timer)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .with_filter(file_filter);

    // Combine layers
    let subscriber = Registry::default().with(terminal_layer).with(file_layer);

    #[expect(
        clippy::expect_used,
        reason = "global subscriber must be set once at startup; failure is unrecoverable"
    )]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber");

    guard
}

/// Entry point: parse CLI, set up terminal, and run the TUI event loop.
fn main() -> Result<()> {
    // Check for -v/--verbose flag early
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    let cli = Cli::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app, optionally loading an index
    let mut app = if let Some(index_path) = cli.index {
        match MftReader::load_parquet(&index_path) {
            Ok(df) => App::with_dataframe(df, Some(index_path)),
            Err(err) => {
                let mut fallback = App::new();
                fallback.error = Some(format!("Failed to load index: {err}"));
                fallback
            }
        }
    } else {
        App::new()
    };

    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        #[expect(
            clippy::print_stderr,
            reason = "terminal is restored at this point; stderr is appropriate for error reporting"
        )]
        #[expect(
            clippy::use_debug,
            reason = "Debug format provides full error chain for diagnostics"
        )]
        {
            eprintln!("Error: {err:?}");
        }
    }

    Ok(())
}

/// Run the TUI event loop, handling key input and rendering.
#[expect(
    clippy::single_call_fn,
    reason = "separated from main for readability; event loop is a distinct concern"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only specific keys are handled; wildcard is idiomatic for key dispatch"
)]
fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: Send + Sync + 'static,
{
    loop {
        terminal.draw(|frame| ui(frame, app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char(ch) => app.input.push(ch),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Down => app.next(),
                    KeyCode::Up => app.previous(),
                    KeyCode::Enter => app.search(),
                    _ => {}
                }
            }
        }
    }
}

/// Render the TUI layout: search bar, status, results list, and help bar.
#[expect(
    clippy::single_call_fn,
    reason = "separated from run_app for readability; UI rendering is a distinct concern"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "layout split guarantees exactly 4 chunks matching the 4 constraints"
)]
#[expect(
    clippy::missing_asserts_for_indexing,
    reason = "layout split guarantees exactly 4 chunks matching the 4 constraints"
)]
#[expect(
    clippy::option_if_let_else,
    reason = "if-let is more readable for widget construction with different layouts per branch"
)]
fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Search input
            Constraint::Length(3), // Status/Error bar
            Constraint::Min(10),   // Results
            Constraint::Length(3), // Help bar
        ])
        .split(frame.area());

    // Search input
    let input_title = if app.has_index() {
        " Search (Enter to search, Esc to quit) "
    } else {
        " Search (load index with --index flag) "
    };
    let input = Paragraph::new(app.input.as_str())
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title(input_title));
    frame.render_widget(input, chunks[0]);

    // Status/Error bar
    let status_content = if let Some(err) = &app.error {
        Line::from(vec![
            Span::styled("Error: ", Style::default().fg(Color::Red)),
            Span::styled(err.as_str(), Style::default().fg(Color::Red)),
        ])
    } else {
        Line::from(vec![Span::styled(
            app.status.as_str(),
            Style::default().fg(Color::Green),
        )])
    };
    let status_bar = Paragraph::new(status_content)
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    frame.render_widget(status_bar, chunks[1]);

    // Results list
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|result| {
            let icon = if result.is_directory { "📁" } else { "📄" };
            let size_str = format_size(result.size);
            ListItem::new(Line::from(vec![
                Span::raw(icon),
                Span::raw(" "),
                Span::styled(&result.name, Style::default().fg(Color::Cyan)),
                Span::raw(" - "),
                Span::styled(size_str, Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    let results = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Results ({}) ", app.results.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(results, chunks[2], &mut app.list_state.clone());

    // Help bar
    let help = Paragraph::new(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Green)),
        Span::raw(" Navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Green)),
        Span::raw(" Search  "),
        Span::styled("q/Esc", Style::default().fg(Color::Green)),
        Span::raw(" Quit"),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Help "));
    frame.render_widget(help, chunks[3]);
}

/// Format file size in human-readable format.
#[expect(
    clippy::single_call_fn,
    reason = "separated for testability and readability"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "f64 provides sufficient precision for human-readable file sizes"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "float division is needed for human-readable size formatting"
)]
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
