// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Path-based file lookup for [`IndexManager`] (RPC `info`).
//!
//! The `info` RPC takes a fully-qualified path like
//! `C:\Windows\System32\notepad.exe` and returns the
//! corresponding record's MFT-derived metadata as
//! [`uffs_client::protocol::response::InfoResponse`].
//!
//! Lookup is `O(path_depth)`: we parse the drive prefix, then
//! walk the [`uffs_core::compact::DriveCompactIndex::children`]
//! adjacency list one segment at a time, matching each segment
//! case-insensitively.  This is asymptotically better than
//! [`crate::index::search`] which has to build a `DriveIndex`
//! and run the substring backend over every record — at the
//! cost of failing for partial paths and case-sensitive
//! filesystems (the latter not relevant for NTFS).
//!
//! The four functions in this module form one cohesive
//! pipeline:
//!
//! 1. [`IndexManager::info`] — the async public entry point.  Snapshots the
//!    registry, hands the snapshot to the synchronous tree-walk, and wraps the
//!    resulting `Option<Value>` in [`InfoResponse`].
//! 2. [`IndexManager::info_tree_lookup`] — the synchronous walker.  Drives the
//!    parse + segment-by-segment match.
//! 3. [`IndexManager::parse_drive_prefix`] — helper that splits `"C:\\foo"`
//!    into `(uffs_mft::platform::DriveLetter::C, "foo")`.
//! 4. [`IndexManager::build_info_json`] — turns a matching
//!    [`uffs_core::compact::CompactRecord`] into the JSON payload the response
//!    carries.
//!
//! [`InfoResponse`]: uffs_client::protocol::response::InfoResponse

use uffs_core::search::backend::DriveIndex;

use super::IndexManager;

impl IndexManager {
    /// Look up a file by path and return all available fields (D2.3.7).
    ///
    /// Walks the `children` index top-down in `O(path_depth)` instead of
    /// scanning all records with full path resolution.
    pub(crate) async fn info(
        &self,
        file_path: &str,
    ) -> uffs_client::protocol::response::InfoResponse {
        let snap = self.snapshot().await;

        let found_record = Self::info_tree_lookup(&snap, file_path);

        drop(snap);

        uffs_client::protocol::response::InfoResponse {
            found: found_record.is_some(),
            record: found_record,
        }
    }

    /// Fast tree-walk lookup: parse path → drive letter + segments, then
    /// walk `children` index matching each segment case-insensitively.
    fn info_tree_lookup(snap: &DriveIndex, file_path: &str) -> Option<serde_json::Value> {
        // Parse "C:\Windows\System32\notepad.exe" →
        // (uffs_mft::platform::DriveLetter::C, ["Windows", "System32",
        // "notepad.exe"])
        let normalized = file_path.replace('/', "\\");
        let (drive_letter, remainder) = Self::parse_drive_prefix(&normalized)?;

        let segments: Vec<&str> = remainder
            .split('\\')
            .filter(|seg| !seg.is_empty())
            .collect();
        if segments.is_empty() {
            return None;
        }

        // Find the matching drive.
        let drive = snap.drives.iter().find(|dr| dr.letter == drive_letter)?;

        // Find root entries (parent_idx == u32::MAX) as starting candidates.
        let mut candidates: Vec<u32> = Vec::new();
        for (idx, rec) in drive.records.iter().enumerate() {
            if rec.parent_idx == u32::MAX && rec.name_len > 0 {
                candidates.push(uffs_mft::len_to_u32(idx));
            }
        }

        // Walk segments top-down through the children index.
        for (seg_idx, &segment) in segments.iter().enumerate() {
            let seg_lower = segment.to_ascii_lowercase();
            let is_last = seg_idx == segments.len() - 1;

            let mut next_candidates: Vec<u32> = Vec::new();

            if seg_idx == 0 {
                // First segment: match against root entries.
                for &root_idx in &candidates {
                    if let Some(rec) = drive.records.get(uffs_mft::u32_as_usize(root_idx)) {
                        let name = rec.name(&drive.names);
                        if name.to_ascii_lowercase() == seg_lower {
                            if is_last {
                                let volume_prefix = format!("{}:\\", drive.letter);
                                let resolved = uffs_core::search::tree::resolve_path(
                                    drive,
                                    uffs_mft::u32_as_usize(root_idx),
                                    &volume_prefix,
                                    uffs_core::compact::MalformedRender::Lossy,
                                );
                                return Some(Self::build_info_json(drive, rec, &resolved));
                            }
                            // Collect children for next segment.
                            next_candidates.extend_from_slice(&drive.children_of(root_idx));
                        }
                    }
                }
            } else {
                // Subsequent segments: match against children of previous matches.
                for &child_idx in &candidates {
                    if let Some(rec) = drive.records.get(uffs_mft::u32_as_usize(child_idx)) {
                        let name = rec.name(&drive.names);
                        if name.to_ascii_lowercase() == seg_lower {
                            if is_last {
                                let volume_prefix = format!("{}:\\", drive.letter);
                                let resolved = uffs_core::search::tree::resolve_path(
                                    drive,
                                    uffs_mft::u32_as_usize(child_idx),
                                    &volume_prefix,
                                    uffs_core::compact::MalformedRender::Lossy,
                                );
                                return Some(Self::build_info_json(drive, rec, &resolved));
                            }
                            next_candidates.extend_from_slice(&drive.children_of(child_idx));
                        }
                    }
                }
            }

            if next_candidates.is_empty() {
                return None;
            }
            candidates = next_candidates;
        }

        None
    }

    /// Parse `C:\...` or `c:/...` into `(drive_letter, remainder)`.
    fn parse_drive_prefix(path: &str) -> Option<(uffs_mft::platform::DriveLetter, &str)> {
        let mut chars = path.chars();
        let letter_ch = chars.next()?;
        let letter = uffs_mft::platform::DriveLetter::parse(letter_ch).ok()?;
        if chars.next()? != ':' {
            return None;
        }
        // Skip optional separator after ':'
        let after_colon = path.get(2..)?;
        let remainder = after_colon
            .strip_prefix('\\')
            .or_else(|| after_colon.strip_prefix('/'))
            .unwrap_or(after_colon);
        Some((letter, remainder))
    }

    /// Build the JSON value for an info response record.
    fn build_info_json(
        drive: &uffs_core::compact::DriveCompactIndex,
        rec: &uffs_core::compact::CompactRecord,
        resolved_path: &str,
    ) -> serde_json::Value {
        let name = rec.name(&drive.names);
        serde_json::json!({
            "drive": drive.letter.to_string(),
            "path": resolved_path,
            "name": name,
            "size": rec.size,
            "allocated": rec.allocated,
            "treesize": rec.treesize,
            "tree_allocated": rec.tree_allocated,
            "created": rec.created,
            "modified": rec.modified,
            "accessed": rec.accessed,
            "flags": rec.flags,
            "is_directory": rec.is_directory(),
            "descendants": rec.descendants,
            "parent_idx": rec.parent_idx,
            "extension_id": rec.extension_id,
        })
    }
}
