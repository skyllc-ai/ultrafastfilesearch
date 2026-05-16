// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Path resolution: `PathResolver` and `PathCache` for materializing Windows
//! paths.
//!
//! Extracted from `paths.rs` to keep it under the 800 LOC threshold.

use super::{FileRecord, MftIndex, NO_ENTRY};
use crate::frs::{Frs, ParentFrs};

// ============================================================================
// PathResolver - Ultra-fast path validity and on-demand materialization
// ============================================================================

/// System metafiles are FRS 0-15 (except root at FRS 5).
/// These are filtered out by default from regular path results.
const SYSTEM_METAFILE_MAX_FRS: u64 = 15;

/// State values for path resolution.
mod path_state {
    /// Record has not been visited yet.
    pub(super) const UNSEEN: u8 = 0;
    /// Record is currently being visited (cycle detection).
    pub(super) const VISITING: u8 = 1;
    /// Record has a valid path to root.
    pub(super) const VALID: u8 = 2;
    /// Record is invalid (system metafile, cycle, or descendant of invalid).
    pub(super) const INVALID: u8 = 3;
}

/// Ultra-fast path resolver using dense arrays instead of `HashMap`.
///
/// Key optimizations:
/// 1. Dense `Vec<u8>` state array - O(1) validity check, no hashing
/// 2. BFS illegal propagation - marks descendants in one pass
/// 3. On-demand path materialization - no string allocation until needed
/// 4. `SmallVec` for chain - avoids heap allocation for typical depths
/// 5. Two-pass string building - compute length first, single allocation
#[derive(Debug)]
pub struct PathResolver {
    /// State for each record index (UNSEEN, VISITING, VALID, INVALID).
    state: Vec<u8>,
    /// Volume letter for path prefix.
    volume: crate::platform::DriveLetter,
    /// Count of valid records.
    valid_count: u32,
    /// Count of invalid records.
    invalid_count: u32,
}

impl PathResolver {
    /// Build the path resolver for all records in the index.
    #[must_use]
    pub fn build(index: &MftIndex, include_system_metafiles: bool) -> Self {
        let n = index.records.len();
        let mut resolver = Self {
            state: vec![path_state::UNSEEN; n],
            volume: index.volume,
            valid_count: 0,
            invalid_count: 0,
        };

        if !include_system_metafiles {
            resolver.mark_system_metafiles_invalid(index);
        }
        resolver.propagate_invalid_to_descendants(index);
        resolver.validate_remaining(index);
        resolver
    }

    /// Check if a record at the given index is valid.
    #[inline]
    #[must_use]
    pub fn is_valid_idx(&self, idx: usize) -> bool {
        self.state.get(idx).copied() == Some(path_state::VALID)
    }

    /// Check if a record with the given FRS is valid.
    #[must_use]
    pub fn is_valid(&self, index: &MftIndex, frs: Frs) -> bool {
        index
            .frs_to_idx_opt(frs)
            .is_some_and(|idx| self.is_valid_idx(idx))
    }

    /// Get the number of valid records.
    #[must_use]
    pub const fn valid_count(&self) -> u32 {
        self.valid_count
    }

    /// Get the number of invalid records.
    #[must_use]
    pub const fn invalid_count(&self) -> u32 {
        self.invalid_count
    }

    /// Pre-compute directory paths using the correct `materialize_path`
    /// algorithm (follows `first_name.parent_frs`).
    ///
    /// Returns a dense `Vec<String>` indexed by record index.  Only valid
    /// directory records get a non-empty path; everything else is empty.
    ///
    /// These cached paths are used by `materialize_path_cached` to short-
    /// circuit the parent-chain walk when a directory ancestor is already
    /// resolved.
    #[must_use]
    pub(crate) fn pre_cache_directory_paths(&self, index: &MftIndex) -> Vec<String> {
        let n = index.records.len();
        let mut cache: Vec<String> = vec![String::new(); n];
        for (idx, slot) in cache.iter_mut().enumerate().take(n) {
            if self.is_valid_idx(idx)
                && index.records.get(idx).is_some_and(FileRecord::is_directory)
            {
                *slot = self.materialize_path(index, idx);
            }
        }
        cache
    }

    /// Materialize a path directly into a caller-owned buffer.
    ///
    /// Same logic as `materialize_path_cached` but appends to `out` instead
    /// of returning a new `String`.  Eliminates one heap allocation per
    /// record in the streaming output hot path.
    pub fn materialize_path_into(
        &self,
        index: &MftIndex,
        idx: usize,
        dir_cache: &[String],
        out: &mut String,
    ) {
        let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
        let mut current_idx = idx;

        loop {
            if let Some(cached) = dir_cache.get(current_idx)
                && !cached.is_empty()
            {
                out.push_str(cached);
                for &ci in chain.iter().rev() {
                    if let Some(rec) = index.records.get(ci) {
                        let name = index.record_name(rec);
                        if !name.is_empty() && name != "." {
                            if !out.ends_with('\\') {
                                out.push('\\');
                            }
                            out.push_str(name);
                        }
                    }
                }
                return;
            }

            let Some(record) = index.records.get(current_idx) else {
                break;
            };
            chain.push(current_idx);

            let parent_frs = record.first_name.parent_frs;
            if parent_frs.is_root()
                || parent_frs.as_frs() == record.frs
                || parent_frs == ParentFrs::new(u64::from(NO_ENTRY))
            {
                break;
            }
            let Some(parent_idx) = index.frs_to_idx_opt(parent_frs.as_frs()) else {
                break;
            };
            current_idx = parent_idx;
        }

        // No cache hit — build from scratch.
        out.push(self.volume.as_char());
        out.push(':');
        for &chain_idx in chain.iter().rev() {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    out.push('\\');
                    out.push_str(name);
                }
            }
        }
        if out.len() == 2 && out.as_bytes().last() == Some(&b':') {
            out.push('\\');
        }
    }

    /// Materialize a path using a directory cache for fast parent lookups.
    ///
    /// Identical to `materialize_path` but checks `dir_cache` when walking
    /// the parent chain.  If a cached directory path is found, the walk
    /// stops early and appends remaining components to the cached prefix.
    #[must_use]
    pub fn materialize_path_cached(
        &self,
        index: &MftIndex,
        idx: usize,
        dir_cache: &[String],
    ) -> String {
        let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
        let mut current_idx = idx;

        // Walk up parent chain, checking cache at each step.
        loop {
            // Check if this ancestor is already cached.
            if let Some(cached) = dir_cache.get(current_idx)
                && !cached.is_empty()
            {
                // Build path: cached prefix + remaining components
                if chain.is_empty() {
                    return cached.clone();
                }
                let mut total_len = cached.len();
                for &ci in &chain {
                    if let Some(rec) = index.records.get(ci) {
                        let name = index.record_name(rec);
                        if !name.is_empty() && name != "." {
                            total_len += 1 + name.len();
                        }
                    }
                }
                let mut path = String::with_capacity(total_len);
                path.push_str(cached);
                for &ci in chain.iter().rev() {
                    if let Some(rec) = index.records.get(ci) {
                        let name = index.record_name(rec);
                        if !name.is_empty() && name != "." {
                            if !path.ends_with('\\') {
                                path.push('\\');
                            }
                            path.push_str(name);
                        }
                    }
                }
                return path;
            }

            let Some(record) = index.records.get(current_idx) else {
                break;
            };
            chain.push(current_idx);

            let parent_frs = record.first_name.parent_frs;
            if parent_frs.is_root()
                || parent_frs.as_frs() == record.frs
                || parent_frs == ParentFrs::new(u64::from(NO_ENTRY))
            {
                break;
            }
            let Some(parent_idx) = index.frs_to_idx_opt(parent_frs.as_frs()) else {
                break;
            };
            current_idx = parent_idx;
        }

        // No cache hit — build from scratch (same as materialize_path).
        let mut total_len = 2;
        for &chain_idx in &chain {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    total_len += 1 + name.len();
                }
            }
        }
        let mut path = String::with_capacity(total_len);
        path.push(self.volume.as_char());
        path.push(':');
        for &chain_idx in chain.iter().rev() {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    path.push('\\');
                    path.push_str(name);
                }
            }
        }
        if path.len() == 2 && path.as_bytes().last() == Some(&b':') {
            path.push('\\');
        }
        path
    }

    /// Materialize the full path for a record (on-demand).
    #[must_use]
    // Loop has 4 distinct break conditions: record not found, reached root,
    // self-reference, parent not in index. Cannot be simplified to while_let.
    #[expect(
        clippy::while_let_loop,
        reason = "explicit loop with break is clearer here"
    )]
    pub fn materialize_path(&self, index: &MftIndex, idx: usize) -> String {
        let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
        let mut current_idx = idx;

        // Walk up parent chain
        loop {
            let Some(record) = index.records.get(current_idx) else {
                break;
            };
            chain.push(current_idx);

            let parent_frs = record.first_name.parent_frs;
            if parent_frs.is_root()
                || parent_frs.as_frs() == record.frs
                || parent_frs == ParentFrs::new(u64::from(NO_ENTRY))
            {
                break;
            }
            let Some(parent_idx) = index.frs_to_idx_opt(parent_frs.as_frs()) else {
                break;
            };
            current_idx = parent_idx;
        }

        // Compute total length
        let mut total_len = 2; // "v:"
        for &chain_idx in &chain {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    total_len += 1 + name.len();
                }
            }
        }

        // Build path with single allocation
        let mut path = String::with_capacity(total_len);
        path.push(self.volume.as_char());
        path.push(':');

        for &chain_idx in chain.iter().rev() {
            if let Some(record) = index.records.get(chain_idx) {
                let name = index.record_name(record);
                if !name.is_empty() && name != "." {
                    path.push('\\');
                    path.push_str(name);
                }
            }
        }
        // Normalize volume root to "X:\\"
        // NTFS root record (FRS=5) often has FILE_NAME="." which we skip above,
        // so without this normalization we'd return "X:" which is drive-relative.
        if path.len() == 2 && path.as_bytes().last() == Some(&b':') {
            path.push('\\');
        }

        path
    }

    /// Materialize path for a specific hard link.
    #[must_use]
    pub fn materialize_path_for_name(&self, index: &MftIndex, idx: usize, name_idx: u16) -> String {
        if name_idx == 0 {
            return self.materialize_path(index, idx);
        }

        let Some(record) = index.records.get(idx) else {
            return String::new();
        };
        let Some(link) = index.get_link_at(record, name_idx) else {
            return self.materialize_path(index, idx);
        };

        let parent_frs = link.parent_frs;
        let parent_path = if let Some(pidx) = index.frs_to_idx_opt(parent_frs.as_frs()) {
            self.materialize_path(index, pidx)
        } else if parent_frs.is_root() {
            // Normalize root to "X:\\" (not "X:") so hardlink paths keep
            // standard absolute-drive semantics.
            let mut root_path = String::with_capacity(3);
            root_path.push(self.volume.as_char());
            root_path.push(':');
            root_path.push('\\');
            root_path
        } else {
            return String::new();
        };

        let name = index.link_name(link);
        if name.is_empty() || name == "." {
            parent_path
        } else {
            let mut path = String::with_capacity(parent_path.len() + 1 + name.len());
            path.push_str(&parent_path);
            // Avoid double separators if parent is already the volume root ("X:\\").
            let ends_with_sep = parent_path.as_bytes().last() == Some(&b'\\');
            if !ends_with_sep {
                path.push('\\');
            }
            path.push_str(name);
            path
        }
    }

    /// Mark system metafiles (FRS 0-15 except root) as invalid.
    fn mark_system_metafiles_invalid(&mut self, index: &MftIndex) {
        for (idx, record) in index.records.iter().enumerate() {
            // System metafiles are FRS 0-15 except root — typed comparison
            // uses `.raw()` only at the numeric range check.
            let frs_raw = record.frs.raw();
            if frs_raw <= SYSTEM_METAFILE_MAX_FRS
                && !record.frs.is_root()
                && let Some(state) = self.state.get_mut(idx)
            {
                *state = path_state::INVALID;
                self.invalid_count += 1;
            }
        }
    }

    /// BFS propagation: mark all descendants of invalid nodes as invalid.
    fn propagate_invalid_to_descendants(&mut self, index: &MftIndex) {
        use alloc::collections::VecDeque;
        let mut queue: VecDeque<usize> = VecDeque::new();

        for (idx, &state) in self.state.iter().enumerate() {
            if state == path_state::INVALID {
                queue.push_back(idx);
            }
        }

        while let Some(parent_idx) = queue.pop_front() {
            let Some(record) = index.records.get(parent_idx) else {
                continue;
            };
            let mut child_entry = record.first_child;

            while child_entry != NO_ENTRY {
                let Some(child_info) = index.children.get(child_entry as usize) else {
                    break;
                };
                if let Some(child_idx) = index.frs_to_idx_opt(child_info.child_frs)
                    && let Some(state) = self.state.get_mut(child_idx)
                    && *state == path_state::UNSEEN
                {
                    // typed `Frs` flows through `frs_to_idx_opt` directly.
                    *state = path_state::INVALID;
                    self.invalid_count += 1;
                    queue.push_back(child_idx);
                }
                child_entry = child_info.next_entry;
            }
        }
    }

    /// Validate remaining unseen records by walking parent chains.
    fn validate_remaining(&mut self, index: &MftIndex) {
        for start_idx in 0..index.records.len() {
            if self.state.get(start_idx).copied() != Some(path_state::UNSEEN) {
                continue;
            }

            let mut chain: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
            let mut current_idx = start_idx;

            let final_state = loop {
                match self.state.get(current_idx).copied() {
                    Some(path_state::VALID) => break path_state::VALID,
                    // INVALID or VISITING (cycle) both result in INVALID
                    Some(path_state::INVALID | path_state::VISITING) => break path_state::INVALID,
                    _ => {}
                }

                if let Some(state) = self.state.get_mut(current_idx) {
                    *state = path_state::VISITING;
                }
                chain.push(current_idx);

                let Some(record) = index.records.get(current_idx) else {
                    break path_state::INVALID;
                };

                let parent_frs = record.first_name.parent_frs;

                if parent_frs.is_root() {
                    break path_state::VALID;
                }
                if parent_frs.as_frs() == record.frs
                    || parent_frs == ParentFrs::new(u64::from(NO_ENTRY))
                {
                    if record.frs.is_root() {
                        break path_state::VALID;
                    }
                    break path_state::INVALID;
                }

                let Some(parent_idx) = index.frs_to_idx_opt(parent_frs.as_frs()) else {
                    break path_state::INVALID;
                };
                current_idx = parent_idx;
            };

            for &chain_idx in &chain {
                if let Some(state) = self.state.get_mut(chain_idx) {
                    *state = final_state;
                    if final_state == path_state::VALID {
                        self.valid_count += 1;
                    } else {
                        self.invalid_count += 1;
                    }
                }
            }
        }
    }
}

// ============================================================================
// PathCache - Compatibility wrapper using PathResolver
// ============================================================================

/// Cached path result for a record.
pub type CachedPath = Option<String>;

/// Pre-computed path cache using `PathResolver` internally.
///
/// This is a compatibility wrapper that provides the same API as the old
/// `PathCache` but uses the much faster `PathResolver` under the hood.
///
/// ## Usage
///
/// ```ignore
/// let cache = PathCache::build(&index, false);
/// if let Some(path) = cache.get(record.frs) {
///     println!("Valid: {}", path);
/// }
/// ```
#[derive(Debug)]
pub struct PathCache<'a> {
    /// The underlying path resolver.
    resolver: PathResolver,
    /// Reference to the MFT index.
    index: &'a MftIndex,
    /// Pre-computed directory paths (indexed by `record_idx`, empty = not a
    /// valid directory).  Used by `materialize_path_cached` to short-circuit
    /// parent-chain walks.
    dir_cache: Vec<String>,
}

impl<'a> PathCache<'a> {
    /// Build the path cache for all records in the index.
    #[must_use]
    pub fn build(index: &'a MftIndex, include_system_metafiles: bool) -> Self {
        let resolver = PathResolver::build(index, include_system_metafiles);
        // Pre-compute directory paths using the correct materialize_path
        // algorithm.  ~500K directories × ~40 bytes avg = ~20 MB.
        let dir_cache = resolver.pre_cache_directory_paths(index);
        Self {
            resolver,
            index,
            dir_cache,
        }
    }

    /// Get the directory path cache for use with `materialize_path_cached`.
    #[inline]
    #[must_use]
    pub fn dir_cache(&self) -> &[String] {
        &self.dir_cache
    }

    /// Get the path for a record (materializes on demand).
    #[must_use]
    pub fn get(&self, frs: Frs) -> Option<String> {
        let idx = self.index.frs_to_idx_opt(frs)?;
        self.resolver
            .is_valid_idx(idx)
            .then(|| self.resolver.materialize_path(self.index, idx))
    }

    /// Check if a record is valid (has a path, not illegal).
    #[must_use]
    pub fn is_valid(&self, frs: Frs) -> bool {
        self.resolver.is_valid(self.index, frs)
    }

    /// Check if a record is illegal (filtered out).
    #[must_use]
    pub fn is_illegal(&self, frs: Frs) -> bool {
        self.index
            .frs_to_idx_opt(frs)
            .is_some_and(|idx| !self.resolver.is_valid_idx(idx))
    }

    /// Get the number of valid (non-illegal) records.
    #[must_use]
    pub const fn valid_count(&self) -> usize {
        self.resolver.valid_count() as usize
    }

    /// Get the number of illegal records.
    #[must_use]
    pub const fn illegal_count(&self) -> usize {
        self.resolver.invalid_count() as usize
    }

    /// Get the underlying resolver for direct access.
    #[must_use]
    pub const fn resolver(&self) -> &PathResolver {
        &self.resolver
    }

    /// Get the index reference.
    #[must_use]
    pub const fn index(&self) -> &MftIndex {
        self.index
    }
}
