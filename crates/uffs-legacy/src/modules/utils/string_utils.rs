//! String utilities for cross-platform string handling.
//!
//! Provides functions for converting between Rust strings and
//! platform-specific string formats (e.g., Windows wide strings).

// Infrastructure utilities - defined for Windows API interop
#![allow(dead_code)]

use std::ffi::OsStr;

/// Convert Rust string to a wide string (Vec<u16>) with null termination
/// (Windows only).
#[cfg(windows)]
pub(crate) fn to_wide_string_with_null(s: &OsStr) -> Vec<u16> {
    use std::os::windows::prelude::OsStrExt;
    s.encode_wide().chain(Some(0)).collect()
}

/// Stub for non-Windows platforms - returns empty Vec.
#[cfg(not(windows))]
pub(crate) fn to_wide_string_with_null(_s: &OsStr) -> Vec<u16> {
    Vec::new()
}
