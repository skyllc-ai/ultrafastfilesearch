//! Name iteration and Windows path materialization for index records.

use super::*;

// ============================================================================
// Name/Stream Iteration (for hard link and ADS expansion)
// ============================================================================

/// Iterator over all names (hard links) for a record.
pub struct NameIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first name (inline in the record), consumed on first iteration.
    first: Option<&'a LinkInfo>,
    /// Index into `index.links` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for NameIter<'a> {
    type Item = (u16, &'a LinkInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_name
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let link = self.index.links.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = link.next_entry;
        Some((idx, link))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // We don't know the exact count without traversing
        (1, None)
    }
}

/// Iterator over all streams for a record.
pub struct StreamIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first stream (inline in the record), consumed on first iteration.
    first: Option<&'a IndexStreamInfo>,
    /// Index into `index.streams` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for StreamIter<'a> {
    type Item = (u16, &'a IndexStreamInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_stream
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let stream = self.index.streams.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = stream.next_entry;
        Some((idx, stream))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (1, None)
    }
}

impl MftIndex {
    /// Iterate over all names (hard links) for a record.
    ///
    /// Most files have only one name (the primary), but files with hard links
    /// will have multiple entries. Each name has its own parent directory.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "iterator construction is not const-compatible"
    )]
    pub fn iter_names<'a>(&'a self, record: &'a FileRecord) -> NameIter<'a> {
        NameIter {
            index: self,
            first: Some(&record.first_name),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Iterate over all streams for a record.
    ///
    /// Most files have only the default `$DATA` stream, but files with
    /// Alternate Data Streams (ADS) will have multiple entries.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "iterator construction is not const-compatible"
    )]
    pub fn iter_streams<'a>(&'a self, record: &'a FileRecord) -> StreamIter<'a> {
        StreamIter {
            index: self,
            first: Some(&record.first_stream),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Get the stream name (empty for default `$DATA` stream).
    #[must_use]
    pub fn stream_name(&self, stream: &IndexStreamInfo) -> &str {
        self.get_name(&stream.name)
    }

    /// Get the Nth name (hard link) for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_name_at<'a>(&'a self, record: &'a FileRecord, idx: u16) -> Option<&'a LinkInfo> {
        if idx == 0 {
            return Some(&record.first_name);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_name.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.links.get(current as usize);
            }
            if let Some(link) = self.links.get(current as usize) {
                current = link.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Get the Nth stream for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_stream_at<'a>(
        &'a self,
        record: &'a FileRecord,
        idx: u16,
    ) -> Option<&'a IndexStreamInfo> {
        if idx == 0 {
            return Some(&record.first_stream);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_stream.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.streams.get(current as usize);
            }
            if let Some(stream) = self.streams.get(current as usize) {
                current = stream.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Build the full path for a specific name (hard link) of a record.
    ///
    /// This handles the case where a file has multiple hard links, each
    /// with a different parent directory and thus a different path.
    #[must_use]
    pub fn build_path_for_name(&self, record: &FileRecord, name_idx: u16) -> String {
        let Some(name_info) = self.get_name_at(record, name_idx) else {
            return self.build_path(record.frs); // Fallback to primary
        };

        let mut components = Vec::new();

        // Add the file's own name
        let name = self.get_name(&name_info.name);
        if !name.is_empty() && name != "." {
            components.push(name.to_owned());
        }

        // Walk up the parent chain from this name's parent
        let mut current_frs = name_info.parent_frs;

        while current_frs != u64::from(NO_ENTRY) && current_frs != ROOT_FRS {
            if let Some(parent_record) = self.find(current_frs) {
                let parent_name = self.record_name(parent_record);
                if !parent_name.is_empty() && parent_name != "." {
                    components.push(parent_name.to_owned());
                }

                let parent_frs = parent_record.first_name.parent_frs;
                if parent_frs == u64::from(NO_ENTRY) || parent_frs == current_frs {
                    break;
                }
                current_frs = parent_frs;
            } else {
                break;
            }
        }

        // Reverse and join with a standard drive-qualified backslash path.
        components.reverse();
        format!(
            "{}:\\{}",
            self.volume.to_ascii_uppercase(),
            components.join("\\")
        )
    }

    /// Build the full path including stream name for ADS.
    ///
    /// Format: `C:/path/to/file:stream_name` for ADS
    /// Format: `C:/path/to/file` for default stream
    #[must_use]
    pub fn build_path_with_stream(
        &self,
        record: &FileRecord,
        name_idx: u16,
        stream: &IndexStreamInfo,
    ) -> String {
        let base_path = self.build_path_for_name(record, name_idx);
        let stream_name = self.stream_name(stream);

        if stream_name.is_empty() {
            base_path
        } else {
            format!("{base_path}:{stream_name}")
        }
    }
}

// ============================================================================
// Path resolution (on-demand parent-chain traversal)
// ============================================================================

impl MftIndex {
    /// Build the full path for a record by traversing parent chain.
    ///
    /// This is done on-demand (not stored) to save memory.
    #[must_use]
    pub fn build_path(&self, frs: u64) -> String {
        let mut components = Vec::new();
        let mut current_frs = frs;

        // Walk up the parent chain
        while let Some(record) = self.find(current_frs) {
            let name = self.record_name(record);
            if !name.is_empty() && name != "." {
                components.push(name.to_owned());
            }

            // Move to parent
            let parent_frs = record.first_name.parent_frs;
            if parent_frs == u64::from(NO_ENTRY) || parent_frs == current_frs {
                break; // Root or self-reference
            }
            if parent_frs == ROOT_FRS {
                break; // Reached root
            }
            current_frs = parent_frs;
        }

        // Reverse and join with a standard drive-qualified backslash path.
        components.reverse();
        format!(
            "{}:\\{}",
            self.volume.to_ascii_uppercase(),
            components.join("\\")
        )
    }
}

// ============================================================================
// PathResolver - Ultra-fast path validity and on-demand materialization
// ============================================================================

/// System metafiles are FRS 0-15 (except root at FRS 5).
/// These are filtered out by default from regular path results.
const SYSTEM_METAFILE_MAX_FRS: u64 = 15;

/// State values for path resolution.
mod path_state {
    /// Record has not been visited yet.
    pub const UNSEEN: u8 = 0;
    /// Record is currently being visited (cycle detection).
    pub const VISITING: u8 = 1;
    /// Record has a valid path to root.
    pub const VALID: u8 = 2;
    /// Record is invalid (system metafile, cycle, or descendant of invalid).
    pub const INVALID: u8 = 3;
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
    volume: char,
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
    pub fn is_valid(&self, index: &MftIndex, frs: u64) -> bool {
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
            if parent_frs == ROOT_FRS
                || parent_frs == record.frs
                || parent_frs == u64::from(NO_ENTRY)
            {
                break;
            }
            let Some(parent_idx) = index.frs_to_idx_opt(parent_frs) else {
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
        path.push(self.volume.to_ascii_uppercase());
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
        let parent_path = if let Some(pidx) = index.frs_to_idx_opt(parent_frs) {
            self.materialize_path(index, pidx)
        } else if parent_frs == ROOT_FRS {
            // Normalize root to "X:\\" (not "X:") so hardlink paths keep
            // standard absolute-drive semantics.
            let mut root_path = String::with_capacity(3);
            root_path.push(self.volume.to_ascii_uppercase());
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
            if record.frs <= SYSTEM_METAFILE_MAX_FRS && record.frs != ROOT_FRS {
                if let Some(state) = self.state.get_mut(idx) {
                    *state = path_state::INVALID;
                    self.invalid_count += 1;
                }
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
                if let Some(child_idx) = index.frs_to_idx_opt(child_info.child_frs) {
                    if let Some(state) = self.state.get_mut(child_idx) {
                        if *state == path_state::UNSEEN {
                            *state = path_state::INVALID;
                            self.invalid_count += 1;
                            queue.push_back(child_idx);
                        }
                    }
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

                if parent_frs == ROOT_FRS {
                    break path_state::VALID;
                }
                if parent_frs == record.frs || parent_frs == u64::from(NO_ENTRY) {
                    if record.frs == ROOT_FRS {
                        break path_state::VALID;
                    }
                    break path_state::INVALID;
                }

                let Some(parent_idx) = index.frs_to_idx_opt(parent_frs) else {
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
}

impl<'a> PathCache<'a> {
    /// Build the path cache for all records in the index.
    #[must_use]
    pub fn build(index: &'a MftIndex, include_system_metafiles: bool) -> Self {
        Self {
            resolver: PathResolver::build(index, include_system_metafiles),
            index,
        }
    }

    /// Get the path for a record (materializes on demand).
    #[must_use]
    pub fn get(&self, frs: u64) -> Option<String> {
        let idx = self.index.frs_to_idx_opt(frs)?;
        self.resolver
            .is_valid_idx(idx)
            .then(|| self.resolver.materialize_path(self.index, idx))
    }

    /// Check if a record is valid (has a path, not illegal).
    #[must_use]
    pub fn is_valid(&self, frs: u64) -> bool {
        self.resolver.is_valid(self.index, frs)
    }

    /// Check if a record is illegal (filtered out).
    #[must_use]
    pub fn is_illegal(&self, frs: u64) -> bool {
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
