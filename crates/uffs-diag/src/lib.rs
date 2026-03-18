//! UFFS Diagnostic Tools Library
//!
//! This crate provides diagnostic tools for MFT analysis. The library portion
//! exposes shared modules used by the diagnostic binaries.

// Keep dependencies wired in for version-locking, even though the library
// portion does not use them directly (the binaries do).
use anyhow as _;
use chrono as _;
use rayon as _;
use uffs_mft as _;
use uffs_polars as _;

/// Parity comparison helpers for validating scan output between reference and
/// Rust implementations.
pub mod parity;

/// Windows-only helpers for inspecting the full uffs-mft raw->fixup->parse
/// pipeline for a single FRS.
#[cfg(windows)]
pub mod uffs_mft_helpers_windows;
