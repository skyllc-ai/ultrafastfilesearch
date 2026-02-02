//! UFFS Diagnostic Tools Library
//!
//! This crate provides diagnostic tools for MFT analysis. The library portion
//! exposes shared modules used by the diagnostic binaries.

#![allow(clippy::missing_docs_in_private_items)]

// Keep dependencies wired in for version-locking, even though the library
// portion does not use them directly (the binaries do).
use {anyhow as _, chrono as _, rayon as _, uffs_mft as _, uffs_polars as _};

/// Windows-only helpers for inspecting the full uffs-mft raw->fixup->parse
/// pipeline for a single FRS.
#[cfg(windows)]
pub mod uffs_mft_helpers_windows;
