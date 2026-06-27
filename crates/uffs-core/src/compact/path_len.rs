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
/// `children` is the base CSR; `delta` is the overlay (Phase 4b) — together
/// they give the **current** child adjacency a directory rename walks to shift
/// its subtree, even for children created in the same batch. Caller falls back
/// to the full [`compute_path_lengths`] for cold loads and for batches large
/// enough that incremental loses (see the threshold in
/// `compact_loader/rebuild.rs`).
pub(crate) fn update_path_lengths_incremental(
    records: &mut [CompactRecord],
    names: &[u8],
    drive_letter: uffs_mft::platform::DriveLetter,
    children: &ChildrenIndex,
    delta: Option<&crate::compact::IndexDelta>,
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
            let shift = i64::from(new_pl) - i64::from(old_pl);
            if shift != 0 {
                shift_subtree_path_len(records, children, delta, change.idx, shift);
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

/// Add `shift` to every descendant of `root`'s `path_len` (a directory rename
/// moves each descendant's full path by the same amount).
///
/// Two passes so the read of the child adjacency (which validates against
/// `records`) never overlaps the write of `path_len`: pass 1 collects every
/// descendant over the base ∪ delta children (Phase 4b), pass 2 shifts each.
/// Pure arithmetic, no name/string walk.
fn shift_subtree_path_len(
    records: &mut [CompactRecord],
    children: &ChildrenIndex,
    delta: Option<&crate::compact::IndexDelta>,
    root: u32,
    shift: i64,
) {
    // Pass 1 — collect descendants (read-only over records + children + delta).
    let mut descendants: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    push_children(records, children, delta, root, &mut stack);
    while let Some(idx) = stack.pop() {
        descendants.push(idx);
        push_children(records, children, delta, idx, &mut stack);
    }
    // Pass 2 — shift each descendant's path_len (mutates records).
    for idx in descendants {
        if let Some(rec) = records.get_mut(idx as usize) {
            let shifted = i64::from(rec.path_len)
                .saturating_add(shift)
                .clamp(0, i64::from(u16::MAX));
            rec.path_len = u16::try_from(shifted).unwrap_or(u16::MAX);
        }
    }
}

/// Push the live children of `parent` (base ∪ delta, validated against
/// `records`) onto `stack`. The read-only adjacency primitive of the subtree
/// walk; mirrors [`crate::compact::DriveCompactIndex::for_each_child`].
fn push_children(
    records: &[CompactRecord],
    children: &ChildrenIndex,
    delta: Option<&crate::compact::IndexDelta>,
    parent: u32,
    stack: &mut Vec<u32>,
) {
    let base = children.get(parent as usize);
    let Some(overlay) = delta else {
        stack.extend_from_slice(base);
        return;
    };
    let is_valid = |child: u32| {
        records
            .get(child as usize)
            .is_some_and(|rec| rec.parent_idx == parent && rec.name_len != 0)
    };
    crate::compact::delta::merge_filter(base, overlay.child_postings(parent), is_valid, |child| {
        stack.push(child);
    });
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
