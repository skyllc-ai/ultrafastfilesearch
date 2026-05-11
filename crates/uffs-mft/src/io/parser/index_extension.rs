// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Extension record parser for direct-to-index path.
//!
//! Exception: This file is intentionally large (720+ LOC) to match the
//! completeness of `index.rs` - it handles all the same attribute types that
//! can appear in extension records. See `scripts/ci/file_size_exceptions.txt`.
//!
//! This module handles extension records for the single-pass parser, extracting
//! names, streams, and all attribute types from extension records and merging
//! them into base records in the index.

use core::mem::size_of;

use smallvec::SmallVec;
use zerocopy::FromBytes as _;

use crate::index::{frs_to_usize, len_to_u16, len_to_u32, u32_as_usize};

/// Parses an extension record and adds its names/streams to the base record.
///
/// Extension records contain additional `$FILE_NAME` attributes (hard links)
/// and additional attributes (ADS, system attributes, etc.) that don't fit
/// in the base record. This function extracts those attributes and adds them
/// to the base record in the index.
///
/// Handles ALL attribute types that `parse_record_full()` handles, including:
/// - `$FILE_NAME` (hard links)
/// - `$DATA` (ADS)
/// - `$REPARSE_POINT`, `$INDEX_ROOT`, `$INDEX_ALLOCATION`, `$BITMAP`
/// - `$OBJECT_ID`, `$EA`, `$LOGGED_UTILITY_STREAM`, etc.
/// - Unknown attribute types
///
/// # Arguments
///
/// * `data` - The raw extension record data (after fixup)
/// * `base_frs` - The FRS of the base record this extension belongs to
/// * `index` - The MFT index to update
///
/// # Returns
///
/// `true` if any names/streams were added, `false` otherwise.
#[expect(
    clippy::cognitive_complexity,
    reason = "NTFS attribute dispatch is inherently complex"
)]
#[expect(
    clippy::too_many_lines,
    reason = "monolithic extension parser for performance"
)]
#[expect(
    clippy::indexing_slicing,
    clippy::missing_asserts_for_indexing,
    reason = "all slice access is bounds-guarded: while-loop checks offset + HEADER_SIZE <= max_offset, \
              and each attribute access validates offset + field_len <= data.len() before indexing"
)]
pub(super) fn parse_extension_to_index(
    data: &[u8],
    base_frs: u64,
    index: &mut crate::index::MftIndex,
) -> bool {
    use crate::index::{ChildInfo, IndexNameRef, IndexStreamInfo, LinkInfo, NO_ENTRY, SizeInfo};
    use crate::ntfs::{
        AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    };

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
        return false;
    };

    // Parse attributes to find $FILE_NAME and $DATA
    let mut offset = usize::from(header.first_attribute_offset);
    let max_offset = core::cmp::min(u32_as_usize(header.bytes_in_use), data.len());

    // Collect names and streams from extension record
    let mut names: SmallVec<[(String, u64); 4]> = SmallVec::new();
    // User-visible ADS only
    let mut streams: SmallVec<[(String, u64, u64); 4]> = SmallVec::new();
    // Internal NTFS streams (for tree-metrics accounting)
    let mut ext_internal_streams: SmallVec<[(u64, u64); 4]> = SmallVec::new();
    let mut dir_index_size: u64 = 0;
    let mut dir_index_allocated: u64 = 0;
    // Default $DATA stream (unnamed, name_len == 0) found in extension record
    let mut default_data_size: u64 = 0;
    let mut default_data_allocated: u64 = 0;
    let mut found_default_data = false;

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

        let attr_type = AttributeType::from_u32(attr_header.type_code);
        match attr_type {
            Some(AttributeType::FileName) => {
                // Parse $FILE_NAME attribute
                if attr_header.is_non_resident == 0 {
                    let value_offset_bytes = &data[offset + 20..offset + 22];
                    let value_offset = usize::from(u16::from_le_bytes(
                        value_offset_bytes.try_into().unwrap_or([0, 0]),
                    ));
                    let fn_offset = offset + value_offset;
                    if fn_offset + size_of::<FileNameAttribute>() <= data.len() {
                        let Ok((fn_attr, _)) =
                            FileNameAttribute::read_from_prefix(&data[fn_offset..])
                        else {
                            break;
                        };

                        // Skip DOS-only names (namespace 2)
                        if fn_attr.file_name_namespace != 2 {
                            let name_len = usize::from(fn_attr.file_name_length);
                            let name_start = fn_offset + size_of::<FileNameAttribute>();
                            if name_start + name_len * 2 <= data.len() {
                                let name_bytes = &data[name_start..name_start + name_len * 2];
                                let name_u16: SmallVec<[u16; 64]> = name_bytes
                                    .chunks_exact(2)
                                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
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
                    offset += u32_as_usize(attr_header.length);
                    continue;
                }

                // Parse $DATA attribute — default stream (unnamed) or ADS (named)
                let name_len = usize::from(attr_header.name_length);
                let (size, allocated) = if attr_header.is_non_resident != 0 {
                    let nr_offset = offset + 16;
                    if nr_offset + 48 <= data.len() {
                        // Check if compressed or sparse
                        let is_compressed_or_sparse = (attr_header.flags & 0x8001) != 0;
                        let compression_unit_offset = nr_offset + 18;
                        let has_compression_unit = if compression_unit_offset + 2 <= data.len() {
                            let compression_unit = u16::from_le_bytes(
                                data[compression_unit_offset..compression_unit_offset + 2]
                                    .try_into()
                                    .unwrap_or([0; 2]),
                            );
                            compression_unit > 0
                        } else {
                            false
                        };

                        let use_compressed_size = is_compressed_or_sparse || has_compression_unit;
                        let compressed_size_offset = nr_offset + 48; // offset + 64

                        let allocated =
                            if use_compressed_size && compressed_size_offset + 8 <= data.len() {
                                // Read CompressedSize for compressed/sparse files
                                i64::from_le_bytes(
                                    data[compressed_size_offset..compressed_size_offset + 8]
                                        .try_into()
                                        .unwrap_or([0; 8]),
                                )
                            } else {
                                // Read AllocatedLength for normal files
                                i64::from_le_bytes(
                                    data[nr_offset + 24..nr_offset + 32]
                                        .try_into()
                                        .unwrap_or([0; 8]),
                                )
                            };

                        let size = i64::from_le_bytes(
                            data[nr_offset + 32..nr_offset + 40]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        (
                            size.max(0).cast_unsigned(),
                            allocated.max(0).cast_unsigned(),
                        )
                    } else {
                        (0, 0)
                    }
                } else {
                    let len_offset = offset + 16;
                    if len_offset + 4 <= data.len() {
                        let len = u64::from(u32::from_le_bytes(
                            data[len_offset..len_offset + 4]
                                .try_into()
                                .unwrap_or([0; 4]),
                        ));
                        (len, 0)
                    } else {
                        (0, 0)
                    }
                };

                if name_len == 0 {
                    // Default $DATA stream — update base record size
                    // Mark that unnamed $DATA exists on the base record
                    // (distinguishes "empty $DATA" from "no $DATA")
                    {
                        let bf = frs_to_usize(base_frs);
                        if bf < index.frs_to_idx.len() {
                            let base_idx = index.frs_to_idx[bf];
                            if base_idx != NO_ENTRY {
                                index.records[u32_as_usize(base_idx)].set_has_default_data();
                            }
                        }
                    }
                    default_data_size = size;
                    default_data_allocated = allocated;
                    found_default_data = true;
                } else {
                    // ADS (named stream)
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
                        streams.push((stream_name, size, allocated));
                    }
                }
            }
            Some(AttributeType::ReparsePoint) => {
                // Parse $REPARSE_POINT - add as stream
                let (rp_size, rp_allocated) = if attr_header.is_non_resident == 0 {
                    let value_length_bytes = &data[offset + 16..offset + 20];
                    let value_length = u64::from(u32::from_le_bytes(
                        value_length_bytes.try_into().unwrap_or([0; 4]),
                    ));
                    (value_length, 0_u64)
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 48 <= data.len() {
                        let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                        let allocated =
                            i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                        let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                        let data_size = i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                        (
                            data_size.max(0).cast_unsigned(),
                            allocated.max(0).cast_unsigned(),
                        )
                    } else {
                        (0_u64, 0_u64)
                    }
                };
                ext_internal_streams.push((rp_size, rp_allocated));
            }
            Some(
                AttributeType::IndexRoot | AttributeType::IndexAllocation | AttributeType::Bitmap,
            ) => {
                // Extract attribute name
                let name_len = usize::from(attr_header.name_length);
                let (is_i30, _attr_name) = if name_len > 0 {
                    let name_offset = offset + usize::from(attr_header.name_offset);
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let is_i30 =
                            attr_header.name_length == 4 && name_bytes == b"$\x00I\x003\x000\x00";
                        let name = if is_i30 {
                            String::new()
                        } else {
                            let name_u16: SmallVec<[u16; 64]> = name_bytes
                                .chunks_exact(2)
                                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                                .collect();
                            String::from_utf16_lossy(&name_u16)
                        };
                        (is_i30, name)
                    } else {
                        (false, String::new())
                    }
                } else {
                    (false, String::new())
                };

                if is_i30 {
                    // Accumulate $I30 sizes
                    if attr_header.is_non_resident == 0 {
                        let value_length_bytes = &data[offset + 16..offset + 20];
                        let value_length = u64::from(u32::from_le_bytes(
                            value_length_bytes.try_into().unwrap_or([0; 4]),
                        ));
                        dir_index_size += value_length;
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                            let allocated =
                                i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                            let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                            let data_size =
                                i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                            dir_index_size += data_size.max(0).cast_unsigned();
                            dir_index_allocated += allocated.max(0).cast_unsigned();
                        }
                    }
                } else {
                    // Non-$I30 index — internal stream for tree metrics
                    let is_primary = if attr_header.is_non_resident == 0 {
                        true
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 8 <= data.len() {
                            let lowest_vcn = i64::from_le_bytes(
                                data[nr_offset..nr_offset + 8].try_into().unwrap_or([0; 8]),
                            );
                            lowest_vcn == 0
                        } else {
                            false
                        }
                    };

                    if is_primary {
                        let (size, allocated) = if attr_header.is_non_resident == 0 {
                            let value_length_bytes = &data[offset + 16..offset + 20];
                            let value_length = u64::from(u32::from_le_bytes(
                                value_length_bytes.try_into().unwrap_or([0; 4]),
                            ));
                            (value_length, 0_u64)
                        } else {
                            let nr_offset = offset + 16;
                            if nr_offset + 48 <= data.len() {
                                let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                                let allocated =
                                    i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                                let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                                let data_size =
                                    i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                                (
                                    data_size.max(0).cast_unsigned(),
                                    allocated.max(0).cast_unsigned(),
                                )
                            } else {
                                (0_u64, 0_u64)
                            }
                        };

                        ext_internal_streams.push((size, allocated));
                    }
                }
            }
            Some(
                AttributeType::ObjectId
                | AttributeType::VolumeName
                | AttributeType::VolumeInformation
                | AttributeType::PropertySet
                | AttributeType::Ea
                | AttributeType::EaInformation
                | AttributeType::LoggedUtilityStream
                | AttributeType::SecurityDescriptor
                | AttributeType::AttributeList,
            ) => {
                // All counted as streams
                let is_primary = if attr_header.is_non_resident == 0 {
                    true
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 8 <= data.len() {
                        let lowest_vcn = i64::from_le_bytes(
                            data[nr_offset..nr_offset + 8].try_into().unwrap_or([0; 8]),
                        );
                        lowest_vcn == 0
                    } else {
                        false
                    }
                };

                if is_primary {
                    let (size, allocated) = if attr_header.is_non_resident == 0 {
                        let value_length_bytes = &data[offset + 16..offset + 20];
                        let value_length = u64::from(u32::from_le_bytes(
                            value_length_bytes.try_into().unwrap_or([0; 4]),
                        ));
                        (value_length, 0_u64)
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                            let allocated =
                                i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                            let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                            let data_size =
                                i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                            (
                                data_size.max(0).cast_unsigned(),
                                allocated.max(0).cast_unsigned(),
                            )
                        } else {
                            (0_u64, 0_u64)
                        }
                    };

                    ext_internal_streams.push((size, allocated));
                }
            }
            Some(AttributeType::StandardInformation) => {
                // Skip - not expected in extension records
            }
            _ => {
                // Unknown attribute types — counted as streams (catch-all).
                let is_primary = if attr_header.is_non_resident == 0 {
                    true
                } else {
                    let nr_offset = offset + 16;
                    if nr_offset + 8 <= data.len() {
                        let lowest_vcn = i64::from_le_bytes(
                            data[nr_offset..nr_offset + 8].try_into().unwrap_or([0; 8]),
                        );
                        lowest_vcn == 0
                    } else {
                        false
                    }
                };

                if is_primary {
                    let (size, allocated) = if attr_header.is_non_resident == 0 {
                        let value_length_bytes = &data[offset + 16..offset + 20];
                        let value_length = u64::from(u32::from_le_bytes(
                            value_length_bytes.try_into().unwrap_or([0; 4]),
                        ));
                        (value_length, 0_u64)
                    } else {
                        let nr_offset = offset + 16;
                        if nr_offset + 48 <= data.len() {
                            let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                            let allocated =
                                i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                            let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                            let data_size =
                                i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                            (
                                data_size.max(0).cast_unsigned(),
                                allocated.max(0).cast_unsigned(),
                            )
                        } else {
                            (0_u64, 0_u64)
                        }
                    };

                    ext_internal_streams.push((size, allocated));
                }
            }
        }

        offset += u32_as_usize(attr_header.length);
    }

    // If no names, user-visible streams, internal streams, default data, or
    // directory index sizes found, nothing to do
    if names.is_empty()
        && streams.is_empty()
        && ext_internal_streams.is_empty()
        && !found_default_data
        && dir_index_size == 0
        && dir_index_allocated == 0
    {
        return false;
    }

    // Add names to the base record
    // First, add all names to the names buffer and create LinkInfo entries
    let mut link_indices: Vec<u32> = Vec::with_capacity(names.len());
    for (name, parent_frs) in &names {
        let name_offset = index.add_name(name);
        let name_len = name.len();
        let is_ascii = name.is_ascii();
        let extension_id = index.intern_extension(name);
        let name_ref = IndexNameRef::new(name_offset, len_to_u16(name_len), is_ascii, extension_id);

        let link_idx = len_to_u32(index.links.len());
        index.links.push(LinkInfo {
            next_entry: NO_ENTRY,
            name: name_ref,
            _pad0: [0; 4],
            parent_frs: *parent_frs,
        });
        link_indices.push(link_idx);
    }

    // Add streams to the streams buffer
    let mut stream_indices: Vec<u32> = Vec::with_capacity(streams.len());
    for (stream_name, size, allocated) in &streams {
        let name_offset = index.add_name(stream_name);
        let name_len = stream_name.len();
        let is_ascii = stream_name.is_ascii();
        let extension_id = index.intern_extension(stream_name);
        let name_ref = IndexNameRef::new(name_offset, len_to_u16(name_len), is_ascii, extension_id);

        let stream_idx = len_to_u32(index.streams.len());
        index.streams.push(IndexStreamInfo {
            size: SizeInfo {
                length: *size,
                allocated: *allocated,
            },
            next_entry: NO_ENTRY,
            name: name_ref,
            flags: 8_u8 << 2_u8,
            _pad0: [0; 3],
        });
        stream_indices.push(stream_idx);
    }

    // Ensure parent directories exist for the new names
    for (_, parent_frs) in &names {
        if *parent_frs != base_frs && *parent_frs != 0 {
            index.get_or_create(*parent_frs);
            // ^ side effect: ensures parent placeholder exists
        }
    }

    // Get the base record and add the names/streams to it
    let base_frs_usize = frs_to_usize(base_frs);
    if base_frs_usize >= index.frs_to_idx.len() {
        // Base record doesn't exist yet — create a placeholder
        index.get_or_create(base_frs);
    }

    let record_idx = index.frs_to_idx[base_frs_usize];
    if record_idx == NO_ENTRY {
        // Base record doesn't exist — create it
        index.get_or_create(base_frs);
    }

    // Now get the record and chain the new links/streams
    let base_idx = index.frs_to_idx[base_frs_usize];
    if base_idx != NO_ENTRY {
        // Snapshot fields from the record before any re-borrowing
        let (pre_chain_name_count, has_valid_name, first_name_next, first_stream_next) = {
            let rec = &index.records[u32_as_usize(base_idx)];
            (
                rec.name_count,
                rec.first_name.name.is_valid(),
                rec.first_name.next_entry,
                rec.first_stream.next_entry,
            )
        };

        // Add new links to the record
        if !link_indices.is_empty() {
            // Check if base record has no name (first_name is empty)
            // This happens when the $FILE_NAME attribute is ONLY in extension records
            if has_valid_name {
                // Base record already has a name — chain extension names as additional hard
                // links. Find the end of the current link chain.
                let last_link_idx = if first_name_next == NO_ENTRY {
                    None
                } else {
                    let mut idx = first_name_next;
                    while index.links[u32_as_usize(idx)].next_entry != NO_ENTRY {
                        idx = index.links[u32_as_usize(idx)].next_entry;
                    }
                    Some(idx)
                };

                // Chain the new links together
                for i in 0..link_indices.len().saturating_sub(1) {
                    let current_idx = u32_as_usize(link_indices[i]);
                    let next_idx = link_indices[i + 1];
                    index.links[current_idx].next_entry = next_idx;
                }

                // Attach to the chain
                if let Some(last_idx) = last_link_idx {
                    index.links[u32_as_usize(last_idx)].next_entry = link_indices[0];
                } else {
                    // first_name has no next_entry, attach directly
                    let rec_link = &mut index.records[u32_as_usize(base_idx)];
                    rec_link.first_name.next_entry = link_indices[0];
                }

                // Update name count
                let rec_name_count = &mut index.records[u32_as_usize(base_idx)];
                rec_name_count.name_count += len_to_u16(link_indices.len());
            } else {
                // Copy the first extension name directly into first_name
                // This matches established behavior (ntfs_index.hpp lines 559-567)
                let first_link_name = index.links[u32_as_usize(link_indices[0])].name;
                let first_link_parent = index.links[u32_as_usize(link_indices[0])].parent_frs;
                let rec_first = &mut index.records[u32_as_usize(base_idx)];
                rec_first.first_name.name = first_link_name;
                rec_first.first_name.parent_frs = first_link_parent;
                // Don't increment name_count for the first name (it's already counted as 1)

                // Chain remaining links (if any) to first_name.next_entry
                if link_indices.len() > 1 {
                    // Chain the remaining links together
                    for i in 1..link_indices.len().saturating_sub(1) {
                        let current_idx = u32_as_usize(link_indices[i]);
                        let next_idx = link_indices[i + 1];
                        index.links[current_idx].next_entry = next_idx;
                    }
                    // Attach remaining links to first_name
                    let rec_extra = &mut index.records[u32_as_usize(base_idx)];
                    rec_extra.first_name.next_entry = link_indices[1];
                    // Update name count for additional links only
                    rec_extra.name_count += len_to_u16(link_indices.len().saturating_sub(1));
                }
            }
        }

        // Chain new streams to the end of the existing stream chain
        if !stream_indices.is_empty() {
            // Find the end of the current stream chain (using snapshot)
            let last_stream_idx = if first_stream_next == NO_ENTRY {
                None
            } else {
                let mut idx = first_stream_next;
                while index.streams[u32_as_usize(idx)].next_entry != NO_ENTRY {
                    idx = index.streams[u32_as_usize(idx)].next_entry;
                }
                Some(idx)
            };

            // Chain the new streams together
            for i in 0..stream_indices.len().saturating_sub(1) {
                let current_idx = u32_as_usize(stream_indices[i]);
                let next_idx = stream_indices[i + 1];
                index.streams[current_idx].next_entry = next_idx;
            }

            // Attach to the chain
            if let Some(last_idx) = last_stream_idx {
                index.streams[u32_as_usize(last_idx)].next_entry = stream_indices[0];
            } else {
                // first_stream has no next_entry, attach directly
                let rec_stream_attach = &mut index.records[u32_as_usize(base_idx)];
                rec_stream_attach.first_stream.next_entry = stream_indices[0];
            }

            // Update stream count (user-visible only)
            let rec_stream_count = &mut index.records[u32_as_usize(base_idx)];
            rec_stream_count.stream_count += len_to_u16(stream_indices.len());
            rec_stream_count.total_stream_count += len_to_u16(stream_indices.len());
        }

        // Build internal stream chain for extension record attributes
        if !ext_internal_streams.is_empty() {
            let rec_internal = &mut index.records[u32_as_usize(base_idx)];

            // Find end of existing internal stream chain
            let last_internal_idx = if rec_internal.first_internal_stream == NO_ENTRY {
                None
            } else {
                let mut idx = rec_internal.first_internal_stream;
                while index.internal_streams[u32_as_usize(idx)].next_entry != NO_ENTRY {
                    idx = index.internal_streams[u32_as_usize(idx)].next_entry;
                }
                Some(idx)
            };

            let mut first_new_internal = NO_ENTRY;
            let mut prev_internal = NO_ENTRY;
            for (ist_size, ist_allocated) in &ext_internal_streams {
                rec_internal.internal_streams_size =
                    rec_internal.internal_streams_size.saturating_add(*ist_size);
                rec_internal.internal_streams_allocated = rec_internal
                    .internal_streams_allocated
                    .saturating_add(*ist_allocated);

                let new_idx = len_to_u32(index.internal_streams.len());
                index
                    .internal_streams
                    .push(crate::index::InternalStreamInfo {
                        size: SizeInfo {
                            length: *ist_size,
                            allocated: *ist_allocated,
                        },
                        next_entry: NO_ENTRY,
                        flags: 0,
                    });

                if first_new_internal == NO_ENTRY {
                    first_new_internal = new_idx;
                }
                if prev_internal != NO_ENTRY {
                    index.internal_streams[u32_as_usize(prev_internal)].next_entry = new_idx;
                }
                prev_internal = new_idx;
            }

            // Attach to existing chain or set as head
            if let Some(last_idx) = last_internal_idx {
                index.internal_streams[u32_as_usize(last_idx)].next_entry = first_new_internal;
            } else {
                let rec_head = &mut index.records[u32_as_usize(base_idx)];
                rec_head.first_internal_stream = first_new_internal;
            }

            // Update total_stream_count to include new internal streams
            let rec_total = &mut index.records[u32_as_usize(base_idx)];
            rec_total.total_stream_count += len_to_u16(ext_internal_streams.len());
        }

        // Merge default $DATA stream from extension record into base record.
        // This handles files whose $DATA attribute doesn't fit in the base MFT
        // record (e.g., large files with extensive run lists).
        if found_default_data {
            let rec_data = &mut index.records[u32_as_usize(base_idx)];
            // Ensure has_default_data bit is set (may not have been set
            // earlier if the base record didn't exist at attribute-parse time)
            rec_data.set_has_default_data();

            // If base record has no $DATA (both fields are 0), use extension's $DATA.
            // Otherwise, accumulate extension $DATA to base $DATA.
            if rec_data.first_stream.size.length == 0 && rec_data.first_stream.size.allocated == 0 {
                // Base has no $DATA — use extension's values
                rec_data.first_stream.size.length = default_data_size;
                rec_data.first_stream.size.allocated = default_data_allocated;
            } else {
                // Base has partial $DATA — accumulate extension values
                rec_data.first_stream.size.length = rec_data
                    .first_stream
                    .size
                    .length
                    .saturating_add(default_data_size);
                rec_data.first_stream.size.allocated = rec_data
                    .first_stream
                    .size
                    .allocated
                    .saturating_add(default_data_allocated);
            }
        }

        // Merge directory index sizes from extension records
        if dir_index_size > 0 || dir_index_allocated > 0 {
            let rec_dir = &mut index.records[u32_as_usize(base_idx)];
            // Add to the first_stream size (which represents the default stream for
            // directories)
            rec_dir.first_stream.size.length += dir_index_size;
            rec_dir.first_stream.size.allocated += dir_index_allocated;
        }

        // Build parent-child relationship for names added from extension records
        // This is critical for compute_tree_metrics() to work correctly.
        // Use the name_count from BEFORE link-chaining to avoid overflow
        let existing_name_count = pre_chain_name_count;

        for (name_idx, (_, parent_frs)) in names.iter().enumerate() {
            let p_frs = *parent_frs;
            if p_frs == base_frs || p_frs == u64::from(NO_ENTRY) {
                continue;
            }

            // Ensure parent exists
            let parent_idx = {
                let p_frs_usize = frs_to_usize(p_frs);
                if p_frs_usize >= index.frs_to_idx.len() {
                    index.frs_to_idx.resize(p_frs_usize + 1, NO_ENTRY);
                }
                if index.frs_to_idx[p_frs_usize] == NO_ENTRY {
                    // Create placeholder parent
                    let new_idx = len_to_u32(index.records.len());
                    index.frs_to_idx[p_frs_usize] = new_idx;
                    index.records.push(crate::index::FileRecord::new(p_frs));
                }
                index.frs_to_idx[p_frs_usize]
            };

            // Add child entry
            // name_index is the position in the combined name list (existing + new)
            // For extension records, the first name might replace first_name (if empty),
            // so we need to account for that
            //
            // FIX: The off-by-one bug was here. Extension names are appended AFTER
            // existing names, so the index should be existing_name_count + name_idx,
            // not existing_name_count - 1 + name_idx.
            //
            // Example: base has 1 name (index 0), extension adds 1 name
            //   - existing_name_count = 1
            //   - name_idx = 0 (first extension name)
            //   - effective_name_idx should be 1 (the second name overall)
            let effective_name_idx = if existing_name_count == 0 {
                // First extension name became first_name, so name_index starts at 0
                len_to_u16(name_idx)
            } else {
                // Extension names are appended after existing names
                existing_name_count + len_to_u16(name_idx)
            };

            let child_idx = len_to_u32(index.children.len());
            let parent = &mut index.records[u32_as_usize(parent_idx)];
            let old_first_child = parent.first_child;
            parent.first_child = child_idx;

            index.children.push(ChildInfo {
                next_entry: old_first_child,
                _pad0: [0; 4],
                child_frs: base_frs,
                name_index: effective_name_idx,
                _pad1: [0; 6],
            });
        }
    }

    !names.is_empty()
        || !streams.is_empty()
        || !ext_internal_streams.is_empty()
        || found_default_data
        || dir_index_size > 0
        || dir_index_allocated > 0
}
