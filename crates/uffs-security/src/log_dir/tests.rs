// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for log-directory resolution.
//!
//! These deliberately avoid mutating the process-global environment
//! (`std::env::set_var` is `unsafe` under the Rust 2024 edition and
//! racy across the test thread pool).  The env-driven branch is
//! exercised by passing values directly into the pure helpers.

use super::*;

#[test]
fn native_log_dir_is_absolute_or_relative_fallback() {
    // On any machine with a home dir the native path is absolute and
    // ends in the platform-appropriate suffix.  Without a home dir it
    // falls back to the relative `logs`.  Either way it ends with a
    // recognizable UFFS log segment.
    let dir = native_log_dir();
    let ends_ok = dir.ends_with("logs") || dir.ends_with("uffs");
    assert!(ends_ok, "unexpected native log dir: {}", dir.display());
}

#[test]
fn log_dir_env_const_is_stable() {
    // The override variable name is part of the cross-binary contract
    // (and is mirrored by the products-repo TUI); pin it.
    assert_eq!(LOG_DIR_ENV, "UFFS_LOG_DIR");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_uses_library_logs() {
    let dir = native_log_dir();
    // When a home dir exists, the path is `<home>/Library/Logs/uffs`.
    if let Some(home) = dirs_next::home_dir() {
        assert_eq!(dir, home.join("Library").join("Logs").join("uffs"));
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_uses_local_appdata_logs() {
    let dir = native_log_dir();
    if let Some(local) = dirs_next::data_local_dir() {
        assert_eq!(dir, local.join("uffs").join("logs"));
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod linux {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn absolute_xdg_state_home_is_used_verbatim() {
        let raw = Some(std::ffi::OsString::from("/custom/state"));
        let home = Some(PathBuf::from("/home/user"));
        assert_eq!(xdg_state_home(raw, home), Path::new("/custom/state"));
    }

    #[test]
    fn relative_xdg_state_home_is_ignored_per_spec() {
        // A relative XDG_STATE_HOME must be ignored; fall back to
        // `~/.local/state`.
        let raw = Some(std::ffi::OsString::from("relative/path"));
        let home = Some(PathBuf::from("/home/user"));
        assert_eq!(
            xdg_state_home(raw, home),
            Path::new("/home/user/.local/state")
        );
    }

    #[test]
    fn unset_xdg_state_home_falls_back_to_local_state() {
        let home = Some(PathBuf::from("/home/user"));
        assert_eq!(
            xdg_state_home(None, home),
            Path::new("/home/user/.local/state")
        );
    }

    #[test]
    fn no_home_and_no_xdg_yields_current_dir() {
        assert_eq!(xdg_state_home(None, None), Path::new("."));
    }

    #[test]
    fn linux_native_dir_ends_with_uffs_logs() {
        // Full path assembled by the public helper ends in uffs/logs
        // regardless of which fallback fired.
        let dir = native_log_dir();
        assert!(
            dir.ends_with(Path::new("uffs/logs")),
            "unexpected linux log dir: {}",
            dir.display()
        );
    }
}
