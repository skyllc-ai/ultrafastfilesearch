// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Helpers for selecting the preferred primary file name during parsing.

use crate::frs::ParentFrs;
use crate::ntfs::NameInfo;

/// Tracks the best primary name during parsing.
/// Win32 (1) and Win32+DOS (3) are preferred over POSIX (0).
pub(super) struct PrimaryNameTracker {
    /// Primary filename.
    pub(super) name: String,
    /// Parent FRS of the primary name.
    pub(super) parent_frs: ParentFrs,
    /// Namespace of the primary name (255 = invalid/unset).
    pub(super) namespace: u8,
    /// `$FILE_NAME` creation timestamp.
    pub(super) fn_created: i64,
    /// `$FILE_NAME` modification timestamp.
    pub(super) fn_modified: i64,
    /// `$FILE_NAME` access timestamp.
    pub(super) fn_accessed: i64,
    /// `$FILE_NAME` MFT change timestamp.
    pub(super) fn_mft_changed: i64,
}

impl PrimaryNameTracker {
    /// Sentinel value indicating no namespace has been set yet.
    const INVALID_NAMESPACE: u8 = 255;

    /// Updates the primary name if the new name is better.
    pub(super) fn update(&mut self, name_info: &NameInfo) {
        let dominated = self.namespace == Self::INVALID_NAMESPACE;
        let is_better =
            matches!(name_info.namespace, 1 | 3) || (name_info.namespace == 0 && dominated);
        if is_better || dominated {
            self.name = name_info.name.clone();
            self.parent_frs = name_info.parent_frs;
            self.namespace = name_info.namespace;
            self.fn_created = name_info.fn_created;
            self.fn_modified = name_info.fn_modified;
            self.fn_accessed = name_info.fn_accessed;
            self.fn_mft_changed = name_info.fn_mft_changed;
        }
    }
}

impl Default for PrimaryNameTracker {
    fn default() -> Self {
        Self {
            name: String::new(),
            parent_frs: ParentFrs::ZERO,
            namespace: Self::INVALID_NAMESPACE,
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
        }
    }
}
