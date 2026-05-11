// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Legacy direct-to-fragment parser bridge.
//! Preserves the parallel parser surface used before fragment merge.

use core::mem::size_of;

use smallvec::SmallVec;
use zerocopy::FromBytes as _;

#[expect(
    deprecated,
    reason = "internal use in deprecated parse_record_to_fragment"
)]
use super::fragment_extension::parse_extension_to_fragment;
use crate::index::{
    ChildInfo, IndexNameRef, IndexStreamInfo, LinkInfo, NO_ENTRY, SizeInfo, StandardInfo,
    frs_to_usize, len_to_u16, len_to_u32, u32_as_usize,
};
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    StandardInformation, file_reference_to_frs,
};

/// Parses a record directly into an `MftIndexFragment` (for parallel parsing).
///
/// This is the parallel-parsing variant of `parse_record_to_index`. Each worker
/// thread builds its own fragment, which is later merged into the final index.
///
/// # Returns
///
/// `true` if a record was added to the fragment, `false` if skipped.
#[deprecated(note = "Use parse_record_full() + MftRecordMerger + from_parsed_records() instead")]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "monolithic parser kept for performance-critical hot path"
)]
#[expect(
    clippy::indexing_slicing,
    clippy::missing_asserts_for_indexing,
    reason = "all slice access is bounds-guarded by while-loop condition and per-access length checks"
)]
#[expect(deprecated, reason = "deprecated function calling deprecated helper")]
pub fn parse_record_to_fragment(
    data: &[u8],
    frs: u64,
    fragment: &mut crate::index::MftIndexFragment,
) -> bool {
    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
        return false;
    };

    // Check if record is in use
    if !header.is_in_use() {
        return false;
    }

    // Check magic
    let multi_sector_header = header.multi_sector_header;
    if !multi_sector_header.is_file_record() {
        return false;
    }

    // Handle extension records: add their names/streams to the base record
    if !header.is_base_record() {
        let base_frs = file_reference_to_frs(header.base_file_record_segment);
        return parse_extension_to_fragment(data, base_frs, fragment);
    }

    let is_directory = header.is_directory();

    // Parse attributes
    let mut offset = usize::from(header.first_attribute_offset);
    let max_offset = core::cmp::min(u32_as_usize(header.bytes_in_use), data.len());

    // Temporary storage for parsed data
    let mut std_info = StandardInfo::default();
    let mut primary_name: Option<(String, u64, u8, u16)> = None; // (name, parent_frs, namespace, parse_index)
    let mut additional_names: SmallVec<[(String, u64, u16); 4]> = SmallVec::new();
    let mut name_parse_counter: u16 = 0;
    let mut default_size = 0_u64;
    let mut default_allocated = 0_u64;
    // ADS: (stream_name, size, allocated)
    let mut additional_streams: SmallVec<[(String, u64, u64); 4]> = SmallVec::new();

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let Ok((attr_header, _)) = AttributeRecordHeader::read_from_prefix(&data[offset..]) else {
            break;
        };

        if attr_header.type_code == AttributeType::END_MARKER {
            break;
        }

        if attr_header.length == 0 || offset + u32_as_usize(attr_header.length) > max_offset {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) if attr_header.is_non_resident == 0 => {
                let value_offset_bytes = &data[offset + 20..offset + 22];
                let value_offset =
                    u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;
                let si_offset = offset + value_offset;
                if si_offset + size_of::<StandardInformation>() <= data.len() {
                    let Ok((si, _)) = StandardInformation::read_from_prefix(&data[si_offset..])
                    else {
                        break;
                    };
                    let ext =
                        crate::ntfs::ExtendedStandardInfo::from_attributes(si.file_attributes);
                    let mut info = StandardInfo::from_extended(&ext);
                    info.created = si.creation_time;
                    info.modified = si.modification_time;
                    info.accessed = si.access_time;
                    info.mft_changed = si.mft_change_time;
                    std_info = info;
                }
            }
            Some(AttributeType::FileName) if attr_header.is_non_resident == 0 => {
                let value_offset_bytes = &data[offset + 20..offset + 22];
                let value_offset =
                    u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;
                let fn_offset = offset + value_offset;
                if fn_offset + size_of::<FileNameAttribute>() <= data.len() {
                    let Ok((fn_attr, _)) = FileNameAttribute::read_from_prefix(&data[fn_offset..])
                    else {
                        break;
                    };
                    let name_len = usize::from(fn_attr.file_name_length);
                    let name_bytes_offset = fn_offset + size_of::<FileNameAttribute>();
                    if name_bytes_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_bytes_offset..name_bytes_offset + name_len * 2];
                        let name_u16: Vec<u16> = name_bytes
                            .chunks_exact(2)
                            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                            .collect();
                        let name = String::from_utf16_lossy(&name_u16);
                        let parent_frs = file_reference_to_frs(fn_attr.parent_directory);
                        let namespace = fn_attr.file_name_namespace;

                        if namespace != 2 {
                            let parse_idx = name_parse_counter;
                            name_parse_counter += 1;
                            let is_better = match namespace {
                                1 | 3 => true,
                                0 => primary_name.is_none(),
                                _ => false,
                            };
                            if is_better || primary_name.is_none() {
                                if let Some((old_name, old_parent, _, old_parse_idx)) =
                                    primary_name.take()
                                {
                                    additional_names.push((old_name, old_parent, old_parse_idx));
                                }
                                primary_name = Some((name, parent_frs, namespace, parse_idx));
                            } else {
                                additional_names.push((name, parent_frs, parse_idx));
                            }
                        }
                    }
                }
            }
            Some(AttributeType::Data) => {
                // legacy-output parity: Only primary attributes (LowestVCN == 0) count as
                // streams. Continuation extents (LowestVCN > 0) are skipped.
                // See ntfs_index_load.hpp:358
                let is_primary = if attr_header.is_non_resident == 0 {
                    true // Resident attributes are always primary
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 8 <= data.len() {
                        let lowest_vcn = i64::from_le_bytes(
                            data[nr_offset..nr_offset + 8].try_into().unwrap_or([0; 8]),
                        );
                        lowest_vcn == 0
                    } else {
                        false // Can't verify, skip to be safe
                    }
                };

                if !is_primary {
                    // Skip continuation extents - they don't count as new streams
                    offset += u32_as_usize(attr_header.length);
                    continue;
                }

                // Parse $DATA - track both default stream and ADS
                let name_len = usize::from(attr_header.name_length);
                let (size, allocated) = if attr_header.is_non_resident != 0 {
                    let alloc_offset = offset + 40;
                    let size_offset = offset + 48;
                    if size_offset + 8 <= data.len() {
                        let allocated = u64::from_le_bytes(
                            data[alloc_offset..alloc_offset + 8]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let size = u64::from_le_bytes(
                            data[size_offset..size_offset + 8]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (size, allocated)
                    } else {
                        (0, 0)
                    }
                } else {
                    // Resident: value_length at offset 16
                    // Resident files have no clusters allocated - data is stored in MFT record
                    // Resident files have allocated_size=0 (data stored in MFT record)
                    let len_offset = offset + 16;
                    if len_offset + 4 <= data.len() {
                        let len = u64::from(u32::from_le_bytes(
                            data[len_offset..len_offset + 4]
                                .try_into()
                                .unwrap_or([0; 4]),
                        ));
                        (len, 0) // allocated_size = 0 for resident files
                    } else {
                        (0, 0)
                    }
                };

                if name_len == 0 {
                    // Default stream
                    default_size = size;
                    default_allocated = allocated;
                } else {
                    // Alternate Data Stream (ADS)
                    let name_offset = offset + usize::from(attr_header.name_offset);
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let name_u16: SmallVec<[u16; 64]> = name_bytes
                            .chunks_exact(2)
                            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                            .collect();
                        let stream_name = String::from_utf16_lossy(&name_u16);
                        // ALL named $DATA streams create regular
                        // stream entries.  Internal ones are filtered from
                        // output by is_internal_windows_stream in the output layer.
                        additional_streams.push((stream_name, size, allocated));
                    }
                }
            }
            _ => {}
        }

        offset += u32_as_usize(attr_header.length);
    }

    // Set directory flag in std_info BEFORE checking for filename
    // This ensures is_directory is set even when $FILE_NAME is in extension record
    if is_directory {
        std_info.set_directory(true);
    }

    // Handle records without a filename in the base record
    // The $FILE_NAME may be in an extension record - we still need to store stdinfo
    let Some((name, parent_frs, _namespace, primary_parse_index)) = primary_name else {
        // No $FILE_NAME in base record — store stdinfo anyway.
        // The extension record will add the name later.
        //
        // IMPORTANT: We must still add ADS streams from the base record!
        // The $FILE_NAME may be in an extension record, but the ADS are here.
        // Without this, ADS on files/directories with extension records are lost.
        store_nameless_record(
            fragment,
            frs,
            std_info,
            default_size,
            default_allocated,
            additional_streams,
        );
        return false;
    };

    // Add primary name to names buffer and get reference
    let name_offset = fragment.add_name(&name);
    let name_len = name.len();
    let is_ascii = name.is_ascii();
    let extension_id = fragment.intern_extension(&name);
    let name_ref = IndexNameRef::new(name_offset, len_to_u16(name_len), is_ascii, extension_id);

    // Pre-process additional names
    let additional_count = additional_names.len();
    let mut link_indices: Vec<u32> = Vec::with_capacity(additional_count);
    // Collect parent FRS values for building children array later
    let mut additional_parent_frs: SmallVec<[(u64, u16); 4]> =
        SmallVec::with_capacity(additional_count);
    for (link_name, link_parent, link_parse_idx) in additional_names {
        additional_parent_frs.push((link_parent, link_parse_idx));
        let link_offset = fragment.add_name(&link_name);
        let link_len = link_name.len();
        let link_is_ascii = link_name.is_ascii();
        let link_ext_id = fragment.intern_extension(&link_name);
        let link_name_ref = IndexNameRef::new(
            link_offset,
            len_to_u16(link_len),
            link_is_ascii,
            link_ext_id,
        );

        let link_idx = len_to_u32(fragment.links.len());
        fragment.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: link_name_ref,
            _pad0: [0; 4],
            parent_frs: link_parent,
        });
        link_indices.push(link_idx);
    }

    // Pre-process additional streams (ADS)
    let additional_stream_count = additional_streams.len();
    let mut stream_indices: Vec<u32> = Vec::with_capacity(additional_stream_count);
    for (stream_name, stream_size, stream_allocated) in additional_streams {
        let stream_name_offset = fragment.add_name(&stream_name);
        let stream_name_len = stream_name.len();
        let stream_is_ascii = stream_name.is_ascii();
        let stream_ext_id = fragment.intern_extension(&stream_name);
        let stream_name_ref = IndexNameRef::new(
            stream_name_offset,
            len_to_u16(stream_name_len),
            stream_is_ascii,
            stream_ext_id,
        );

        let stream_idx = len_to_u32(fragment.streams.len());
        fragment.streams.push(IndexStreamInfo {
            size: SizeInfo {
                length: stream_size,
                allocated: stream_allocated,
            },
            next_entry: NO_ENTRY,
            name: stream_name_ref,
            flags: 8_u8 << 2_u8,
            _pad0: [0; 3],
        });
        stream_indices.push(stream_idx);
    }

    // Create parent placeholder if needed (within this fragment)
    if parent_frs != frs && parent_frs != 0 {
        fragment.get_or_create(parent_frs);
        // ^ side effect only: ensures parent placeholder exists
    }

    // Get or create the record in the fragment
    // IMPORTANT: The record may already have names/streams from extension records
    // that were processed BEFORE this base record in the same fragment.
    // We must preserve those extension names/streams and chain them to base record
    // data.
    let record = fragment.get_or_create(frs);

    // Save any existing extension data BEFORE overwriting
    // Copy the entire first_name LinkInfo so we can add it as a link later
    let existing_first_name = record.first_name;
    let existing_name_valid = existing_first_name.name.is_valid();
    let existing_name_count = if existing_name_valid {
        record.name_count
    } else if existing_first_name.next_entry != NO_ENTRY {
        // Has overflow links but no first_name - count those
        record.name_count.saturating_sub(1)
    } else {
        0
    };
    let existing_stream_next = record.first_stream.next_entry;
    let existing_stream_count = if existing_stream_next == NO_ENTRY {
        0
    } else {
        // Extension records added ADS - count is stream_count - 1 (exclude default)
        record.stream_count.saturating_sub(1)
    };

    // Now set the base record data
    record.stdinfo = std_info;
    record.first_stream.size = SizeInfo {
        length: default_size,
        allocated: default_allocated,
    };
    record.first_name = LinkInfo {
        next_entry: NO_ENTRY,
        name: name_ref,
        _pad0: [0; 4],
        parent_frs,
    };

    // Chain the base record's additional links together
    for i in 0..link_indices.len().saturating_sub(1) {
        let current_idx = u32_as_usize(link_indices[i]);
        let next_idx = link_indices[i + 1];
        fragment.links[current_idx].next_entry = next_idx;
    }

    // Chain the base record's additional streams together
    for i in 0..stream_indices.len().saturating_sub(1) {
        let current_idx = u32_as_usize(stream_indices[i]);
        let next_idx = stream_indices[i + 1];
        fragment.streams[current_idx].next_entry = next_idx;
    }

    // Now chain base record links, then extension links
    // Extension names become additional hard links after base record's names
    //
    // We need to update fragment.links BEFORE borrowing record to avoid borrow
    // conflicts. Calculate what the first_name.next_entry should be, then set
    // it after.
    let first_name_next_entry: u32;

    if existing_name_valid {
        // Extension had first_name set - add it as a new link in the links array
        let ext_link_idx = len_to_u32(fragment.links.len());
        fragment.links.push(existing_first_name);

        // Chain: base first_name -> base additional links -> ext first_name -> ext
        // overflow
        if link_indices.is_empty() {
            first_name_next_entry = ext_link_idx;
        } else {
            first_name_next_entry = link_indices[0];
            let last_base_link = u32_as_usize(link_indices[link_indices.len() - 1]);
            fragment.links[last_base_link].next_entry = ext_link_idx;
        }
    } else if existing_first_name.next_entry != NO_ENTRY {
        // Extension only had overflow links (no first_name) - chain them
        if link_indices.is_empty() {
            first_name_next_entry = existing_first_name.next_entry;
        } else {
            first_name_next_entry = link_indices[0];
            let last_base_link = u32_as_usize(link_indices[link_indices.len() - 1]);
            fragment.links[last_base_link].next_entry = existing_first_name.next_entry;
        }
    } else {
        // No extension names - just chain base's additional links
        if link_indices.is_empty() {
            first_name_next_entry = NO_ENTRY;
        } else {
            first_name_next_entry = link_indices[0];
        }
    }

    // Now set first_name.next_entry on the record.
    // (Separate borrow scope because we mutate fragment.links/streams above
    // and need a fresh &mut record.)
    let rec_for_name = fragment.get_or_create(frs);
    rec_for_name.first_name.next_entry = first_name_next_entry;

    // Chain streams: base ADS -> extension ADS (must be done before borrowing
    // record) If base has ADS and extension has ADS, chain them together
    if !stream_indices.is_empty() && existing_stream_next != NO_ENTRY {
        let last_base_stream = u32_as_usize(stream_indices[stream_indices.len() - 1]);
        fragment.streams[last_base_stream].next_entry = existing_stream_next;
    }

    // Now get record and update counts and first_stream chain
    let rec_for_counts = fragment.get_or_create(frs);

    // Calculate total name count
    // Base: 1 (first_name) + additional_count
    // Extension: existing_name_count (includes extension's names)
    rec_for_counts.name_count = 1 + len_to_u16(additional_count) + existing_name_count;

    // Set first_stream.next_entry to chain to base ADS or extension ADS
    if !stream_indices.is_empty() {
        rec_for_counts.first_stream.next_entry = stream_indices[0];
    } else if existing_stream_next != NO_ENTRY {
        // Base has no ADS, but extension had ADS
        rec_for_counts.first_stream.next_entry = existing_stream_next;
    }

    // Calculate total stream count
    // Base: 1 (default $DATA) + additional_stream_count
    // Extension: existing_stream_count (ADS from extension records)
    rec_for_counts.stream_count = 1 + len_to_u16(additional_stream_count) + existing_stream_count;

    // Build parent-child relationship for tree metrics computation
    // This is critical for compute_tree_metrics() to work correctly.
    // Each name (primary + additional) creates a child entry in its parent.
    // name_index 0 = primary name, 1+ = additional names (hardlinks)

    // Helper to add a child entry to a parent in the fragment
    let add_child_entry = |frag: &mut crate::index::MftIndexFragment, p_frs: u64, name_idx: u16| {
        if p_frs == frs || p_frs == u64::from(NO_ENTRY) {
            return;
        }
        // Ensure parent exists in fragment
        let parent_idx = {
            let p_frs_usize = frs_to_usize(p_frs);
            if p_frs_usize >= frag.frs_to_idx.len() {
                frag.frs_to_idx.resize(p_frs_usize + 1, NO_ENTRY);
            }
            if frag.frs_to_idx[p_frs_usize] == NO_ENTRY {
                // Create placeholder parent
                let new_idx = len_to_u32(frag.records.len());
                frag.frs_to_idx[p_frs_usize] = new_idx;
                frag.records.push(crate::index::FileRecord::new(p_frs));
            }
            frag.frs_to_idx[p_frs_usize]
        };

        // Add child entry
        let child_idx = len_to_u32(frag.children.len());
        let parent = &mut frag.records[u32_as_usize(parent_idx)];
        let old_first_child = parent.first_child;
        parent.first_child = child_idx;

        frag.children.push(ChildInfo {
            next_entry: old_first_child,
            _pad0: [0; 4],
            child_frs: frs,
            name_index: name_idx,
            _pad1: [0; 6],
        });
    };

    // Add child entry for primary name (using MFT parse-order index)
    add_child_entry(fragment, parent_frs, primary_parse_index);

    // Add child entries for additional names (hardlinks)
    for &(link_parent_frs, link_parse_idx) in &additional_parent_frs {
        add_child_entry(fragment, link_parent_frs, link_parse_idx);
    }

    true
}

/// Handle a base record that has no `$FILE_NAME` attribute (name comes from
/// an extension record). Stores stdinfo and ADS streams so they are not lost.
#[expect(
    clippy::indexing_slicing,
    reason = "stream_indices elements are valid indices into fragment.streams (just pushed)"
)]
fn store_nameless_record(
    fragment: &mut crate::index::MftIndexFragment,
    frs: u64,
    std_info: StandardInfo,
    default_size: u64,
    default_allocated: u64,
    additional_streams: SmallVec<[(String, u64, u64); 4]>,
) {
    // Pre-process ADS streams BEFORE creating the record
    let additional_stream_count = additional_streams.len();
    let mut stream_indices: Vec<u32> = Vec::with_capacity(additional_stream_count);
    for (stream_name, stream_size, stream_allocated) in additional_streams {
        let stream_name_offset = fragment.add_name(&stream_name);
        let stream_name_len = stream_name.len();
        let stream_is_ascii = stream_name.is_ascii();
        let extension_id = fragment.intern_extension(&stream_name);
        let stream_name_ref = IndexNameRef::new(
            stream_name_offset,
            len_to_u16(stream_name_len),
            stream_is_ascii,
            extension_id,
        );

        let stream_idx = len_to_u32(fragment.streams.len());
        fragment.streams.push(IndexStreamInfo {
            size: SizeInfo {
                length: stream_size,
                allocated: stream_allocated,
            },
            next_entry: NO_ENTRY,
            name: stream_name_ref,
            flags: 8_u8 << 2_u8,
            _pad0: [0; 3],
        });
        stream_indices.push(stream_idx);
    }

    // Now create the record and set up streams
    let record = fragment.get_or_create(frs);
    record.stdinfo = std_info;
    record.first_stream.size = SizeInfo {
        length: default_size,
        allocated: default_allocated,
    };

    // Chain ADS streams to first_stream
    if !stream_indices.is_empty() {
        for i in 0..stream_indices.len().saturating_sub(1) {
            let current_idx = u32_as_usize(stream_indices[i]);
            let next_idx = stream_indices[i + 1];
            fragment.streams[current_idx].next_entry = next_idx;
        }
        let rec_for_stream = fragment.get_or_create(frs);
        rec_for_stream.first_stream.next_entry = stream_indices[0];
        rec_for_stream.stream_count = 1 + len_to_u16(additional_stream_count);
    }
}
