//! Progress helpers for Windows-only `uffs_mft` commands.

#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};

/// Creates the standard spinner used by long-running `uffs_mft` commands.
#[cfg(windows)]
#[expect(
    clippy::expect_used,
    reason = "progress template is a validated constant"
)]
pub fn spinner(message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new_spinner();
    progress_bar.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    progress_bar.set_message(message.to_owned());
    progress_bar
}
