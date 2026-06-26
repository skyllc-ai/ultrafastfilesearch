// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Per-record `path_len` computation: the cold-load top-down BFS plus the
//! Phase-1 incremental update used by the USN apply path
//! (incremental-index-maintenance §5.5).

use crate::compact::{ChildrenIndex, CompactRecord};

/// Compute `path_len` (in **characters**, not bytes) for every record
/// via top-down BFS.
///
/// Root entries (`parent_idx == u32::MAX`) get
/// `path_len = 2 + 1 + name_chars` (e.g. `"C:\" + name`), and children
/// accumulate `parent.path_len + 1 (separator) + name_chars`.
/// Saturates at `u16::MAX` (65 535) for extremely deep paths.
///
/// Character counting matches `str::chars().count()` so the precomputed
/// value agrees with the display-row path-length filter.
pub(crate) fn compute_path_lengths(
    records: &mut [CompactRecord],
    names: &[u8],
    drive_letter: uffs_mft::platform::DriveLetter,
) {
    // Drive prefix in characters: the letter (1 char) + colon (1 char) = 2.
    // `DriveLetter` is ASCII A–Z by construction (validated in
    // `DriveLetter::parse`), so the previous runtime `debug_assert!`
    // is now a tautology and was removed.  The arithmetic only cares
    // about "1 letter char + 1 colon".
    let _: uffs_mft::platform::DriveLetter = drive_letter;
    let drive_prefix_chars: u32 = 1 /* letter */ + 1 /* ':' */;

    // Build forward adjacency list (parent → children) for top-down BFS.
    let record_count = records.len();
    let mut children_of: Vec<Vec<u32>> = vec![Vec::new(); record_count];
    let mut roots: Vec<u32> = Vec::new();

    for (idx, rec) in records.iter().enumerate() {
        let pi = rec.parent_idx;
        if pi == u32::MAX {
            roots.push(uffs_mft::len_to_u32(idx));
        } else if let Some(siblings) = children_of.get_mut(pi as usize) {
            siblings.push(uffs_mft::len_to_u32(idx));
        }
    }

    // BFS from roots.
    let mut queue = alloc::collections::VecDeque::with_capacity(roots.len());
    for &root in &roots {
        let Some(rec) = records.get(root as usize) else {
            continue;
        };
        let name_chars = name_char_count(rec, names);
        let pl = if name_chars == 0 {
            // Drive root directory: "C:\"
            drive_prefix_chars + 1
        } else {
            // Top-level file/dir: "C:\<name>"
            drive_prefix_chars + 1 + name_chars
        };
        if let Some(slot) = records.get_mut(root as usize) {
            slot.path_len = uffs_mft::len_to_u16(pl as usize);
        }
        queue.push_back(root);
    }

    while let Some(idx) = queue.pop_front() {
        let parent_pl = records
            .get(idx as usize)
            .map_or(0, |rec| u32::from(rec.path_len));
        let children: Vec<u32> = children_of
            .get(idx as usize)
            .map_or_else(Vec::new, Clone::clone);
        for &child in &children {
            let child_chars = records
                .get(child as usize)
                .map_or(0, |rec| name_char_count(rec, names));
            // path = parent_path + "\" + name
            let pl = parent_pl.saturating_add(1).saturating_add(child_chars);
            if let Some(slot) = records.get_mut(child as usize) {
                slot.path_len = uffs_mft::len_to_u16(pl as usize);
            }
            queue.push_back(child);
        }
    }
}

/// A record whose `path_len` a USN apply must refresh, plus whether the change
/// can shift its whole subtree.
///
/// Phase 1 of incremental-index-maintenance (design doc §5.5): instead of the
/// O(total) [`compute_path_lengths`] BFS every apply, refresh only the records
/// a batch touched.  A **directory rename** moves every descendant's path by a
/// constant Δ, so `subtree` requests the descendant walk; creates and file
/// renames are a single O(1) refresh.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathChange {
    /// Compact index of the created / renamed record.
    pub idx: u32,
    /// `true` for a directory rename (propagate Δ to descendants); `false` for
    /// creates and file renames (refresh this record only).
    pub subtree: bool,
}

/// Refresh `path_len` for only the records a USN batch touched, instead of the
/// O(total-records) [`compute_path_lengths`] BFS — the Phase-1 lever of
/// incremental-index-maintenance (design doc §5.5).
///
/// `children` must be the **freshly rebuilt** CSR so a directory rename can
/// walk its subtree.  Caller falls back to the full [`compute_path_lengths`]
/// for cold loads and for batches large enough that incremental loses
/// (see the threshold in `compact_loader/rebuild.rs`).
pub(crate) fn update_path_lengths_incremental(
    records: &mut [CompactRecord],
    names: &[u8],
    drive_letter: uffs_mft::platform::DriveLetter,
    children: &ChildrenIndex,
    changed: &[PathChange],
) {
    // `DriveLetter` is ASCII A–Z by construction, so the drive prefix is always
    // "X:" = 2 chars (matches `compute_path_lengths`).
    let _: uffs_mft::platform::DriveLetter = drive_letter;
    let drive_prefix_chars: u32 = 1 /* letter */ + 1 /* ':' */;

    for change in changed {
        let idx = change.idx as usize;
        let Some(rec) = records.get(idx) else {
            continue;
        };
        // Skip a slot tombstoned within the same batch (create then delete):
        // `apply_delete` set name_len=0 + parent=MAX.
        if rec.name_len == 0 && rec.parent_idx == u32::MAX {
            continue;
        }
        let old_pl = u32::from(rec.path_len);
        let new_pl = path_len_from_parent(records, names, drive_prefix_chars, change.idx);
        if let Some(slot) = records.get_mut(idx) {
            slot.path_len = uffs_mft::len_to_u16(new_pl as usize);
        }
        if change.subtree {
            let delta = i64::from(new_pl) - i64::from(old_pl);
            if delta != 0 {
                shift_subtree_path_len(records, children, change.idx, delta);
            }
        }
    }
}

/// `path_len` for `idx` from its (current) parent's `path_len` + own name —
/// the per-node arithmetic of [`compute_path_lengths`]'s BFS, in isolation.
fn path_len_from_parent(
    records: &[CompactRecord],
    names: &[u8],
    drive_prefix_chars: u32,
    idx: u32,
) -> u32 {
    let Some(rec) = records.get(idx as usize) else {
        return 0;
    };
    let name_chars = name_char_count(rec, names);
    if rec.parent_idx == u32::MAX {
        // Root level: "C:\" (no name) or "C:\<name>".
        if name_chars == 0 {
            drive_prefix_chars.saturating_add(1)
        } else {
            drive_prefix_chars
                .saturating_add(1)
                .saturating_add(name_chars)
        }
    } else {
        let parent_pl = records
            .get(rec.parent_idx as usize)
            .map_or(0, |parent| u32::from(parent.path_len));
        parent_pl.saturating_add(1).saturating_add(name_chars)
    }
}

/// Add `delta` to every descendant of `root`'s `path_len` (a directory rename
/// shifts each descendant's full path by the same amount).  Iterative DFS over
/// the children CSR — pure arithmetic, no name/string walk.
fn shift_subtree_path_len(
    records: &mut [CompactRecord],
    children: &ChildrenIndex,
    root: u32,
    delta: i64,
) {
    let mut stack: Vec<u32> = children.get(root as usize).to_vec();
    while let Some(idx) = stack.pop() {
        if let Some(rec) = records.get_mut(idx as usize) {
            let shifted = i64::from(rec.path_len)
                .saturating_add(delta)
                .clamp(0, i64::from(u16::MAX));
            rec.path_len = u16::try_from(shifted).unwrap_or(u16::MAX);
        }
        stack.extend_from_slice(children.get(idx as usize));
    }
}

/// Count the number of Unicode characters in a record's filename.
///
/// Falls back to `name_len` (byte count) if the name slice is not valid
/// UTF-8 — this is correct for ASCII names and a safe upper bound
/// otherwise.
fn name_char_count(rec: &CompactRecord, names: &[u8]) -> u32 {
    let start = rec.name_offset as usize;
    let end = start + rec.name_len as usize;
    names
        .get(start..end)
        .and_then(|slice| core::str::from_utf8(slice).ok())
        .map_or_else(
            || u32::from(rec.name_len),
            |name| uffs_mft::len_to_u32(name.chars().count()),
        )
}
