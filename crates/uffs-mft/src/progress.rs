// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Progress helpers for Windows-only `uffs-mft` commands.

#[cfg(windows)]
use indicatif::{ProgressBar, ProgressStyle};

/// Creates the standard spinner used by long-running `uffs-mft` commands.
#[cfg(windows)]
pub(crate) fn spinner(message: &str) -> ProgressBar {
    let progress_bar = ProgressBar::new_spinner();
    let style = ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());
    progress_bar.set_style(style);
    progress_bar.set_message(message.to_owned());
    progress_bar
}
