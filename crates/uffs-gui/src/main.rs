//! UFFS (Ultra Fast File Search) GUI
//!
//! Native graphical user interface for file search.
//!
//! This is a placeholder for future GUI implementation.
//! Planned technologies: egui or Tauri
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for info-level terminal output:
//! ```bash
//! uffs_gui -v
//! ```
//!
//! For finer control, use environment variables:
//! - `RUST_LOG`: Terminal log level (default: `error`, or `info` with `-v`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/uffs/logs`)

// Suppress lints for GUI binary - these are intentional for GUI apps
#![allow(clippy::print_stdout)]
#![allow(clippy::print_stderr)]
#![allow(clippy::use_debug)]
#![allow(clippy::single_call_fn)]
#![allow(clippy::missing_docs_in_private_items)]
#![allow(unused_crate_dependencies)]

use std::io;
use std::path::PathBuf;

use clap::Parser;
use tracing::info;
use tracing_appender::non_blocking::NonBlocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::{EnvFilter, Layer};

/// UFFS (Ultra Fast File Search) GUI
#[derive(Parser)]
#[command(name = "uffs_gui")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

/// Initialize logging with terminal + file support.
///
/// If `verbose` is true and `RUST_LOG` is not set, uses `info` level for terminal.
/// Otherwise, terminal logging is controlled by `RUST_LOG` (default: `error`).
/// File logging is controlled by `RUST_LOG_FILE` (default: `info`).
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
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "uffs_gui_log_");
    let (non_blocking, guard): (NonBlocking, _) = NonBlocking::new(file_appender);

    // Terminal filter: -v sets info if RUST_LOG not explicitly set
    let terminal_default = if verbose { "info" } else { "error" };
    let terminal_filter =
        EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| terminal_default.to_owned()));

    // File filter (default: info)
    let file_filter =
        EnvFilter::new(std::env::var("RUST_LOG_FILE").unwrap_or_else(|_| "info".to_owned()));

    // Timer format
    let timer = UtcTime::rfc_3339();

    // Terminal layer (to stderr, with ANSI colors)
    let terminal_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_timer(timer.clone())
        .with_ansi(true)
        .with_filter(terminal_filter);

    // File layer (no ANSI colors)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_timer(timer)
        .with_ansi(false)
        .with_filter(file_filter);

    // Combine layers
    let subscriber = Registry::default().with(terminal_layer).with(file_layer);

    #[allow(clippy::expect_used)]
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global tracing subscriber");

    guard
}

fn main() {
    // Check for -v/--verbose flag early
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");

    // Initialize logging with terminal + file support
    let _guard = init_logging(verbose);

    let _cli = Cli::parse();

    info!("UFFS GUI starting (placeholder)");

    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║       UFFS (Ultra Fast File Search) GUI - Coming Soon!       ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║                                                              ║");
    eprintln!("║  The GUI is not yet implemented.                             ║");
    eprintln!("║                                                              ║");
    eprintln!("║  In the meantime, please use:                                ║");
    eprintln!("║    • uffs      - Command-line interface                      ║");
    eprintln!("║    • uffs_tui  - Terminal user interface                     ║");
    eprintln!("║                                                              ║");
    eprintln!("║  Planned features:                                           ║");
    eprintln!("║    • Native Windows/macOS/Linux GUI                          ║");
    eprintln!("║    • Real-time search with instant results                   ║");
    eprintln!("║    • File preview and quick actions                          ║");
    eprintln!("║    • Customizable themes                                     ║");
    eprintln!("║    • System tray integration                                 ║");
    eprintln!("║                                                              ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");

    std::process::exit(1);
}
