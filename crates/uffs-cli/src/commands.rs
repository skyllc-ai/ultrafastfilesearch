//! CLI command implementations.
//!
//! This module provides the public command surface for the UFFS CLI and shared
//! helpers used by the split command modules.

#[cfg(windows)]
use indicatif::MultiProgress;
#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};

/// Index subcommand implementation.
mod index;
/// Info subcommand implementation.
mod info;
/// Output helpers for search results.
mod output;
/// Raw MFT/data loading helpers for search.
mod raw_io;
/// Search command implementation.
mod search;
/// Stats subcommand implementation.
mod stats;

pub use self::index::index;
pub use self::info::info;
pub use self::search::search;
pub use self::stats::stats;

/// Check if progress bars are disabled via `UFFS_NO_PROGRESS=1` environment
/// variable.
#[cfg(windows)]
#[inline]
fn is_progress_disabled() -> bool {
    std::env::var("UFFS_NO_PROGRESS")
        .map(|val| val == "1" || val.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Create a multi-progress container for multiple drives.
/// Returns `None` if progress is disabled via `UFFS_NO_PROGRESS=1`.
#[cfg(windows)]
fn create_multi_progress() -> Option<MultiProgress> {
    if is_progress_disabled() {
        None
    } else {
        Some(MultiProgress::new())
    }
}

/// Create a progress bar for a specific drive.
#[cfg(windows)]
fn add_drive_progress(mp: &MultiProgress, drive: char) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(0));
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {bytes:>7}/{total_bytes:7} {msg}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-"),
    );
    pb.set_message(format!("Reading {drive}:\\$MFT"));
    pb
}

/// Format a number with comma separators.
fn format_number(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, ch) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Format file size in human-readable format.
#[expect(
    clippy::cast_precision_loss,
    reason = "u64 to f64 is acceptable for human-readable size display"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "division for human-readable size formatting"
)]
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
