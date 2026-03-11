//! Progress helpers for Windows-only `uffs_mft` commands.

#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};

/// Creates the standard spinner used by long-running `uffs_mft` commands.
#[cfg(windows)]
pub fn spinner(message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new_spinner();
    let style = ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());
    progress_bar.set_style(style);
    progress_bar.set_message(message.to_owned());
    progress_bar
}
