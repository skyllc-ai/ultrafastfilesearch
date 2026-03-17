//! Legacy extension-record helper for direct-to-fragment parsing.
//! Extracts names and streams from extension records into a fragment.

// Performance-critical hot-path parser — lint suppressions match index.rs.
#![allow(clippy::all, clippy::nursery, clippy::pedantic)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use core::mem::size_of;

use smallvec::SmallVec;
use zerocopy::FromBytes;

use crate::ntfs::is_internal_windows_stream;

/// Parses an extension record and adds its names/streams to the base record in
/// a fragment.
///
/// This is the parallel-parsing variant of `parse_extension_to_index`.
///
/// # Arguments
///
/// * `data` - The raw extension record data (after fixup)
/// * `base_frs` - The FRS of the base record this extension belongs to
/// * `fragment` - The MFT index fragment to update
///
/// # Returns
///
/// `true` if any names/streams were added, `false` otherwise.
#[deprecated(note = "Use parse_record_full() + MftRecordMerger instead")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "NTFS field sizes are bounded by u16/u32 record layout"
)]
pub(super) fn parse_extension_to_fragment(
    data: &[u8],
    base_frs: u64,
    fragment: &mut crate::index::MftIndexFragment,
) -> bool {
    use crate::index::{ChildInfo, IndexNameRef, IndexStreamInfo, LinkInfo, NO_ENTRY, SizeInfo};
    use crate::ntfs::{
        AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    };

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let header = match FileRecordSegmentHeader::read_from_prefix(data) {
        Ok((header, _)) => header,
        Err(_) => return false,
    };

    // Parse attributes to find $FILE_NAME and $DATA
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    // Collect names and streams from extension record
    let mut names: SmallVec<[(String, u64); 4]> = SmallVec::new();
    let mut streams: SmallVec<[(String, u64, u64); 4]> = SmallVec::new();

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let attr_header = match AttributeRecordHeader::read_from_prefix(&data[offset..]) {
            Ok((attr_header, _)) => attr_header,
            Err(_) => break,
        };

        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }

        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    let value_offset_bytes = &data[offset + 20..offset + 22];
                    let value_offset =
                        u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0]))
                            as usize;
                    let fn_offset = offset + value_offset;
                    if fn_offset + size_of::<FileNameAttribute>() <= data.len() {
                        let fn_attr = match FileNameAttribute::read_from_prefix(&data[fn_offset..])
                        {
                            Ok((fn_attr, _)) => fn_attr,
                            Err(_) => break,
                        };

                        if fn_attr.file_name_namespace != 2 {
                            let name_len = fn_attr.file_name_length as usize;
                            let name_start = fn_offset + size_of::<FileNameAttribute>();
                            if name_start + name_len * 2 <= data.len() {
                                let name_bytes = &data[name_start..name_start + name_len * 2];
                                let name_u16: SmallVec<[u16; 64]> = name_bytes
                                    .chunks_exact(2)
                                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                    .collect();
                                let name = String::from_utf16_lossy(&name_u16);
                                let parent_frs = fn_attr.parent_directory & 0x0000_FFFF_FFFF_FFFF;
                                names.push((name, parent_frs));
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
                    offset += attr_header.length as usize;
                    continue;
                }

                let name_len = attr_header.name_length as usize;
                if name_len > 0 {
                    let (size, allocated) = if attr_header.is_non_resident != 0 {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let allocated = i64::from_le_bytes(
                                data[nr_offset + 24..nr_offset + 32]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            let size = i64::from_le_bytes(
                                data[nr_offset + 32..nr_offset + 40]
                                    .try_into()
                                    .unwrap_or([0; 8]),
                            );
                            (size.max(0) as u64, allocated.max(0) as u64)
                        } else {
                            (0, 0)
                        }
                    } else {
                        let len_offset = offset + 16;
                        if len_offset + 4 <= data.len() {
                            let len = u32::from_le_bytes(
                                data[len_offset..len_offset + 4]
                                    .try_into()
                                    .unwrap_or([0; 4]),
                            ) as u64;
                            (len, 0)
                        } else {
                            (0, 0)
                        }
                    };

                    let name_offset = offset + attr_header.name_offset as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let name_u16: SmallVec<[u16; 64]> = name_bytes
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        let stream_name = String::from_utf16_lossy(&name_u16);
                        // Filter out internal Windows streams (names starting with $)
                        if !is_internal_windows_stream(&stream_name) {
                            streams.push((stream_name, size, allocated));
                        }
                    }
                }
            }
            _ => {}
        }

        offset += attr_header.length as usize;
    }

    if names.is_empty() && streams.is_empty() {
        return false;
    }

    // Add names to the fragment
    let mut link_indices: Vec<u32> = Vec::with_capacity(names.len());
    for (name, parent_frs) in &names {
        let name_offset = fragment.names.len() as u32;
        fragment.names.push_str(name);
        let name_len = name.len();
        let is_ascii = name.is_ascii();
        let extension_id = fragment.intern_extension(name);
        let name_ref = IndexNameRef::new(name_offset, name_len as u16, is_ascii, extension_id);

        let link_idx = fragment.links.len() as u32;
        fragment.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: name_ref,
            parent_frs: *parent_frs,
        });
        link_indices.push(link_idx);
    }

    // Add streams to the fragment
    let mut stream_indices: Vec<u32> = Vec::with_capacity(streams.len());
    for (stream_name, size, allocated) in &streams {
        let name_offset = fragment.names.len() as u32;
        fragment.names.push_str(stream_name);
        let name_len = stream_name.len();
        let is_ascii = stream_name.is_ascii();
        let extension_id = fragment.intern_extension(stream_name);
        let name_ref = IndexNameRef::new(name_offset, name_len as u16, is_ascii, extension_id);

        let stream_idx = fragment.streams.len() as u32;
        fragment.streams.push(IndexStreamInfo {
            size: SizeInfo {
                length: *size,
                allocated: *allocated,
            },
            next_entry: NO_ENTRY,
            name: name_ref,
            // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
            flags: 8 << 2,
        });
        stream_indices.push(stream_idx);
    }

    // Ensure parent directories exist
    for (_, parent_frs) in &names {
        if *parent_frs != base_frs && *parent_frs != 0 {
            let _ = fragment.get_or_create(*parent_frs);
        }
    }

    // Chain new links together first (before getting record reference)
    if !link_indices.is_empty() {
        for i in 0..link_indices.len().saturating_sub(1) {
            let current_idx = link_indices[i] as usize;
            let next_idx = link_indices[i + 1];
            fragment.links[current_idx].next_entry = next_idx;
        }
    }

    // Chain new streams together first (before getting record reference)
    if !stream_indices.is_empty() {
        for i in 0..stream_indices.len().saturating_sub(1) {
            let current_idx = stream_indices[i] as usize;
            let next_idx = stream_indices[i + 1];
            fragment.streams[current_idx].next_entry = next_idx;
        }
    }

    // Get the first_name.next_entry, first_stream.next_entry, and first_name
    // validity before we start modifying things
    let record = fragment.get_or_create(base_frs);
    let first_name_valid = record.first_name.name.is_valid();
    let first_name_next = record.first_name.next_entry;
    let first_stream_next = record.first_stream.next_entry;

    // Find the end of the current link chain
    let link_chain_end = if first_name_next != NO_ENTRY {
        let mut idx = first_name_next;
        while fragment.links[idx as usize].next_entry != NO_ENTRY {
            idx = fragment.links[idx as usize].next_entry;
        }
        Some(idx)
    } else {
        None
    };

    // Find the end of the current stream chain
    let stream_chain_end = if first_stream_next != NO_ENTRY {
        let mut idx = first_stream_next;
        while fragment.streams[idx as usize].next_entry != NO_ENTRY {
            idx = fragment.streams[idx as usize].next_entry;
        }
        Some(idx)
    } else {
        None
    };

    // Now attach the new links
    if !link_indices.is_empty() {
        // Check if base record has no name (first_name is empty)
        // This happens when the $FILE_NAME attribute is ONLY in extension records
        if !first_name_valid {
            // Copy the first extension name directly into first_name
            // This matches established behavior (ntfs_index.hpp lines 559-567)
            // Copy values first to avoid borrow conflict
            let first_link_name = fragment.links[link_indices[0] as usize].name;
            let first_link_parent = fragment.links[link_indices[0] as usize].parent_frs;
            let record = fragment.get_or_create(base_frs);
            record.first_name.name = first_link_name;
            record.first_name.parent_frs = first_link_parent;
            // Don't increment name_count for the first name (it's already counted as 1)

            // Chain remaining links (if any) to first_name.next_entry
            if link_indices.len() > 1 {
                let record = fragment.get_or_create(base_frs);
                record.first_name.next_entry = link_indices[1];
                // Update name count for additional links only
                record.name_count += (link_indices.len() - 1) as u16;
            }
        } else {
            // Base record already has a name - chain extension names as additional hard
            // links
            if let Some(end_idx) = link_chain_end {
                fragment.links[end_idx as usize].next_entry = link_indices[0];
            } else {
                let record = fragment.get_or_create(base_frs);
                record.first_name.next_entry = link_indices[0];
            }
            let record = fragment.get_or_create(base_frs);
            record.name_count += link_indices.len() as u16;
        }
    }

    // Now attach the new streams
    if !stream_indices.is_empty() {
        if let Some(end_idx) = stream_chain_end {
            fragment.streams[end_idx as usize].next_entry = stream_indices[0];
        } else {
            let record = fragment.get_or_create(base_frs);
            record.first_stream.next_entry = stream_indices[0];
        }
        let record = fragment.get_or_create(base_frs);
        record.stream_count += stream_indices.len() as u16;
        record.total_stream_count += stream_indices.len() as u16;
    }

    // Build parent-child relationship for names added from extension records
    // This is critical for compute_tree_metrics() to work correctly.
    // Get the current name_count to determine the name_index for each new name
    let record = fragment.get_or_create(base_frs);
    let existing_name_count = record.name_count;

    for (name_idx, (_, parent_frs)) in names.iter().enumerate() {
        let p_frs = *parent_frs;
        if p_frs == base_frs || p_frs == 0 || p_frs == u64::from(NO_ENTRY) {
            continue;
        }

        // Ensure parent exists in fragment
        let parent_idx = {
            let p_frs_usize = p_frs as usize;
            if p_frs_usize >= fragment.frs_to_idx.len() {
                fragment.frs_to_idx.resize(p_frs_usize + 1, NO_ENTRY);
            }
            if fragment.frs_to_idx[p_frs_usize] == NO_ENTRY {
                // Create placeholder parent
                let new_idx = fragment.records.len() as u32;
                fragment.frs_to_idx[p_frs_usize] = new_idx;
                fragment.records.push(crate::index::FileRecord::new(p_frs));
            }
            fragment.frs_to_idx[p_frs_usize]
        };

        // Add child entry
        // name_index is the position in the combined name list (existing + new)
        // For extension records, the first name might replace first_name (if empty),
        // so we need to account for that
        let effective_name_idx = if existing_name_count == 0 {
            // First extension name became first_name, so name_index starts at 0
            name_idx as u16
        } else {
            // Extension names are appended after existing names
            existing_name_count - 1 + name_idx as u16
        };

        let child_idx = fragment.children.len() as u32;
        let parent = &mut fragment.records[parent_idx as usize];
        let old_first_child = parent.first_child;
        parent.first_child = child_idx;

        fragment.children.push(ChildInfo {
            next_entry: old_first_child,
            child_frs: base_frs,
            name_index: effective_name_idx,
        });
    }

    !names.is_empty() || !streams.is_empty()
}
