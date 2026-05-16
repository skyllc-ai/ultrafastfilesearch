// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Fragment merge helpers for combining worker-local index fragments.

use super::{
    ChildInfo, ExtensionIndex, FileRecord, IndexNameRef, IndexStreamInfo, InternalStreamInfo,
    LinkInfo, MftIndex, MftIndexFragment, NO_ENTRY, len_to_u32,
};

/// Returns true when a record carries overflow name/stream payload to merge.
#[inline]
const fn has_merge_payload(record: &FileRecord) -> bool {
    record.first_name.name.is_valid()
        || record.first_name.next_entry != NO_ENTRY
        || record.first_stream.name.is_valid()
        || record.first_stream.next_entry != NO_ENTRY
        || record.first_internal_stream != NO_ENTRY
}

impl MftIndex {
    /// Merge multiple index fragments into this index.
    ///
    /// This is used for parallel parsing where each worker builds a local
    /// fragment, then all fragments are merged into the final index.
    ///
    /// # Performance
    ///
    /// O(n) merge - each fragment is processed once. The merge handles:
    /// - Deduplication of records (same FRS from different fragments)
    /// - Name buffer concatenation with offset adjustment
    /// - Link/stream/child list merging
    // cognitive_complexity fires in `--lib` but not `--tests`, so `#[expect]` is
    // unreliable — use `#[allow]` and suppress the meta-lint.
    #[expect(
        clippy::allow_attributes,
        reason = "cognitive_complexity differs between lib and test compilation"
    )]
    #[allow(
        clippy::cognitive_complexity,
        reason = "O(n) merge of parallel fragments: dedup records, rebase name/link/stream/child offsets"
    )]
    pub fn merge_fragments(&mut self, fragments: Vec<MftIndexFragment>) {
        use tracing::debug;

        let total_records: usize = fragments.iter().map(|frag| frag.records.len()).sum();
        let total_names: usize = fragments.iter().map(|frag| frag.names.len()).sum();

        debug!(
            fragments = fragments.len(),
            total_records, total_names, "🔀 Merging index fragments"
        );

        self.records.reserve(total_records);
        self.names.reserve(total_names);

        for fragment in fragments {
            self.merge_single_fragment(fragment);
        }

        debug!(
            records = self.records.len(),
            names_kb = self.names.len() / 1024,
            "✅ Fragment merge complete"
        );

        debug!("🔨 Building extension index...");
        self.extension_index = Some(ExtensionIndex::build(self));

        debug!("🔨 Sorting directory children...");
        self.sort_directory_children();

        debug!("🔨 Computing tree metrics...");
        self.compute_tree_metrics();

        debug!("✅ Post-processing complete");
    }

    /// Merge a single fragment into this index.
    fn merge_single_fragment(&mut self, fragment: MftIndexFragment) {
        let name_offset_adjustment = len_to_u32(self.names.len());
        let link_offset_adjustment = len_to_u32(self.links.len());
        let stream_offset_adjustment = len_to_u32(self.streams.len());
        let internal_stream_offset_adjustment = len_to_u32(self.internal_streams.len());

        let extension_id_map = self.build_extension_id_map(&fragment);
        self.names.push_str(&fragment.names);

        let records_to_merge = self.merge_fragment_records_with_deferred_merge(
            fragment.records,
            name_offset_adjustment,
            link_offset_adjustment,
            stream_offset_adjustment,
            internal_stream_offset_adjustment,
            &extension_id_map,
        );

        self.merge_fragment_links(fragment.links, name_offset_adjustment, &extension_id_map);
        self.merge_fragment_streams(fragment.streams, name_offset_adjustment, &extension_id_map);
        self.merge_fragment_internal_streams(fragment.internal_streams);
        self.merge_fragment_children(fragment.children);
        self.apply_deferred_name_merges(
            records_to_merge,
            link_offset_adjustment,
            stream_offset_adjustment,
        );
    }

    /// Build extension ID remapping table from fragment to merged index.
    fn build_extension_id_map(&mut self, fragment: &MftIndexFragment) -> Vec<u16> {
        let mut extension_id_map: Vec<u16> = Vec::with_capacity(fragment.extensions.len());
        extension_id_map.push(0);

        for idx in 1..fragment.extensions.len() {
            let ext_idx = u16::try_from(idx).unwrap_or(u16::MAX);
            if let Some(ext_str) = fragment.extensions.get_extension(ext_idx) {
                let merged_id = self.extensions.intern(ext_str);
                extension_id_map.push(merged_id);

                let count = fragment.extensions.get_count(ext_idx);
                let bytes = fragment.extensions.get_bytes(ext_idx);
                let merged_idx = merged_id as usize;
                if let Some(count_slot) = self.extensions.counts.get_mut(merged_idx) {
                    *count_slot += count;
                }
                if let Some(bytes_slot) = self.extensions.bytes.get_mut(merged_idx) {
                    *bytes_slot += bytes;
                }
            }
        }

        extension_id_map
    }

    /// Merge records from a fragment into this index, returning records that
    /// need deferred merging.
    fn merge_fragment_records_with_deferred_merge(
        &mut self,
        records: Vec<FileRecord>,
        name_offset_adjustment: u32,
        link_offset_adjustment: u32,
        stream_offset_adjustment: u32,
        internal_stream_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) -> Vec<(u32, FileRecord)> {
        let mut deferred_merges: Vec<(u32, FileRecord)> = Vec::new();

        for mut record in records {
            // `record.frs` is typed `Frs`; the `frs_to_idx` lookup table is
            // `Vec<u32>` indexed by `usize`, so demote via `.raw()` at the
            // indexing boundary only.
            let frs = record.frs;
            let frs_usize = usize::try_from(frs.raw()).unwrap_or(usize::MAX);

            Self::adjust_name_ref(
                &mut record.first_name.name,
                name_offset_adjustment,
                extension_id_map,
            );
            Self::adjust_name_ref(
                &mut record.first_stream.name,
                name_offset_adjustment,
                extension_id_map,
            );

            if record.first_name.next_entry != NO_ENTRY {
                record.first_name.next_entry += link_offset_adjustment;
            }
            if record.first_stream.next_entry != NO_ENTRY {
                record.first_stream.next_entry += stream_offset_adjustment;
            }
            if record.first_internal_stream != NO_ENTRY {
                record.first_internal_stream += internal_stream_offset_adjustment;
            }

            if frs_usize >= self.frs_to_idx.len() {
                self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
            }

            let Some(frs_slot) = self.frs_to_idx.get_mut(frs_usize) else {
                continue;
            };
            let existing_idx = *frs_slot;
            if existing_idx == NO_ENTRY {
                let new_idx = len_to_u32(self.records.len());
                *frs_slot = new_idx;
                self.records.push(record);
            } else {
                self.merge_or_defer_record(existing_idx, record, &mut deferred_merges);
            }
        }

        deferred_merges
    }

    /// Decide whether to replace an existing record or defer its merge.
    fn merge_or_defer_record(
        &mut self,
        existing_idx: u32,
        record: FileRecord,
        deferred_merges: &mut Vec<(u32, FileRecord)>,
    ) {
        let Some(existing) = self.records.get(existing_idx as usize) else {
            return;
        };
        let existing_is_placeholder = !existing.has_base_data();
        let record_has_base = record.has_base_data();
        let should_replace = (existing_is_placeholder && record_has_base)
            || (!existing.has_name() && record.has_name());

        if should_replace {
            let Some(slot) = self.records.get_mut(existing_idx as usize) else {
                return;
            };
            let placeholder = core::mem::replace(slot, record);
            if has_merge_payload(&placeholder) {
                deferred_merges.push((existing_idx, placeholder));
            }
        } else if has_merge_payload(&record) {
            deferred_merges.push((existing_idx, record));
        }
    }

    /// Apply deferred name and stream merges from discarded records.
    #[expect(
        clippy::too_many_lines,
        reason = "three sequential chain-append blocks (links, streams, internal_streams) \
                  with type-specific field access — extracting would require awkward re-borrows \
                  of self.records between calls"
    )]
    fn apply_deferred_name_merges(
        &mut self,
        deferred_merges: Vec<(u32, FileRecord)>,
        _link_offset_adjustment: u32,
        _stream_offset_adjustment: u32,
    ) {
        for (existing_idx, discarded) in deferred_merges {
            let Some(existing) = self.records.get_mut(existing_idx as usize) else {
                continue;
            };

            if discarded.first_name.name.is_valid() || discarded.first_name.next_entry != NO_ENTRY {
                let last_link_idx = (existing.first_name.next_entry != NO_ENTRY).then(|| {
                    let mut idx = existing.first_name.next_entry;
                    while self
                        .links
                        .get(idx as usize)
                        .is_some_and(|link| link.next_entry != NO_ENTRY)
                    {
                        if let Some(link) = self.links.get(idx as usize) {
                            idx = link.next_entry;
                        } else {
                            break;
                        }
                    }
                    idx
                });

                let chain_start = if discarded.first_name.name.is_valid() {
                    let new_link_idx = len_to_u32(self.links.len());
                    self.links.push(LinkInfo {
                        next_entry: discarded.first_name.next_entry,
                        name: discarded.first_name.name,
                        _pad0: [0; 4],
                        parent_frs: discarded.first_name.parent_frs,
                    });
                    Some(new_link_idx)
                } else {
                    (discarded.first_name.next_entry != NO_ENTRY)
                        .then_some(discarded.first_name.next_entry)
                };

                if let Some(start) = chain_start {
                    if let Some(last_idx) = last_link_idx {
                        if let Some(link) = self.links.get_mut(last_idx as usize) {
                            link.next_entry = start;
                        }
                    } else if existing.first_name.name.is_valid() {
                        existing.first_name.next_entry = start;
                    } else {
                        existing.first_name = discarded.first_name;
                    }
                    existing.name_count += discarded.name_count;
                }
            }

            // Re-borrow after the links section (prior mutable borrow was consumed).
            let Some(rec) = self.records.get_mut(existing_idx as usize) else {
                continue;
            };

            if discarded.first_stream.name.is_valid()
                || discarded.first_stream.next_entry != NO_ENTRY
            {
                let last_stream_idx = (rec.first_stream.next_entry != NO_ENTRY).then(|| {
                    let mut idx = rec.first_stream.next_entry;
                    while self
                        .streams
                        .get(idx as usize)
                        .is_some_and(|stream| stream.next_entry != NO_ENTRY)
                    {
                        if let Some(stream) = self.streams.get(idx as usize) {
                            idx = stream.next_entry;
                        } else {
                            break;
                        }
                    }
                    idx
                });

                let chain_start = if discarded.first_stream.name.is_valid() {
                    let new_stream_idx = len_to_u32(self.streams.len());
                    self.streams.push(IndexStreamInfo {
                        size: discarded.first_stream.size,
                        next_entry: discarded.first_stream.next_entry,
                        name: discarded.first_stream.name,
                        flags: discarded.first_stream.flags,
                        _pad0: [0; 3],
                    });
                    Some(new_stream_idx)
                } else {
                    (discarded.first_stream.next_entry != NO_ENTRY)
                        .then_some(discarded.first_stream.next_entry)
                };

                if let Some(start) = chain_start {
                    if let Some(last_idx) = last_stream_idx {
                        if let Some(stream) = self.streams.get_mut(last_idx as usize) {
                            stream.next_entry = start;
                        }
                    } else if rec.first_stream.name.is_valid() {
                        rec.first_stream.next_entry = start;
                    } else {
                        rec.first_stream = discarded.first_stream;
                    }
                    rec.stream_count += discarded.stream_count;
                    rec.total_stream_count += discarded.total_stream_count;
                }
            }

            if discarded.first_internal_stream != NO_ENTRY {
                let last_internal_idx = (rec.first_internal_stream != NO_ENTRY).then(|| {
                    let mut idx = rec.first_internal_stream;
                    while self
                        .internal_streams
                        .get(idx as usize)
                        .is_some_and(|st| st.next_entry != NO_ENTRY)
                    {
                        if let Some(st) = self.internal_streams.get(idx as usize) {
                            idx = st.next_entry;
                        } else {
                            break;
                        }
                    }
                    idx
                });

                let chain_start = discarded.first_internal_stream;

                if let Some(last_idx) = last_internal_idx {
                    if let Some(st) = self.internal_streams.get_mut(last_idx as usize) {
                        st.next_entry = chain_start;
                    }
                } else {
                    rec.first_internal_stream = chain_start;
                }

                rec.internal_streams_size = rec
                    .internal_streams_size
                    .saturating_add(discarded.internal_streams_size);
                rec.internal_streams_allocated = rec
                    .internal_streams_allocated
                    .saturating_add(discarded.internal_streams_allocated);

                let discarded_has_streams = discarded.first_stream.name.is_valid()
                    || discarded.first_stream.next_entry != NO_ENTRY;
                if !discarded_has_streams {
                    let mut count: u16 = 0;
                    let mut idx = chain_start;
                    while let Some(st) = self.internal_streams.get(idx as usize) {
                        count = count.saturating_add(1);
                        idx = st.next_entry;
                    }
                    rec.total_stream_count = rec.total_stream_count.saturating_add(count);
                }
            }
        }
    }

    /// Adjust a name reference with offset and extension ID remapping.
    fn adjust_name_ref(
        name_ref: &mut IndexNameRef,
        offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        if name_ref.is_valid() {
            name_ref.offset += offset_adjustment;
            let old_ext_id = name_ref.extension_id();
            if let Some(&new_ext_id) = extension_id_map.get(old_ext_id as usize) {
                name_ref.remap_extension_id(new_ext_id);
            }
        }
    }

    /// Merge links from a fragment into this index.
    fn merge_fragment_links(
        &mut self,
        links: Vec<LinkInfo>,
        name_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        let link_offset_adjustment = len_to_u32(self.links.len());
        for mut link in links {
            Self::adjust_name_ref(&mut link.name, name_offset_adjustment, extension_id_map);
            if link.next_entry != NO_ENTRY {
                link.next_entry += link_offset_adjustment;
            }
            self.links.push(link);
        }
    }

    /// Merge streams from a fragment into this index.
    fn merge_fragment_streams(
        &mut self,
        streams: Vec<IndexStreamInfo>,
        name_offset_adjustment: u32,
        extension_id_map: &[u16],
    ) {
        let stream_offset_adjustment = len_to_u32(self.streams.len());
        for mut stream in streams {
            Self::adjust_name_ref(&mut stream.name, name_offset_adjustment, extension_id_map);
            if stream.next_entry != NO_ENTRY {
                stream.next_entry += stream_offset_adjustment;
            }
            self.streams.push(stream);
        }
    }

    /// Merge internal streams from a fragment into this index.
    fn merge_fragment_internal_streams(&mut self, internal_streams: Vec<InternalStreamInfo>) {
        let internal_offset_adjustment = len_to_u32(self.internal_streams.len());
        for mut st in internal_streams {
            if st.next_entry != NO_ENTRY {
                st.next_entry += internal_offset_adjustment;
            }
            self.internal_streams.push(st);
        }
    }

    /// Merge children from a fragment into this index.
    fn merge_fragment_children(&mut self, children: Vec<ChildInfo>) {
        let child_offset_adjustment = len_to_u32(self.children.len());
        for mut child in children {
            if child.next_entry != NO_ENTRY {
                child.next_entry += child_offset_adjustment;
            }
            self.children.push(child);
        }
    }
}
