//! Core `MftIndex` constructors, lookup helpers, and stats utilities.

use super::*;

impl MftIndex {
    /// Create a new empty index for the given volume
    #[must_use]
    pub fn new(volume: char) -> Self {
        Self {
            volume,
            extensions: ExtensionTable::new(),
            ..Default::default()
        }
    }

    /// Create with pre-allocated capacity
    #[must_use]
    pub fn with_capacity(volume: char, record_capacity: usize) -> Self {
        Self {
            volume,
            records: Vec::with_capacity(record_capacity),
            frs_to_idx: Vec::with_capacity(record_capacity),
            names: String::with_capacity(record_capacity * 20), // ~20 chars avg
            links: Vec::new(),
            streams: Vec::new(),
            internal_streams: Vec::new(),
            children: Vec::with_capacity(record_capacity),
            stats: MftStats::new(),
            extensions: ExtensionTable::new(),
            extension_index: None,
            forensic_mode: false,
        }
    }

    /// Recompute stats from the current index data.
    ///
    /// This is useful after deserializing an index from disk,
    /// or after merging fragments.
    pub fn recompute_stats(&mut self) {
        /// System metafiles are FRS 0-15 (except root at FRS 5)
        const SYSTEM_METAFILE_MAX_FRS: u64 = 15;
        const ROOT_FRS_LOCAL: u64 = 5;

        let mut stats = MftStats::new();

        for record in &self.records {
            stats.record_count += 1;

            // Track max FRS
            if record.frs > stats.max_frs {
                stats.max_frs = record.frs;
            }

            // Get file size from first stream
            let file_size = record.first_stream.size.length;

            // Count directories vs files
            if record.is_directory() {
                stats.dir_count += 1;
                stats.dir_bytes += file_size;
            } else {
                stats.file_count += 1;
            }

            // Track total bytes
            stats.total_bytes += file_size;

            // Track size buckets (Phase 3)
            let bucket = MftStats::size_bucket(file_size);
            if let Some(count) = stats.size_bucket_counts.get_mut(bucket) {
                *count += 1;
            }
            if let Some(bytes) = stats.size_bucket_bytes.get_mut(bucket) {
                *bytes += file_size;
            }

            // Track attribute-specific bytes (Phase 3)
            if record.stdinfo.is_hidden() {
                stats.hidden_bytes += file_size;
            }
            if record.stdinfo.is_system() {
                stats.system_bytes += file_size;
            }
            if record.stdinfo.is_compressed() {
                stats.compressed_bytes += file_size;
            }
            if record.stdinfo.is_encrypted() {
                stats.encrypted_bytes += file_size;
            }
            if record.stdinfo.is_sparse() {
                stats.sparse_bytes += file_size;
            }
            if record.stdinfo.is_reparse() {
                stats.reparse_bytes += file_size;
            }

            // Count multi-name records (hard links)
            if record.name_count > 1 {
                stats.multi_name_count += 1;
            }

            // Count ADS records
            if record.stream_count > 1 {
                stats.ads_count += 1;
            }

            // System metafile detection
            if record.frs <= SYSTEM_METAFILE_MAX_FRS && record.frs != ROOT_FRS_LOCAL {
                stats.system_metafile_count += 1;
            }

            // Child of system metafile detection
            let parent_frs = record.first_name.parent_frs;
            if parent_frs <= SYSTEM_METAFILE_MAX_FRS && parent_frs != ROOT_FRS_LOCAL {
                stats.system_child_count += 1;
            }

            // Sum name bytes
            stats.total_name_bytes += u64::from(record.first_name.name.length());
        }

        self.stats = stats;
    }

    /// Get or create a record for the given FRS.
    ///
    /// Returns a mutable reference to the record. Creates a new record if
    /// one doesn't exist for the given FRS.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "FRS fits in usize on 64-bit"
    )]
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: resize ensures frs_usize < len"
    )]
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        let frs_usize = frs as usize;

        // Expand lookup table if needed
        if frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
        }

        let idx = self.frs_to_idx[frs_usize];
        if idx == NO_ENTRY {
            // Create new record
            let new_idx = self.records.len() as u32;
            self.frs_to_idx[frs_usize] = new_idx;
            self.records.push(FileRecord::new(frs));
            let len = self.records.len();
            &mut self.records[len - 1]
        } else {
            &mut self.records[idx as usize]
        }
    }

    /// Find a record by FRS (returns None if not present)
    #[must_use]
    pub fn find(&self, frs: u64) -> Option<&FileRecord> {
        let frs_usize = usize::try_from(frs).ok()?;
        let idx = *self.frs_to_idx.get(frs_usize)?;
        if idx == NO_ENTRY {
            None
        } else {
            self.records.get(usize::try_from(idx).ok()?)
        }
    }

    /// Add a filename to the names buffer, return the offset
    pub fn add_name(&mut self, name: &str) -> u32 {
        let offset = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push_str(name);
        offset
    }

    /// Extract extension from a filename and intern it.
    ///
    /// Returns the `extension_id` for the extension (0 if no extension).
    /// Extensions are normalized to lowercase without the leading dot.
    pub fn intern_extension(&mut self, filename: &str) -> u16 {
        // Find the last dot in the filename
        if let Some(dot_pos) = filename.rfind('.') {
            // Make sure it's not a hidden file (e.g., ".gitignore")
            // and not at the end (e.g., "file.")
            if dot_pos > 0 && dot_pos < filename.len() - 1 {
                if let Some(extension) = filename.get(dot_pos + 1..) {
                    return self.extensions.intern(extension);
                }
            }
        }

        // No extension found
        0
    }

    /// Build the extension index for O(matches) queries.
    ///
    /// This should be called after all records are parsed and before
    /// performing extension-based queries.
    ///
    /// # Performance
    ///
    /// - Build time: O(n) where n = number of files
    /// - Memory overhead: ~4 MB per 1M files
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.build_extension_index();
    ///
    /// // Now extension queries are O(matches) instead of O(n)
    /// if let Some(ext_index) = &index.extension_index {
    ///     let txt_files = ext_index.get_records(txt_id);
    /// }
    /// ```
    pub fn build_extension_index(&mut self) {
        self.extension_index = Some(ExtensionIndex::build(self));
    }

    /// Get a filename from the names buffer
    #[must_use]
    pub fn get_name(&self, info: &IndexNameRef) -> &str {
        if !info.is_valid() {
            return "";
        }
        let start = info.offset as usize;
        let end = start + info.length() as usize;
        self.names.get(start..end).unwrap_or("")
    }

    /// Get the primary name of a record
    #[must_use]
    pub fn record_name(&self, record: &FileRecord) -> &str {
        self.get_name(&record.first_name.name)
    }

    /// Get all records as a slice.
    #[must_use]
    pub fn records(&self) -> &[FileRecord] {
        &self.records
    }

    /// Number of records in the index
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if index is empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Count files (non-directories)
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.records
            .iter()
            .filter(|rec| !rec.is_directory())
            .count()
    }

    /// Count directories
    #[must_use]
    pub fn dir_count(&self) -> usize {
        self.records.iter().filter(|rec| rec.is_directory()).count()
    }

    /// Memory usage estimate in bytes
    #[must_use]
    pub fn memory_usage(&self) -> usize {
        use core::mem::size_of;
        size_of::<Self>()
            + self.records.capacity() * size_of::<FileRecord>()
            + self.frs_to_idx.capacity() * size_of::<u32>()
            + self.names.capacity()
            + self.links.capacity() * size_of::<LinkInfo>()
            + self.streams.capacity() * size_of::<IndexStreamInfo>()
            + self.children.capacity() * size_of::<ChildInfo>()
    }

    /// Convert FRS to record index (returns None if not present).
    #[must_use]
    pub fn frs_to_idx_opt(&self, frs: u64) -> Option<usize> {
        let frs_usize = usize::try_from(frs).ok()?;
        let idx = *self.frs_to_idx.get(frs_usize)?;
        if idx == NO_ENTRY {
            None
        } else {
            Some(usize::try_from(idx).ok()?)
        }
    }

    /// Get a specific hard link by index (0 = `first_name`, 1+ = overflow
    /// links).
    #[must_use]
    pub fn get_link_at<'a>(
        &'a self,
        record: &'a FileRecord,
        name_idx: u16,
    ) -> Option<&'a LinkInfo> {
        if name_idx == 0 {
            return Some(&record.first_name);
        }
        let mut current = record.first_name.next_entry;
        let mut idx = 1_u16;
        while current != NO_ENTRY {
            let link = self.links.get(current as usize)?;
            if idx == name_idx {
                return Some(link);
            }
            current = link.next_entry;
            idx += 1;
        }
        None
    }

    /// Display enhanced statistics to stdout.
    ///
    /// This shows:
    /// - Basic counts (files, directories)
    /// - Byte counters (total, hidden, system, etc.)
    /// - Size distribution buckets
    /// - Top extensions by count and by bytes
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// index.display_stats();
    /// ```
    #[expect(
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for display"
    )]
    #[expect(
        clippy::float_arithmetic,
        reason = "used for percentage and size formatting"
    )]
    #[expect(clippy::min_ident_chars, reason = "'n' for count is idiomatic")]
    #[expect(clippy::too_many_lines, reason = "stats display has many fields")]
    pub fn display_stats(&self) {
        use std::io::Write;

        let mut out = std::io::stdout().lock();
        let sep = "═══════════════════════════════════════════════════════════════";

        // Helper to format sizes
        let format_size = |bytes: u64| -> String {
            const KB: u64 = 1024;
            const MB: u64 = KB * 1024;
            const GB: u64 = MB * 1024;
            const TB: u64 = GB * 1024;

            if bytes >= TB {
                format!("{:.2} TB", bytes as f64 / TB as f64)
            } else if bytes >= GB {
                format!("{:.2} GB", bytes as f64 / GB as f64)
            } else if bytes >= MB {
                format!("{:.2} MB", bytes as f64 / MB as f64)
            } else if bytes >= KB {
                format!("{:.2} KB", bytes as f64 / KB as f64)
            } else {
                format!("{bytes} B")
            }
        };

        // Helper to format numbers with commas
        let format_number = |n: u64| -> String {
            let s = n.to_string();
            let mut result = String::new();
            for (i, c) in s.chars().rev().enumerate() {
                if i > 0 && i % 3 == 0 {
                    result.push(',');
                }
                result.push(c);
            }
            result.chars().rev().collect()
        };

        writeln!(out, "{sep}").ok();
        writeln!(out, "                    ENHANCED MFT STATISTICS").ok();
        writeln!(out, "{sep}\n").ok();

        // Basic counts
        writeln!(out, "📊 RECORD COUNTS").ok();
        writeln!(
            out,
            "  Total records:        {}",
            format_number(u64::from(self.stats.record_count))
        )
        .ok();
        writeln!(
            out,
            "  Directories:          {}",
            format_number(u64::from(self.stats.dir_count))
        )
        .ok();
        writeln!(
            out,
            "  Files:                {}\n",
            format_number(u64::from(self.stats.file_count))
        )
        .ok();

        // Byte counters
        writeln!(out, "💾 SIZE METRICS").ok();
        writeln!(
            out,
            "  Total bytes:          {}",
            format_size(self.stats.total_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Directory bytes:      {}",
            format_size(self.stats.dir_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Hidden bytes:         {}",
            format_size(self.stats.hidden_bytes)
        )
        .ok();
        writeln!(
            out,
            "  System bytes:         {}",
            format_size(self.stats.system_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Compressed bytes:     {}",
            format_size(self.stats.compressed_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Encrypted bytes:      {}",
            format_size(self.stats.encrypted_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Sparse bytes:         {}",
            format_size(self.stats.sparse_bytes)
        )
        .ok();
        writeln!(
            out,
            "  Reparse bytes:        {}\n",
            format_size(self.stats.reparse_bytes)
        )
        .ok();

        // Size distribution
        writeln!(out, "📏 SIZE DISTRIBUTION").ok();
        let bucket_names = [
            "0-1 KB",
            "1-10 KB",
            "10-100 KB",
            "100 KB-1 MB",
            "1-10 MB",
            "10-100 MB",
            "100 MB-1 GB",
            ">1 GB",
        ];
        for (i, name) in bucket_names.iter().enumerate() {
            if let (Some(&count), Some(&bytes)) = (
                self.stats.size_bucket_counts.get(i),
                self.stats.size_bucket_bytes.get(i),
            ) {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    name,
                    format_number(u64::from(count)),
                    format_size(bytes)
                )
                .ok();
            }
        }
        writeln!(out).ok();

        // Top extensions by count
        writeln!(out, "🏆 TOP EXTENSIONS BY COUNT").ok();
        let top_by_count = self.extensions.top_by_count(10);
        if top_by_count.is_empty() {
            writeln!(out, "  (no extensions)").ok();
        } else {
            for (_ext_id, ext, count, bytes) in &top_by_count {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    ext,
                    format_number(u64::from(*count)),
                    format_size(*bytes)
                )
                .ok();
            }
        }
        writeln!(out).ok();

        // Top extensions by bytes
        writeln!(out, "🏆 TOP EXTENSIONS BY SIZE").ok();
        let top_by_bytes = self.extensions.top_by_bytes(10);
        if top_by_bytes.is_empty() {
            writeln!(out, "  (no extensions)").ok();
        } else {
            for (_ext_id, ext, bytes, count) in &top_by_bytes {
                writeln!(
                    out,
                    "  {:15} {:>10} files  ({:>10})",
                    ext,
                    format_number(u64::from(*count)),
                    format_size(*bytes)
                )
                .ok();
            }
        }

        writeln!(out, "\n{sep}").ok();
    }

    /// Get the name string for a link.
    #[must_use]
    pub fn link_name(&self, link: &LinkInfo) -> &str {
        self.get_name(&link.name)
    }
}
