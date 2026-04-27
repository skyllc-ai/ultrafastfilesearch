// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Security primitives for UFFS.
//!
//! This crate provides encryption, key management, and secure filesystem
//! operations. It has **no dependency** on MFT, search, or UI crates.
//!
//! # Modules
//!
//! - [`crypto`] — AES-256-GCM authenticated encryption (Phase S2)
//! - [`keystore`] — Platform-native key storage: DPAPI / Keychain / Secret
//!   Service (Phase S2)
//! - [`fs`] — Secure file operations: atomic write, secure delete, permissions,
//!   file locking
//! - [`runtime_dir`] — Daemon-private runtime tempfile lifecycle (Phase 2b
//!   memory tiering): owner-only file creation, orphan-pid sweep, read-only
//!   mmap behind a typed soundness wrapper

// Platform-gated deps: used by sub-modules behind #[cfg] gates.
// Suppress unused-crate-dependencies lint for platforms where the
// usage is behind cfg and the lint can't see it.
use dirs_next as _;
#[cfg(target_os = "macos")]
use security_framework as _;

pub mod crypto;
pub mod fs;
pub mod keystore;
pub mod runtime_dir;

/// Windows named-pipe security helpers (DACL, SID resolution, pipe naming).
///
/// Only compiled on Windows.  See [`pipe`] module docs for the security
/// model rationale.
#[cfg(windows)]
pub mod pipe;
