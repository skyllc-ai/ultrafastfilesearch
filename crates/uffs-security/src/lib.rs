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
//! - [`log_dir`] — Shared per-platform native log-directory resolution for
//!   every UFFS binary (macOS `~/Library/Logs/uffs`, Windows
//!   `%LOCALAPPDATA%\uffs\logs`, Linux `$XDG_STATE_HOME/uffs/logs`)
//!
//! # Environment
//!
//! Env vars read by this crate (registry:
//! `docs/architecture/code-quality/build_codegen_policy.md` §5, playbook
//! §1049-1056):
//!
//! | Env var | Type | Default | Notes |
//! |---|---|---|---|
//! | `UFFS_DEV` | `bool` | `false` | Enables dev-mode keystore relaxation in [`keystore`] (no DPAPI binding; file-based key at `~/.local/share/uffs/key.bin` on Unix).  INTERNAL semver class. |
//! | `USERNAME` | `string` | (Windows: current user) | Read by [`fs::set_file_permissions_owner_only`] on Windows to derive the principal for the `icacls /grant` ACL.  STANDARD semver class. |
//! | `UFFS_LOG_DIR` | path | (native per-OS dir) | Read by [`log_dir`] to override the log directory for every UFFS binary.  STANDARD semver class. |
//! | `XDG_STATE_HOME` | path | `~/.local/state` | Read by [`log_dir`] on Linux for the native log location (absolute paths only, per XDG spec).  STANDARD semver class. |

// Platform-gated deps: used by sub-modules behind #[cfg] gates.
// Suppress unused-crate-dependencies lint for platforms where the
// usage is behind cfg and the lint can't see it.
use dirs_next as _;
#[cfg(target_os = "macos")]
use security_framework as _;

pub mod crypto;
pub mod fs;
pub mod keystore;
/// Shared per-platform log-directory resolution for all UFFS binaries.
pub mod log_dir;
pub mod runtime_dir;

/// In-process Authenticode (`WinVerifyTrust`) signature verification —
/// shared by the Access Broker and the self-updater. Windows-only.
#[cfg(windows)]
pub mod authenticode;

/// Windows named-pipe security helpers (DACL, SID resolution, pipe naming).
///
/// Only compiled on Windows.  See [`pipe`] module docs for the security
/// model rationale.
#[cfg(windows)]
pub mod pipe;
