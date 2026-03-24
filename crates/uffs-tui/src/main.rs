//! UFFS (Ultra Fast File Search) TUI
//!
//! Interactive terminal user interface for file search.
//!
//! ## Usage
//!
//! ```bash
//! # Load MFT files (cross-platform)
//! uffs_tui --mft-file C_mft.iocp --drive C
//! uffs_tui --mft-file C.iocp,D.iocp
//!
//! # Windows: auto-detect NTFS drives (future)
//! uffs_tui
//! ```
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for info-level terminal output.
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
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

/// Application state and search logic.
mod app;
/// Search backend: MftIndex-backed multi-drive search.
mod backend;

use app::App;

/// UFFS (Ultra Fast File Search) Terminal UI
#[derive(Parser)]
#[command(name = "uffs_tui")]
#[command(
    author,
    version,
    about = "Terminal UI for UFFS (Ultra Fast File Search)"
)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// MFT file(s) to load — supports raw, IOCP capture, and compressed
    ///
    /// Cross-platform: works on macOS, Linux, and Windows.
    /// Auto-detects format. Drive letter inferred from filename.
    ///
    /// Examples:
    ///   `uffs_tui` `D_mft.iocp`
    ///   `uffs_tui` `C.iocp` `D.iocp`
    ///   `uffs_tui` `/path/to/C_mft.bin` `--drive` C
    #[arg(value_name = "FILE")]
    mft_file: Vec<PathBuf>,

    /// Data directory containing `drive_*` subdirectories with MFT files
    ///
    /// Auto-discovers all MFT files in `drive_c/`, `drive_d/`, etc.
    /// Example: `uffs_tui --data-dir ~/uffs_data`
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Drive letter(s) to override auto-detection from filenames.
    #[arg(long, value_delimiter = ',')]
    drive: Vec<char>,
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
#[expect(
    clippy::too_many_lines,
    reason = "main function orchestrates TUI setup, async loading, and event loop; splitting would fragment cohesive logic"
)]
fn main() -> Result<()> {
    // Check for -v/--verbose flag early
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    let cli = Cli::parse();

    // Discover MFT files from --data-dir if specified
    let mut mft_files = cli.mft_file;
    if let Some(data_dir) = &cli.data_dir {
        if let Ok(entries) = std::fs::read_dir(data_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if let Some(letter) = name.strip_prefix("drive_") {
                    if letter.len() == 1
                        && letter
                            .chars()
                            .next()
                            .is_some_and(|ch| ch.is_ascii_alphabetic())
                    {
                        // Prefer .iocp > .bin > .mft
                        if let Some(best) = find_best_mft_file(&path) {
                            mft_files.push(best);
                        }
                    }
                }
            }
        }
        mft_files.sort();
    }

    // Setup terminal immediately so the TUI is visible during loading
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let ratatui_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(ratatui_backend)?;

    // Create app and start loading MFT files on background threads
    let mut app = App::new();

    if !mft_files.is_empty() {
        app.status = format!("Loading {} MFT file(s)...", mft_files.len());

        // Build load tasks
        let load_tasks: Vec<_> = mft_files
            .iter()
            .enumerate()
            .map(|(idx, path)| (path.clone(), cli.drive.get(idx).copied()))
            .collect();

        // Use a channel to receive loaded drives from background threads
        let (sender, receiver) = std::sync::mpsc::channel();

        // Spawn loading threads
        let _load_handle = std::thread::spawn(move || {
            std::thread::scope(|scope| {
                let senders: Vec<_> = load_tasks
                    .iter()
                    .map(|(file_path, drive_opt)| {
                        let thread_sender = sender.clone();
                        let thread_path = file_path.clone();
                        let thread_drive = *drive_opt;
                        scope.spawn(move || {
                            let result = backend::load_mft_file(&thread_path, thread_drive);
                            let file_name = thread_path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("?")
                                .to_owned();
                            drop(thread_sender.send((file_name, result)));
                        })
                    })
                    .collect();
                for handle in senders {
                    drop(handle.join());
                }
            });
        });

        // Poll for loaded drives while rendering the TUI
        let total_files = mft_files.len();
        let mut loaded_count = 0_usize;
        let load_start = std::time::Instant::now();

        while loaded_count < total_files {
            // Render current state
            terminal.draw(|frame| ui(frame, &app))?;

            // Check for loaded drives (non-blocking)
            while let Ok((file_name, result)) = receiver.try_recv() {
                loaded_count += 1;
                match result {
                    Ok(drive_index) => {
                        let msg = format!(
                            "✅ Drive {}: {} records, {} paths, {} trigrams ({})",
                            drive_index.letter,
                            drive_index.index.records.len(),
                            drive_index
                                .paths_lower
                                .iter()
                                .filter(|path| !path.is_empty())
                                .count(),
                            drive_index.trigram.posting_count(),
                            file_name,
                        );
                        app.backend.drives.push(drive_index);
                        // Show progress as search results
                        app.results.push(backend::DisplayRow {
                            drive: ' ',
                            path: String::new(),
                            name: msg,
                            size: 0,
                            is_directory: false,
                            modified: 0,
                        });
                    }
                    Err(err) => {
                        app.results.push(backend::DisplayRow {
                            drive: ' ',
                            path: String::new(),
                            name: format!("❌ {file_name}: {err}"),
                            size: 0,
                            is_directory: false,
                            modified: 0,
                        });
                    }
                }
                app.status = format!(
                    "Loading... {loaded_count}/{total_files} drives ({} records, {:.1}s)",
                    app.backend.total_records(),
                    load_start.elapsed().as_secs_f64()
                );
            }

            // Handle input during loading — text box is always active
            if event::poll(core::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if is_exit_key(key) {
                            disable_raw_mode()?;
                            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                            terminal.show_cursor()?;
                            return Ok(());
                        }
                        #[expect(
                            clippy::wildcard_enum_match_arm,
                            reason = "only handling text input during loading; other keys are ignored"
                        )]
                        match key.code {
                            KeyCode::Char(ch) => app.input.push(ch),
                            KeyCode::Backspace => {
                                app.input.pop();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Loading complete — clear progress and show summary
        app.results.clear();
        let elapsed = load_start.elapsed();
        app.status = format!(
            "Loaded {} drive(s), {} records in {:.1}s — type to search",
            app.backend.drives.len(),
            app.backend.total_records(),
            elapsed.as_secs_f64()
        );

        // If user typed a pattern during loading, search immediately
        if !app.input.is_empty() {
            app.search();
        }
    }

    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
    let mut needs_search = false;

    loop {
        // 1. Always render first — input box is always up-to-date
        terminal.draw(|frame| ui(frame, app))?;

        // 2. If search is pending, drain ALL buffered keystrokes first so the input box
        //    stays responsive even if search is slow.
        if needs_search {
            // Drain any queued keystrokes (non-blocking)
            while event::poll(core::time::Duration::ZERO)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if is_exit_key(key) {
                            return Ok(());
                        }
                        match key.code {
                            KeyCode::Char(ch) => app.input.push(ch),
                            KeyCode::Backspace => {
                                app.input.pop();
                            }
                            KeyCode::Down => app.next(),
                            KeyCode::Up => app.previous(),
                            KeyCode::Tab => app.cycle_sort(),
                            KeyCode::BackTab => app.toggle_sort_direction(),
                            _ => {}
                        }
                    }
                }
            }

            // Re-render with ALL accumulated input BEFORE searching
            terminal.draw(|frame| ui(frame, app))?;

            // Now search (blocks, but user already sees their typed text)
            app.search();
            needs_search = false;
            continue;
        }

        // 3. Wait for next keystroke (with debounce timeout)
        if event::poll(core::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if is_exit_key(key) {
                        return Ok(());
                    }

                    match key.code {
                        KeyCode::Char(ch) => {
                            app.input.push(ch);
                            needs_search = true;
                        }
                        KeyCode::Backspace => {
                            app.input.pop();
                            needs_search = true;
                        }
                        KeyCode::Down => app.next(),
                        KeyCode::Up => app.previous(),
                        KeyCode::Enter => app.search(),
                        KeyCode::Tab => app.cycle_sort(),
                        KeyCode::BackTab => app.toggle_sort_direction(),
                        KeyCode::F(2) => {
                            app.toggle_name_only();
                            needs_search = true;
                        }
                        _ => {}
                    }
                }
            }
        } else if needs_search {
            // Debounce expired — no more typing, run search
            app.search();
            needs_search = false;
        }
    }
}

/// Returns whether the given key event should terminate the TUI.
#[must_use]
const fn is_exit_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C')))
}

/// Render the TUI layout: search bar, status, results list, and help bar.
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
#[expect(
    clippy::too_many_lines,
    reason = "UI rendering is a single cohesive function; splitting would fragment layout logic"
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

    // Search input with drive indicators
    let drive_info = app
        .backend
        .drive_summary()
        .iter()
        .map(|(letter, _count)| letter.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let name_only_indicator = if app.name_only { " [NAME]" } else { "" };
    let input_title = if app.has_data() {
        format!(
            " Search [{drive_info}] {}{name_only_indicator} ",
            app.backend.total_records()
        )
    } else {
        " Search (use --mft-file to load data) ".to_owned()
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

    // Sort indicator for title
    let sort_arrow = if app.sort_desc() { "▼" } else { "▲" };
    let sort_label = match app.sort_column() {
        backend::SortColumn::Name => "Name",
        backend::SortColumn::Size => "Size",
        backend::SortColumn::Modified => "Modified",
        backend::SortColumn::Path => "Path",
    };

    // Results list with path and size
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|row| {
            let icon = if row.is_directory { "📁" } else { "📄" };
            // Hide size/path for loading progress messages (drive=' ', path empty)
            if row.drive == ' ' {
                ListItem::new(Line::from(vec![Span::styled(
                    &row.name,
                    Style::default().fg(Color::Cyan),
                )]))
            } else {
                let size_str = format_size(row.size);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", row.drive),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(icon),
                    Span::raw(" "),
                    Span::styled(&row.name, Style::default().fg(Color::Cyan)),
                    Span::raw("  "),
                    Span::styled(size_str, Style::default().fg(Color::Yellow)),
                    Span::raw("  "),
                    Span::styled(
                        truncate_path(&row.path, 60),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            }
        })
        .collect();

    let results = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " Results ({}) — Sort: {sort_label} {sort_arrow} ",
            app.results.len()
        )))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(results, chunks[2], &mut app.list_state.clone());

    // Help bar
    let help = Paragraph::new(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Green)),
        Span::raw(" Nav  "),
        Span::styled("Tab", Style::default().fg(Color::Green)),
        Span::raw(" Sort  "),
        Span::styled("S-Tab", Style::default().fg(Color::Green)),
        Span::raw(" Dir  "),
        Span::styled("F2", Style::default().fg(Color::Green)),
        Span::raw(" Name-only  "),
        Span::styled("Esc/q", Style::default().fg(Color::Green)),
        Span::raw(" Quit"),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Help "));
    frame.render_widget(help, chunks[3]);
}

/// Find the best MFT file in a drive directory, preferring .iocp > .bin > .mft.
#[expect(
    clippy::single_call_fn,
    reason = "called from async loader; separation keeps file discovery logic isolated"
)]
fn find_best_mft_file(dir: &std::path::Path) -> Option<PathBuf> {
    let Ok(files) = std::fs::read_dir(dir) else {
        return None;
    };

    let mut best: Option<(PathBuf, u8)> = None; // (path, priority: 0=iocp, 1=bin, 2=mft)

    for file in files.flatten() {
        let file_path = file.path();
        if !file_path.is_file() {
            continue;
        }
        let Some(ext) = file_path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let priority = match ext {
            "iocp" => 0_u8, // best
            "bin" => 1,
            "mft" => 2,
            _ => continue,
        };
        if best.as_ref().is_none_or(|(_, bp)| priority < *bp) {
            best = Some((file_path, priority));
        }
    }

    best.map(|(path, _)| path)
}

/// Truncate a path string for display, keeping the end visible.
#[expect(
    clippy::single_call_fn,
    reason = "called from ui rendering; separation keeps display formatting isolated"
)]
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.chars().count() <= max_len {
        return path.to_owned();
    }
    let skip = path.chars().count() - max_len + 1;
    let truncated: String = path.chars().skip(skip).collect();
    format!("…{truncated}")
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

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::is_exit_key;

    #[test]
    fn test_is_exit_key_accepts_quit_shortcuts() {
        assert!(is_exit_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(is_exit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(is_exit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn test_is_exit_key_rejects_regular_input() {
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        )));
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
    }
}
