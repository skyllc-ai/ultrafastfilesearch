// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! NTFS `FILE_ATTRIBUTE_*` bit constants used by the formatter's
//! flag-column dispatch.
//!
//! These mirror [`winnt::FILE_ATTRIBUTE_*`] and the legacy baseline
//! the parity tests pin, so the values are stable and safe to hard-code.
//! Phase 3 (PR #221) demoted the whole module to `pub(crate)` — workspace
//! grep showed zero external uses; the doc comment previously claimed
//! `uffs-core` / `uffs-client` consumed these constants, but the actual
//! consumers maintain their own copies (`uffs_core::compact::flags::*`,
//! etc.).  Re-promote if a cross-crate consumer materializes.
//!
//! [`winnt::FILE_ATTRIBUTE_*`]: https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants

/// Read-only attribute.
pub(crate) const READONLY: u32 = 0x0001;
/// Hidden attribute.
pub(crate) const HIDDEN: u32 = 0x0002;
/// System attribute.
pub(crate) const SYSTEM: u32 = 0x0004;
/// Directory attribute.
pub(crate) const DIRECTORY: u32 = 0x0010;
/// Archive attribute.
pub(crate) const ARCHIVE: u32 = 0x0020;
/// Temporary attribute.
pub(crate) const TEMPORARY: u32 = 0x0100;
/// Sparse-file attribute.
pub(crate) const SPARSE: u32 = 0x0200;
/// Reparse-point attribute.
pub(crate) const REPARSE: u32 = 0x0400;
/// Compressed attribute.
pub(crate) const COMPRESSED: u32 = 0x0800;
/// Offline attribute.
pub(crate) const OFFLINE: u32 = 0x1000;
/// Not-content-indexed attribute.
pub(crate) const NOT_INDEXED: u32 = 0x2000;
/// Encrypted attribute.
pub(crate) const ENCRYPTED: u32 = 0x4000;
/// Integrity-stream attribute.
pub(crate) const INTEGRITY: u32 = 0x8000;
/// Virtual attribute.
pub(crate) const VIRTUAL: u32 = 0x0001_0000;
/// No-scrub-data attribute.
pub(crate) const NO_SCRUB: u32 = 0x0002_0000;
/// Recall-on-open attribute.
pub(crate) const RECALL_ON_OPEN: u32 = 0x0004_0000;
/// Pinned attribute.
pub(crate) const PINNED: u32 = 0x0008_0000;
/// Unpinned attribute.
pub(crate) const UNPINNED: u32 = 0x0010_0000;
/// Recall-on-data-access attribute.
pub(crate) const RECALL_ON_DATA: u32 = 0x0040_0000;

/// Parity-compat mask — the 15 attribute bits the legacy baseline
/// tracks.
///
/// Matches `uffs_mft::StandardInfo::parity_attributes()` and the
/// `ParityAttributes` output column.  Excludes `TEMPORARY` (0x0100)
/// and `VIRTUAL` (`0x0001_0000`) which are not part of the parity
/// contract.
pub(crate) const PARITY_MASK: u32 = READONLY
    | HIDDEN
    | SYSTEM
    | DIRECTORY
    | ARCHIVE
    | SPARSE
    | REPARSE
    | COMPRESSED
    | OFFLINE
    | NOT_INDEXED
    | ENCRYPTED
    | INTEGRITY
    | NO_SCRUB
    | PINNED
    | UNPINNED;
