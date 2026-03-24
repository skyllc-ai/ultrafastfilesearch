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
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
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

    /// Bypass cache and read MFT fresh (default: use cache + USN updates)
    #[arg(long)]
    no_cache: bool,
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

    // On Windows: auto-detect NTFS drives when no files specified
    #[cfg(windows)]
    let live_drives: Vec<char> = if mft_files.is_empty() && cli.data_dir.is_none() {
        let mut drives = uffs_mft::detect_ntfs_drives();
        // If --drive specified, filter to just those
        if !cli.drive.is_empty() {
            drives.retain(|dr| cli.drive.contains(dr));
        }
        drives
    } else {
        Vec::new()
    };
    #[cfg(not(windows))]
    let live_drives: Vec<char> = Vec::new();

    // Setup terminal immediately so the TUI is visible during loading
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let ratatui_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(ratatui_backend)?;

    // Create app and start loading MFT files on background threads
    let mut app = App::new();

    let total_to_load = mft_files.len() + live_drives.len();
    let cli_no_cache = cli.no_cache;

    if total_to_load > 0 {
        app.status = format!("Loading {total_to_load} drive(s)...");

        // Build load tasks: MFT files + live drives unified
        let file_tasks: Vec<_> = mft_files
            .iter()
            .enumerate()
            .map(|(idx, path)| (path.clone(), cli.drive.get(idx).copied()))
            .collect();

        // Use a channel to receive loaded drives from background threads
        let (sender, receiver) = std::sync::mpsc::channel();

        // Spawn loading threads for both MFT files and live drives
        let _load_handle = std::thread::spawn(move || {
            std::thread::scope(|scope| {
                // Spawn threads for MFT file loading
                let mut handles: Vec<_> = file_tasks
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

                // Spawn threads for live NTFS drives
                // (live_drives is always empty on non-Windows)
                let no_cache_flag = cli_no_cache;
                for drive_letter in live_drives {
                    let thread_sender = sender.clone();
                    handles.push(scope.spawn(move || {
                        let label = format!("LIVE {drive_letter}:");
                        let result = load_live_drive_impl(drive_letter, no_cache_flag);
                        drop(thread_sender.send((label, result)));
                    }));
                }

                for handle in handles {
                    drop(handle.join());
                }
            });
        });

        // Poll for loaded drives while rendering the TUI
        let mut loaded_count = 0_usize;
        let load_start = std::time::Instant::now();

        while loaded_count < total_to_load {
            // Render current state
            terminal.draw(|frame| ui(frame, &mut app))?;

            // Check for loaded drives (non-blocking)
            while let Ok((file_name, result)) = receiver.try_recv() {
                loaded_count += 1;
                match result {
                    Ok((drive_index, timing)) => {
                        let fc = |n: usize| uffs_mft::format_number_commas(n as u64);
                        let msg = format!(
                            "✅ {}:  {:>10} rec  │  mft:{:>7}  paths:{:>7}  tri:{:>7}  │  {:>6} trigrams  ({})",
                            drive_index.letter,
                            fc(drive_index.index.records.len()),
                            format_ms_compact(timing.mft),
                            format_ms_compact(timing.path),
                            format_ms_compact(timing.trigram),
                            fc(drive_index.trigram.posting_count()),
                            file_name,
                        );
                        let dl = drive_index.letter;
                        app.backend.drives.push(drive_index);
                        // Show progress as search results (path empty = loading msg)
                        app.results.push(backend::DisplayRow {
                            drive: dl,
                            path: String::new(),
                            name: msg,
                            size: 0,
                            is_directory: false,
                            modified: 0,
                        });
                    }
                    Err(err) => {
                        app.results.push(backend::DisplayRow {
                            drive: '!',
                            path: String::new(),
                            name: format!("❌ {file_name}: {err}"),
                            size: 0,
                            is_directory: false,
                            modified: 0,
                        });
                    }
                }
                app.status = format!(
                    "Loading... {loaded_count}/{total_to_load} drives ({} records, {})",
                    uffs_mft::format_number_commas(app.backend.total_records() as u64),
                    uffs_mft::format_duration(load_start.elapsed()),
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
                        // Windows/Linux keybindings during loading
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match key.code {
                                KeyCode::Char('u') => {
                                    app.textarea.select_all();
                                    app.textarea.cut();
                                }
                                KeyCode::Char('z') => {
                                    app.textarea.undo();
                                }
                                KeyCode::Char('y') => {
                                    app.textarea.redo();
                                }
                                KeyCode::Char('a') => {
                                    app.textarea.select_all();
                                }
                                KeyCode::Char(_)
                                | KeyCode::Backspace
                                | KeyCode::Enter
                                | KeyCode::Left
                                | KeyCode::Right
                                | KeyCode::Up
                                | KeyCode::Down
                                | KeyCode::Home
                                | KeyCode::End
                                | KeyCode::PageUp
                                | KeyCode::PageDown
                                | KeyCode::Tab
                                | KeyCode::BackTab
                                | KeyCode::Delete
                                | KeyCode::Insert
                                | KeyCode::F(_)
                                | KeyCode::Null
                                | KeyCode::Esc
                                | KeyCode::CapsLock
                                | KeyCode::ScrollLock
                                | KeyCode::NumLock
                                | KeyCode::PrintScreen
                                | KeyCode::Pause
                                | KeyCode::Menu
                                | KeyCode::KeypadBegin
                                | KeyCode::Media(_)
                                | KeyCode::Modifier(_) => {
                                    app.textarea.input(key);
                                }
                            }
                        } else {
                            app.textarea.input(key);
                        }
                    }
                }
            }
        }

        // Loading complete — clear progress and show summary
        app.results.clear();
        let elapsed = load_start.elapsed();
        app.status = format!(
            "Loaded {} drive(s), {} records in {} — type to search",
            app.backend.drives.len(),
            uffs_mft::format_number_commas(app.backend.total_records() as u64),
            uffs_mft::format_duration(elapsed),
        );

        // If user typed a pattern during loading, search immediately
        if !app.input_text().is_empty() {
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
#[expect(
    clippy::too_many_lines,
    reason = "event loop is a single cohesive state machine; splitting would fragment control flow"
)]
fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: Send + Sync + 'static,
{
    let mut needs_search = false;

    loop {
        // 1. Always render first — input box is always up-to-date
        terminal.draw(|frame| ui(frame, &mut *app))?;

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
                            KeyCode::Down => app.next(),
                            KeyCode::Up => app.previous(),
                            KeyCode::Tab => app.cycle_sort(),
                            KeyCode::BackTab => app.toggle_sort_direction(),
                            _ => {
                                app.textarea.input(key);
                            }
                        }
                    }
                }
            }

            // Re-render with ALL accumulated input BEFORE searching
            terminal.draw(|frame| ui(frame, &mut *app))?;

            // Now search (blocks, but user already sees their typed text)
            app.search();
            needs_search = false;
            continue;
        }

        // 3. Wait for next event (with debounce timeout)
        if event::poll(core::time::Duration::from_millis(200))? {
            let ev = event::read()?;
            match &ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_exit_key(*key) {
                        return Ok(());
                    }

                    // Intercept our custom action keys BEFORE textarea
                    match key.code {
                        KeyCode::Down => {
                            app.next();
                            continue;
                        }
                        KeyCode::Up => {
                            app.previous();
                            continue;
                        }
                        KeyCode::PageDown => {
                            app.page_down();
                            continue;
                        }
                        KeyCode::PageUp => {
                            app.page_up();
                            continue;
                        }
                        KeyCode::Enter => {
                            // Show selected path in status bar
                            if let Some(path) = app.selected_path() {
                                app.status = format!("📋 {path}");
                            }
                            continue;
                        }
                        KeyCode::Tab => {
                            app.cycle_sort();
                            continue;
                        }
                        KeyCode::BackTab => {
                            app.toggle_sort_direction();
                            continue;
                        }
                        KeyCode::F(2) => {
                            app.toggle_name_only();
                            needs_search = true;
                            continue;
                        }
                        KeyCode::F(3) => {
                            app.cycle_filter();
                            app.search();
                            continue;
                        }
                        // Ctrl+R: refresh (Wave 3 — full USN + trigram rebuild)
                        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            "🔄 Refresh: planned for Wave 3 (USN + incremental trigram update)"
                                .clone_into(&mut app.status);
                            continue;
                        }
                        // Ctrl+U: clear line (unix-style)
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.textarea.select_all();
                            app.textarea.cut();
                            app.search();
                            continue;
                        }
                        // Ctrl+Z: undo (Windows/Linux convention)
                        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.textarea.undo();
                            needs_search = true;
                            continue;
                        }
                        // Ctrl+Y: redo (Windows/Linux convention)
                        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.textarea.redo();
                            needs_search = true;
                            continue;
                        }
                        // Ctrl+A: select all
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.textarea.select_all();
                            continue;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }

            // Forward ALL other events to textarea (keys, mouse, etc.)
            let before = app.input_text();
            app.textarea.input(ev);
            let after = app.input_text();
            if before != after {
                needs_search = true;
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
    // Ctrl+Q exits — Esc and regular keys go to the textarea
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('q'))
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
    clippy::too_many_lines,
    reason = "UI rendering is a single cohesive function; splitting would fragment layout logic"
)]
fn ui(frame: &mut Frame, app: &mut App) {
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

    // Build drive color map (dynamic palette based on number of drives)
    let drive_colors = backend::build_drive_colors(&app.backend.drives);
    let get_drive_color =
        |letter: char| -> Color { drive_colors.get(&letter).copied().unwrap_or(Color::White) };

    // Search input with drive indicators (sorted, comma-formatted count)
    let mut drive_letters: Vec<char> = app
        .backend
        .drive_summary()
        .iter()
        .map(|(letter, _count)| *letter)
        .collect();
    drive_letters.sort_unstable();
    let name_only_indicator = if app.name_only { " [NAME]" } else { "" };
    let filter_indicator = app.filter_label();
    if app.has_data() {
        // Build colored drive letters for the title
        let mut title_spans: Vec<Span> = vec![Span::raw(" Search NTFS Drives [")];
        for (idx, &letter) in drive_letters.iter().enumerate() {
            if idx > 0 {
                title_spans.push(Span::raw(" "));
            }
            title_spans.push(Span::styled(
                letter.to_string(),
                Style::default()
                    .fg(get_drive_color(letter))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        title_spans.push(Span::raw(format!(
            "] {} Files{name_only_indicator}{filter_indicator} ",
            uffs_mft::format_number_commas(app.backend.total_records() as u64),
        )));
        app.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(Line::from(title_spans)),
        );
    } else {
        app.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search (use --mft-file to load data) "),
        );
    }
    frame.render_widget(&app.textarea, chunks[0]);

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

    // Update page size from actual results area height (minus 3 for borders + header)
    app.page_size = chunks[2].height.saturating_sub(3) as usize;

    // Sort indicator helper — appends ▲/▼ to the active column header
    let sort_arrow = if app.sort_desc() { " ▼" } else { " ▲" };
    let current_sort = app.sort_column();
    let col_header = |col: backend::SortColumn, label: &str| -> Line<'static> {
        if col == current_sort {
            Line::from(vec![
                Span::styled(
                    label.to_owned(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    sort_arrow.to_owned(),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        } else {
            Line::from(Span::styled(
                label.to_owned(),
                Style::default().fg(Color::White),
            ))
        }
    };

    // Build table header row
    let header = Row::new(vec![
        Cell::from(col_header(backend::SortColumn::Drive, "Drv")),
        Cell::from(col_header(backend::SortColumn::Name, "Name")),
        Cell::from(col_header(backend::SortColumn::Size, "Size")),
        Cell::from(col_header(backend::SortColumn::Modified, "Modified")),
        Cell::from(col_header(backend::SortColumn::Path, "Path")),
    ])
    .style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(0);

    let search_term = app.input_text().to_lowercase();

    // Build table rows from results
    let rows: Vec<Row> = app
        .results
        .iter()
        .map(|row| {
            // Loading progress messages (path empty = loading msg)
            if row.path.is_empty() {
                return Row::new(vec![
                    Cell::from(""),
                    Cell::from(Line::from(Span::styled(
                        row.name.clone(),
                        Style::default()
                            .fg(get_drive_color(row.drive))
                            .add_modifier(Modifier::BOLD),
                    ))),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                ]);
            }

            // Get file-type icon from devicons (Nerd Font glyphs)
            let fi = devicons::icon_for_file(&row.name, &None);
            let icon_str = fi.icon.to_string();
            let icon_color = devicon_color(fi.color);

            // Drive column (colored letter)
            let drive_cell = Cell::from(Line::from(Span::styled(
                row.drive.to_string(),
                Style::default()
                    .fg(get_drive_color(row.drive))
                    .add_modifier(Modifier::BOLD),
            )));

            // Name column: icon + highlighted name
            let mut name_spans = vec![
                Span::styled(icon_str, Style::default().fg(icon_color)),
                Span::raw(" "),
            ];
            name_spans.extend(highlight_matches(
                &row.name,
                &search_term,
                Style::default().fg(Color::Cyan),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            let name_cell = Cell::from(Line::from(name_spans));

            // Size column
            let size_cell = Cell::from(Line::from(Span::styled(
                uffs_mft::format_bytes(row.size),
                Style::default().fg(Color::Yellow),
            )));

            // Modified column
            let modified_cell = Cell::from(Line::from(Span::styled(
                uffs_mft::format_timestamp(row.modified),
                Style::default().fg(Color::DarkGray),
            )));

            // Path column (highlighted, truncated)
            let path_display = truncate_path(&row.path, 60);
            let path_spans = highlight_matches(
                &path_display,
                &search_term,
                Style::default().fg(Color::DarkGray),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
            let path_cell = Cell::from(Line::from(path_spans));

            Row::new(vec![drive_cell, name_cell, size_cell, modified_cell, path_cell])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),    // Drive
            Constraint::Min(20),      // Name (flexible, takes remaining space)
            Constraint::Length(12),    // Size
            Constraint::Length(19),    // Modified
            Constraint::Length(62),    // Path
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(format!(
        " Results ({}) ",
        app.results.len()
    )))
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, chunks[2], &mut app.table_state.clone());

    // Help bar
    let help = Paragraph::new(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Green)),
        Span::raw(" Nav  "),
        Span::styled("PgUp/Dn", Style::default().fg(Color::Green)),
        Span::raw(" Page  "),
        Span::styled("Enter", Style::default().fg(Color::Green)),
        Span::raw(" Path  "),
        Span::styled("Tab", Style::default().fg(Color::Green)),
        Span::raw(" Sort  "),
        Span::styled("F2", Style::default().fg(Color::Green)),
        Span::raw(" Name-only  "),
        Span::styled("F3", Style::default().fg(Color::Green)),
        Span::raw(" Filter  "),
        Span::styled("Ctrl+R", Style::default().fg(Color::Green)),
        Span::raw(" Refresh  "),
        Span::styled("Ctrl+Q", Style::default().fg(Color::Green)),
        Span::raw(" Quit"),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Help "));
    frame.render_widget(help, chunks[3]);
}

/// Load a live NTFS drive — platform dispatch.
#[cfg(windows)]
fn load_live_drive_impl(
    drive_letter: char,
    no_cache: bool,
) -> anyhow::Result<(backend::DriveIndex, backend::LoadTiming)> {
    backend::load_live_drive(drive_letter, no_cache)
}

/// Load a live NTFS drive — not available on non-Windows.
#[cfg(not(windows))]
#[expect(
    clippy::single_call_fn,
    reason = "platform-specific stub; Windows version in backend::load_live_drive"
)]
fn load_live_drive_impl(
    drive_letter: char,
    _no_cache: bool,
) -> Result<(backend::DriveIndex, backend::LoadTiming)> {
    anyhow::bail!("Live drive loading requires Windows (drive {drive_letter}:)")
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

/// Split text into spans, highlighting case-insensitive matches of `needle`.
///
/// Non-matching parts use `normal_style`, matching parts use `highlight_style`.
fn highlight_matches(
    text: &str,
    needle: &str,
    normal_style: Style,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    if needle.is_empty() {
        return vec![Span::styled(text.to_owned(), normal_style)];
    }

    let lower = text.to_lowercase();
    let mut spans = Vec::new();
    let mut last_end = 0;

    for (start, matched) in lower.match_indices(needle) {
        if start > last_end {
            if let Some(before) = text.get(last_end..start) {
                spans.push(Span::styled(before.to_owned(), normal_style));
            }
        }
        let end = start + matched.len();
        if let Some(hit) = text.get(start..end) {
            spans.push(Span::styled(hit.to_owned(), highlight_style));
        }
        last_end = end;
    }

    if last_end < text.len() {
        if let Some(tail) = text.get(last_end..) {
            spans.push(Span::styled(tail.to_owned(), normal_style));
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_owned(), normal_style));
    }

    spans
}

/// Convert a devicons hex color string (e.g., `"#e37933"`) to a ratatui
/// `Color`.
///
/// Hex strings from devicons are always 7-byte ASCII (`#RRGGBB`), so
/// byte-level `.get()` slicing is safe.
#[expect(
    clippy::single_call_fn,
    reason = "standalone color-parsing helper; keeps rendering code readable"
)]
fn devicon_color(hex: &str) -> Color {
    if hex.len() == 7 && hex.starts_with('#') {
        if let (Some(rr), Some(gg), Some(bb)) = (hex.get(1..3), hex.get(3..5), hex.get(5..7)) {
            if let (Ok(red), Ok(green), Ok(blue)) = (
                u8::from_str_radix(rr, 16),
                u8::from_str_radix(gg, 16),
                u8::from_str_radix(bb, 16),
            ) {
                return Color::Rgb(red, green, blue);
            }
        }
    }
    Color::White
}

/// Format milliseconds compactly: `23 ms`, `535 ms`, `1.1  s`, `19.6  s`.
fn format_ms_compact(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        // Integer arithmetic: tenths of a second to avoid float_arithmetic lint
        let tenths = (ms + 50) / 100; // round to nearest tenth
        let whole = tenths / 10;
        let frac = tenths % 10;
        format!("{whole}.{frac}  s")
    }
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

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::is_exit_key;

    #[test]
    fn test_is_exit_key_accepts_ctrl_q() {
        assert!(is_exit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn test_is_exit_key_rejects_regular_input() {
        // Plain 'q' types the letter, doesn't exit
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        // Esc goes to textarea, doesn't exit
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )));
        // Ctrl+C goes to textarea, doesn't exit
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(!is_exit_key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
    }
}
