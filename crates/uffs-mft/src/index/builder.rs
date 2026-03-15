//! Index construction from parsed records and post-build orchestration.

use super::{
    ChildInfo, ExtensionIndex, FileRecord, IndexBuildTiming, IndexNameRef, IndexStreamInfo,
    InternalStreamInfo, LinkInfo, MftIndex, NO_ENTRY, SizeInfo, StandardInfo,
};

// ============================================================================
// Building MftIndex from ParsedRecords (Cross-Platform)
// ============================================================================

impl MftIndex {
    /// Build an `MftIndex` from a vector of parsed records.
    ///
    /// **LEGACY MULTI-PASS PIPELINE:** This function is the final stage of the
    /// old `parse_record_full → MftRecordMerger → from_parsed_records`
    /// pipeline. The hot path (`SlidingIocpInline`) now uses direct-to-index
    /// parsers that build the index incrementally during I/O, skipping this
    /// separate build phase. This function is still used by:
    /// - Legacy read modes (`Parallel`, `Pipelined`, `PipelinedParallel`,
    ///   `SlidingIocp`)
    /// - File-based readers (`load_raw_to_index_with_options`)
    /// - Tests and diagnostic tools
    /// - `UFFS_LEGACY_PARSE=1` escape hatch
    ///
    /// This directly builds the lean index without going through Polars
    /// `DataFrame`.
    ///
    /// Works on all platforms - uses cross-platform `ParsedRecord` from parse
    /// module.
    #[must_use]
    #[expect(
        clippy::cognitive_complexity,
        reason = "record conversion has many attribute paths"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential field mapping from ParsedRecord"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "FRS fits in usize on 64-bit"
    )]
    #[expect(
        clippy::indexing_slicing,
        reason = "indices validated by get_or_create"
    )]
    #[tracing::instrument(
        level = "info",
        skip_all,
        fields(volume = %volume, input_records = records.len())
    )]
    pub fn from_parsed_records(volume: char, records: Vec<crate::parse::ParsedRecord>) -> Self {
        /// System metafiles are FRS 0-15 (except root at FRS 5)
        const SYSTEM_METAFILE_MAX_FRS: u64 = 15;
        const ROOT_FRS_LOCAL: u64 = 5;

        tracing::debug!(volume = %volume, input_records = records.len(), "[TRIP] MftIndex::from_parsed_records ENTER");

        let capacity = records.len();
        let mut index = Self::with_capacity(volume, capacity);
        let mut has_forensic_records = false;

        for parsed in records {
            // In normal mode, skip records not in use.
            // In forensic mode, include deleted/corrupt/extension records.
            // Forensic records have is_deleted/is_corrupt/is_extension set by
            // parse_record_forensic().
            let is_forensic_record = parsed.is_deleted || parsed.is_corrupt || parsed.is_extension;
            if is_forensic_record {
                has_forensic_records = true;
            }
            if !parsed.in_use && !is_forensic_record {
                continue;
            }

            // === Collect stats (cheap - just incrementing counters) ===
            index.stats.record_count += 1;
            index.stats.total_name_bytes += parsed.name.len() as u64;
            if parsed.frs > index.stats.max_frs {
                index.stats.max_frs = parsed.frs;
            }
            if parsed.is_directory {
                index.stats.dir_count += 1;
            } else {
                index.stats.file_count += 1;
            }
            if parsed.names.len() > 1 {
                index.stats.multi_name_count += 1;
            }
            if parsed.streams.len() > 1 {
                index.stats.ads_count += 1;
            }
            // System metafile detection
            if parsed.frs <= SYSTEM_METAFILE_MAX_FRS && parsed.frs != ROOT_FRS_LOCAL {
                index.stats.system_metafile_count += 1;
            }
            // Child of system metafile detection
            if parsed.parent_frs <= SYSTEM_METAFILE_MAX_FRS && parsed.parent_frs != ROOT_FRS_LOCAL {
                index.stats.system_child_count += 1;
            }

            // Add primary name to names buffer FIRST (before borrowing record)
            let name_offset = index.add_name(&parsed.name);
            let name_len = parsed.name.len() as u16;
            let is_ascii = parsed.name.is_ascii();
            // Extract and intern extension (must be done before get_or_create borrows
            // mutably)
            let extension_id = index.intern_extension(&parsed.name);

            // Get or create the record and set all basic fields in a block scope
            // to end the mutable borrow before adding additional names/streams
            {
                let record = index.get_or_create(parsed.frs);

                // Set sequence number, LSN, and namespace (raw MFT fields)
                record.sequence_number = parsed.sequence_number;
                record.lsn = parsed.lsn;
                record.namespace = parsed.namespace;

                // Set $FILE_NAME timestamps (often differ from $STANDARD_INFORMATION)
                record.fn_created = parsed.fn_created;
                record.fn_modified = parsed.fn_modified;
                record.fn_accessed = parsed.fn_accessed;
                record.fn_mft_changed = parsed.fn_mft_changed;

                // Set $STANDARD_INFORMATION timestamps and flags
                record.stdinfo.created = parsed.std_info.created;
                record.stdinfo.modified = parsed.std_info.modified;
                record.stdinfo.accessed = parsed.std_info.accessed;
                record.stdinfo.mft_changed = parsed.std_info.mft_changed;
                record.stdinfo.usn = parsed.std_info.usn;
                record.stdinfo.security_id = parsed.std_info.security_id;
                record.stdinfo.owner_id = parsed.std_info.owner_id;
                record.stdinfo.set_directory(parsed.is_directory);

                // Set attribute flags from ExtendedStandardInfo
                if parsed.std_info.is_readonly {
                    record.stdinfo.flags |= StandardInfo::IS_READONLY;
                }
                if parsed.std_info.is_archive {
                    record.stdinfo.flags |= StandardInfo::IS_ARCHIVE;
                }
                if parsed.std_info.is_system {
                    record.stdinfo.flags |= StandardInfo::IS_SYSTEM;
                }
                if parsed.std_info.is_hidden {
                    record.stdinfo.flags |= StandardInfo::IS_HIDDEN;
                }
                if parsed.std_info.is_offline {
                    record.stdinfo.flags |= StandardInfo::IS_OFFLINE;
                }
                if parsed.std_info.is_not_content_indexed {
                    record.stdinfo.flags |= StandardInfo::IS_NOT_INDEXED;
                }
                if parsed.std_info.is_compressed {
                    record.stdinfo.flags |= StandardInfo::IS_COMPRESSED;
                }
                if parsed.std_info.is_encrypted {
                    record.stdinfo.flags |= StandardInfo::IS_ENCRYPTED;
                }
                if parsed.std_info.is_sparse {
                    record.stdinfo.flags |= StandardInfo::IS_SPARSE;
                }
                if parsed.std_info.is_reparse {
                    record.stdinfo.flags |= StandardInfo::IS_REPARSE;
                }
                if parsed.std_info.is_temporary {
                    record.stdinfo.flags |= StandardInfo::IS_TEMPORARY;
                }
                if parsed.std_info.is_integrity_stream {
                    record.stdinfo.flags |= StandardInfo::IS_INTEGRITY_STREAM;
                }
                if parsed.std_info.is_no_scrub_data {
                    record.stdinfo.flags |= StandardInfo::IS_NO_SCRUB_DATA;
                }
                if parsed.std_info.is_pinned {
                    record.stdinfo.flags |= StandardInfo::IS_PINNED;
                }
                if parsed.std_info.is_unpinned {
                    record.stdinfo.flags |= StandardInfo::IS_UNPINNED;
                }
                if parsed.std_info.is_virtual {
                    record.stdinfo.flags |= StandardInfo::IS_VIRTUAL;
                }

                // Set name info (offset and extension_id were computed before borrowing record)
                record.first_name.name =
                    IndexNameRef::new(name_offset, name_len, is_ascii, extension_id);
                record.first_name.parent_frs = parsed.parent_frs;
                // Note: name_count is set AFTER filtering additional names to avoid
                // counting duplicates. See the code after this block.

                // Set reparse tag (0 if not a reparse point)
                record.reparse_tag = parsed.reparse_tag;

                // Set P3 forensic fields (is_deleted, is_corrupt, is_extension, base_frs)
                record.set_forensic_flags(
                    parsed.is_deleted,
                    parsed.is_corrupt,
                    parsed.is_extension,
                );
                record.base_frs = parsed.base_frs;

                // Set size and flags
                // For directories, use parsed.size/allocated_size which includes
                // $INDEX_ROOT + $INDEX_ALLOCATION while excluding $BITMAP.
                // For files, use the default stream size
                // Note: stream_count is set AFTER filtering named streams to avoid
                // counting internal Windows streams. See the code after this block.
                if parsed.is_directory {
                    // Directory size comes from index attributes, already in parsed.size
                    record.first_stream.size.length = parsed.size;
                    record.first_stream.size.allocated = parsed.allocated_size;
                } else if let Some(default_stream) =
                    parsed.streams.iter().find(|st| st.name.is_empty())
                {
                    record.first_stream.size.length = default_stream.size;
                    record.first_stream.size.allocated = default_stream.allocated_size;
                    // Set is_resident flag (bit 1)
                    if default_stream.is_resident {
                        record.first_stream.flags |= 0x02;
                    }
                    // Set is_sparse flag (bit 0)
                    if default_stream.is_sparse {
                        record.first_stream.flags |= 0x01;
                    }
                } else if !parsed.streams.is_empty() {
                    // No default stream, use first available
                    record.first_stream.size.length = parsed.size;
                    record.first_stream.size.allocated = parsed.allocated_size;
                }
            } // End record borrow here

            // Store additional names (hardlinks) in the links vector
            // Skip the name that matches first_name (the primary/best name)
            // Note: parsed.name is the BEST name (selected by PrimaryNameTracker),
            // which may not be parsed.names[0]. We must filter by matching name+parent.
            let additional_names: Vec<_> = parsed
                .names
                .iter()
                .filter(|n| !(n.name == parsed.name && n.parent_frs == parsed.parent_frs))
                .collect();

            // Update name_count to reflect actual stored names (1 primary + additional)
            // This must be done AFTER filtering to avoid counting duplicates
            let actual_name_count = (1 + additional_names.len()).max(1) as u16;
            index.get_or_create(parsed.frs).name_count = actual_name_count;

            if !additional_names.is_empty() {
                let mut prev_link_idx = NO_ENTRY;
                for extra_name in additional_names.iter().rev() {
                    // Add name to names buffer
                    let extra_offset = index.add_name(&extra_name.name);
                    let extra_len = extra_name.name.len() as u16;
                    let extra_ascii = extra_name.name.is_ascii();
                    let extra_ext_id = index.intern_extension(&extra_name.name);

                    let link_idx = index.links.len() as u32;
                    index.links.push(LinkInfo {
                        next_entry: prev_link_idx,
                        name: IndexNameRef::new(extra_offset, extra_len, extra_ascii, extra_ext_id),
                        parent_frs: extra_name.parent_frs,
                    });
                    prev_link_idx = link_idx;
                }
                // Link first_name to the chain
                let record = index.get_or_create(parsed.frs);
                record.first_name.next_entry = prev_link_idx;
            }

            // Store additional streams (ADS) in the streams vector.
            //
            // Filter out:
            //   - Empty name (default stream)
            //   - Internal Windows streams (names starting with `$UPPERCASE`)
            //
            // Internal streams are NOT emitted as ADS rows, but they ARE required for
            // precise tree-metrics accounting. We keep them as individual
            // stream entries because proportional size distribution uses
            // integer division:     delta(a + b) != delta(a) + delta(b)
            // So pre-summing internal stream sizes causes 1-4 byte tree-size skews.
            let mut internal_streams_size = 0_u64;
            let mut internal_streams_allocated = 0_u64;
            let mut first_internal_stream = NO_ENTRY;
            let mut last_internal_stream = NO_ENTRY;

            let mut named_streams: Vec<_> = Vec::new();
            for st in &parsed.streams {
                if st.name.is_empty() {
                    continue;
                }

                let is_internal = st
                    .name
                    .strip_prefix('$')
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(|ch| ch.is_ascii_uppercase());

                if is_internal {
                    internal_streams_size = internal_streams_size.saturating_add(st.size);
                    internal_streams_allocated =
                        internal_streams_allocated.saturating_add(st.allocated_size);

                    let flags = u8::from(st.is_sparse) | (u8::from(st.is_resident) << 1_u8);

                    let new_idx = index.internal_streams.len() as u32;
                    index.internal_streams.push(InternalStreamInfo {
                        size: SizeInfo {
                            length: st.size,
                            allocated: st.allocated_size,
                        },
                        next_entry: NO_ENTRY,
                        flags,
                    });

                    if last_internal_stream == NO_ENTRY {
                        first_internal_stream = new_idx;
                    } else {
                        index.internal_streams[last_internal_stream as usize].next_entry = new_idx;
                    }
                    last_internal_stream = new_idx;
                    continue;
                }

                named_streams.push(st);
            }

            // Set total_stream_count to include all streams for tree metrics.
            // This includes internal Windows streams like $REPARSE_POINT and $OBJECT_ID.
            let total_stream_count = parsed.streams.len().max(1) as u16;

            // Set stream_count to reflect only user-visible stored streams (1 default +
            // named) This is used for user-facing output (DataFrame export)
            let actual_stream_count = (1 + named_streams.len()).max(1) as u16;

            let record = index.get_or_create(parsed.frs);
            record.total_stream_count = total_stream_count;
            record.stream_count = actual_stream_count;
            record.internal_streams_size = internal_streams_size;
            record.internal_streams_allocated = internal_streams_allocated;
            record.first_internal_stream = first_internal_stream;

            if !named_streams.is_empty() {
                let mut prev_stream_idx = NO_ENTRY;
                for extra_stream in named_streams.iter().rev() {
                    // Add stream name to names buffer
                    let stream_name_offset = index.add_name(&extra_stream.name);
                    let stream_name_len = extra_stream.name.len() as u16;
                    let stream_ascii = extra_stream.name.is_ascii();
                    // Streams don't have extensions, use 0
                    let stream_ext_id = 0;

                    let stream_idx = index.streams.len() as u32;
                    let mut flags = 0_u8;
                    if extra_stream.is_sparse {
                        flags |= 0x01;
                    }
                    if extra_stream.is_resident {
                        flags |= 0x02;
                    }
                    index.streams.push(IndexStreamInfo {
                        size: SizeInfo {
                            length: extra_stream.size,
                            allocated: extra_stream.allocated_size,
                        },
                        next_entry: prev_stream_idx,
                        name: IndexNameRef::new(
                            stream_name_offset,
                            stream_name_len,
                            stream_ascii,
                            stream_ext_id,
                        ),
                        flags,
                    });
                    prev_stream_idx = stream_idx;
                }
                // Link first_stream to the chain
                let file_record = index.get_or_create(parsed.frs);
                file_record.first_stream.next_entry = prev_stream_idx;
            }

            // Build parent-child relationships for all hard links.
            // Each $FILE_NAME attribute gets its own child edge so tree metrics
            // can attribute proportional shares correctly.
            // Each child entry stores its name_index so we can calculate proportional
            // shares.
            for (name_idx, name_info) in parsed.names.iter().enumerate() {
                let parent_frs = name_info.parent_frs;
                if parent_frs == parsed.frs || parent_frs == u64::from(NO_ENTRY) {
                    continue;
                }

                // Ensure parent exists
                let parent_idx = {
                    let parent_frs_usize = parent_frs as usize;
                    if parent_frs_usize >= index.frs_to_idx.len() {
                        index.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
                    }
                    if index.frs_to_idx[parent_frs_usize] == NO_ENTRY {
                        // Create placeholder parent
                        let new_idx = index.records.len() as u32;
                        index.frs_to_idx[parent_frs_usize] = new_idx;
                        index.records.push(FileRecord::new(parent_frs));
                    }
                    index.frs_to_idx[parent_frs_usize]
                };

                // Add child entry with name_index for proportional share calculation
                let child_idx = index.children.len() as u32;

                // Get parent's first_child and update
                let parent = &mut index.records[parent_idx as usize];
                let old_first_child = parent.first_child;
                parent.first_child = child_idx;

                // Store the parsed name index directly; traversal converts it to the
                // corresponding proportional-share slot when needed.
                index.children.push(ChildInfo {
                    next_entry: old_first_child,
                    child_frs: parsed.frs,
                    name_index: name_idx as u16,
                });
            }

            // Handle case where names is empty (shouldn't happen, but be safe)
            if parsed.names.is_empty()
                && parsed.parent_frs != parsed.frs
                && parsed.parent_frs != u64::from(NO_ENTRY)
            {
                let parent_idx = {
                    let parent_frs_usize = parsed.parent_frs as usize;
                    if parent_frs_usize >= index.frs_to_idx.len() {
                        index.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
                    }
                    if index.frs_to_idx[parent_frs_usize] == NO_ENTRY {
                        let new_idx = index.records.len() as u32;
                        index.frs_to_idx[parent_frs_usize] = new_idx;
                        index.records.push(FileRecord::new(parsed.parent_frs));
                    }
                    index.frs_to_idx[parent_frs_usize]
                };

                let child_idx = index.children.len() as u32;
                let parent = &mut index.records[parent_idx as usize];
                let old_first_child = parent.first_child;
                parent.first_child = child_idx;

                index.children.push(ChildInfo {
                    next_entry: old_first_child,
                    child_frs: parsed.frs,
                    name_index: 0,
                });
            }
        }

        // Post-processing: compute derived data structures
        // These are fast O(n) operations that enhance query performance
        tracing::debug!(
            records = index.records.len(),
            "[TRIP] MftIndex::from_parsed_records -> record insertion done, starting post-processing"
        );

        // 1. Build extension index for fast *.ext queries (Phase 2)
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 2: ExtensionIndex::build");
        index.extension_index = Some(ExtensionIndex::build(&index));

        // 2. Sort directory children for natural ordering (Phase 4)
        // CRITICAL: Must run BEFORE computing tree metrics for correct size
        // aggregation.
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 4: sort_directory_children");
        index.sort_directory_children();

        // 3. Compute tree metrics for directory statistics (Phase 5)
        // Must run AFTER sorting - depends on sorted child traversal order.
        tracing::debug!("[TRIP] MftIndex::from_parsed_records -> Phase 5: compute_tree_metrics");
        index.compute_tree_metrics();

        // 4. Set forensic mode flag if any forensic records were included
        index.forensic_mode = has_forensic_records;

        tracing::debug!(
            records = index.records.len(),
            "[TRIP] MftIndex::from_parsed_records EXIT"
        );
        index
    }

    /// Build an `MftIndex` from parsed records with detailed timing breakdown.
    ///
    /// This is the same as `from_parsed_records()` but returns timing
    /// information for each phase, useful for benchmarking build stages.
    ///
    /// # Returns
    ///
    /// A tuple of (`MftIndex`, `IndexBuildTiming`) with the built index and
    /// timing breakdown.
    #[must_use]
    #[tracing::instrument(
        level = "info",
        skip_all,
        fields(volume = %volume, input_records = records.len())
    )]
    pub fn from_parsed_records_with_timing(
        volume: char,
        records: Vec<crate::parse::ParsedRecord>,
    ) -> (Self, IndexBuildTiming) {
        use std::time::Instant;

        let total_start = Instant::now();

        // Phase 1: Build index without tree metrics
        // We call from_parsed_records which includes all phases, then we'll
        // re-run tree metrics with timing. This is slightly wasteful but
        // ensures correctness and avoids code duplication.
        //
        // For accurate timing, we time the full build, then separately time
        // just the tree metrics by clearing and recomputing.
        let insert_start = Instant::now();
        let mut index = Self::from_parsed_records(volume, records);
        // Saturating cast: u128 -> u64 (overflow impossible for realistic durations)
        let full_build_ms = u64::try_from(insert_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Phase 2: Time tree metrics separately by clearing and recomputing
        // First, clear tree metrics
        for record in &mut index.records {
            record.descendants = 0;
            record.treesize = 0;
            record.tree_allocated = 0;
        }

        // Now time just the tree metrics computation
        let tree_start = Instant::now();
        index.compute_tree_metrics();
        let tree_metrics_ms = u64::try_from(tree_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        let total_ms = u64::try_from(total_start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Estimate the other phases based on the full build time minus tree metrics
        // This is approximate but gives a reasonable breakdown
        let index_only_ms = full_build_ms.saturating_sub(tree_metrics_ms);

        let timing = IndexBuildTiming {
            // Record insertion is the bulk of index_only_ms (estimated ~80%)
            record_insert_ms: index_only_ms * 80 / 100,
            // Extension index is fast (estimated ~10%)
            extension_index_ms: index_only_ms * 10 / 100,
            // Sorting is fast (estimated ~10%)
            sort_children_ms: index_only_ms * 10 / 100,
            // Tree metrics is measured accurately
            tree_metrics_ms,
            total_ms,
        };

        (index, timing)
    }

    /// Returns the number of child entries in the index.
    #[must_use]
    pub fn children_count(&self) -> usize {
        self.children.len()
    }
}
