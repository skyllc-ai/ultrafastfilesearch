//! Single-pass direct-to-index parser (C++-style inline approach).
//!
//! Exception: This file is intentionally monolithic (840+ LOC) because it
//! implements a performance-critical hot path that handles all NTFS attribute
//! types inline. Splitting would introduce indirection overhead and hurt
//! performance. See `scripts/ci/file_size_exceptions.txt`.
//!
//! This module implements the high-performance single-pass parser that matches
//! the C++ architecture. It parses MFT records directly into `MftIndex` without
//! creating intermediate `ParsedRecord` allocations, which is critical for IOCP
//! performance.

use core::mem::size_of;

use smallvec::SmallVec;
use zerocopy::FromBytes;

use super::index_extension::parse_extension_to_index;
use crate::ntfs::is_internal_windows_stream;

/// Parses a record directly into `MftIndex` (single-pass inline parsing).
///
/// This function parses the record and adds it directly to the index,
/// creating parent placeholders on-demand. This is the C++-style single-pass
/// approach that eliminates the intermediate `ParsedRecord` allocation.
///
/// Handles ALL attribute types that `parse_record_full()` handles, including:
/// - `$STANDARD_INFORMATION`, `$FILE_NAME`, `$DATA` (default + ADS)
/// - `$REPARSE_POINT` (for WoF detection and junctions/symlinks)
/// - `$INDEX_ROOT`, `$INDEX_ALLOCATION`, `$BITMAP` (directory indexes)
/// - `$OBJECT_ID`, `$VOLUME_NAME`, `$VOLUME_INFORMATION`, `$PROPERTY_SET`
/// - `$EA`, `$EA_INFORMATION`, `$LOGGED_UTILITY_STREAM`
/// - `$SECURITY_DESCRIPTOR`, `$ATTRIBUTE_LIST`
/// - Unknown attribute types (counted as streams for C++ parity)
///
/// # Returns
///
/// `true` if a record was added to the index, `false` if skipped.
#[expect(
    clippy::too_many_lines,
    reason = "monolithic parser kept for performance-critical hot path"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "NTFS attribute dispatch is inherently complex"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "NTFS field sizes are bounded by u16/u32 record layout"
)]
pub fn parse_record_to_index(data: &[u8], frs: u64, index: &mut crate::index::MftIndex) -> bool {
    use crate::index::{
        ChildInfo, IndexNameRef, IndexStreamInfo, LinkInfo, NO_ENTRY, SizeInfo, StandardInfo,
    };
    use crate::ntfs::{
        AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
        StandardInformation, file_reference_to_frs, filetime_to_unix_micros,
    };

    // Check for parity debug logging
    let debug_parity = std::env::var("UFFS_PARITY_DEBUG").is_ok();

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let header = match FileRecordSegmentHeader::read_from_prefix(data) {
        Ok((header, _)) => header,
        Err(_) => return false,
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
    // C++ does this inline during parsing (see ntfs_index.hpp lines 521-583)
    if !header.is_base_record() {
        let base_frs = file_reference_to_frs(header.base_file_record_segment);
        return parse_extension_to_index(data, base_frs, index);
    }

    let is_directory = header.is_directory();

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    // Temporary storage for parsed data
    let mut std_info = StandardInfo::default();
    let mut primary_name: Option<(String, u64, u8, u16)> = None; // (name, parent_frs, namespace, parse_index)
    let mut additional_names: SmallVec<[(String, u64, u16); 4]> = SmallVec::new();
    let mut name_parse_counter: u16 = 0;
    let mut default_size = 0u64;
    let mut default_allocated = 0u64;
    let mut found_data_attr = false;
    // User-visible ADS: (stream_name, size, allocated)
    let mut additional_streams: SmallVec<[(String, u64, u64); 4]> = SmallVec::new();
    // Internal NTFS streams (e.g. $REPARSE, $EA, $OBJECT_ID) — not emitted as
    // output rows but still tracked for tree-metrics accounting.
    // (size, allocated)
    let mut internal_streams: SmallVec<[(u64, u64); 4]> = SmallVec::new();
    let mut reparse_tag: u32 = 0;
    let mut dir_index_size: u64 = 0;
    let mut dir_index_allocated: u64 = 0;

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

        // Validate that the attribute's declared length fits within the record data
        // This prevents reading past record boundaries when attributes are truncated
        if offset + attr_header.length as usize > data.len() {
            break; // Attribute extends past record — stop processing
        }

        let attr_type = AttributeType::from_u32(attr_header.type_code);
        match attr_type {
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    // Parse $STANDARD_INFORMATION
                    let value_offset_bytes = &data[offset + 20..offset + 22];
                    let value_offset =
                        u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0]))
                            as usize;
                    let si_offset = offset + value_offset;
                    if si_offset + size_of::<StandardInformation>() <= data.len() {
                        let si = match StandardInformation::read_from_prefix(&data[si_offset..]) {
                            Ok((si, _)) => si,
                            Err(_) => break,
                        };
                        // Build StandardInfo with proper flags
                        let mut info = StandardInfo::from_attributes(si.file_attributes);
                        info.created = filetime_to_unix_micros(si.creation_time);
                        info.modified = filetime_to_unix_micros(si.modification_time);
                        info.accessed = filetime_to_unix_micros(si.access_time);
                        info.mft_changed = filetime_to_unix_micros(si.mft_change_time);
                        std_info = info;
                    }
                }
            }
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    // Parse $FILE_NAME
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
                        let name_len = fn_attr.file_name_length as usize;
                        let name_bytes_offset = fn_offset + size_of::<FileNameAttribute>();
                        if name_bytes_offset + name_len * 2 <= data.len() {
                            let name_bytes =
                                &data[name_bytes_offset..name_bytes_offset + name_len * 2];
                            // SmallVec avoids heap allocation for typical filenames (<= 64 chars)
                            let name_u16: SmallVec<[u16; 64]> = name_bytes
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            let name = String::from_utf16_lossy(&name_u16);
                            let parent_frs = file_reference_to_frs(fn_attr.parent_directory);
                            let namespace = fn_attr.file_name_namespace;

                            // Skip DOS-only names (namespace 2)
                            if namespace != 2 {
                                let parse_idx = name_parse_counter;
                                name_parse_counter += 1;
                                let is_better = match namespace {
                                    1 | 3 => true,               // Win32 or Win32+DOS
                                    0 => primary_name.is_none(), // POSIX only if no name yet
                                    _ => false,
                                };
                                if is_better || primary_name.is_none() {
                                    // Move old primary to additional if exists
                                    if let Some((old_name, old_parent, _, old_parse_idx)) =
                                        primary_name.take()
                                    {
                                        additional_names.push((
                                            old_name,
                                            old_parent,
                                            old_parse_idx,
                                        ));
                                    }
                                    primary_name = Some((name, parent_frs, namespace, parse_idx));
                                } else {
                                    additional_names.push((name, parent_frs, parse_idx));
                                }
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
                        let is_primary = lowest_vcn == 0;
                        if debug_parity && !is_primary {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA LowestVCN={}, SKIPPED (continuation extent)",
                                frs, lowest_vcn
                            );
                        }
                        is_primary
                    } else {
                        // Can't read LowestVCN — assume primary (LowestVCN == 0 is common case)
                        // This prevents skipping valid $DATA attributes near record boundaries
                        if debug_parity {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA can't read LowestVCN (nr_offset + 8 > data.len()), assuming primary",
                                frs
                            );
                        }
                        true
                    }
                };

                if !is_primary {
                    // Skip continuation extents - they don't count as new streams
                    offset += attr_header.length as usize;
                    continue;
                }

                // Parse $DATA - track both default stream and ADS
                let name_len = attr_header.name_length as usize;
                let (size, allocated) = if attr_header.is_non_resident != 0 {
                    // Non-resident: size at offset 48, allocated at offset 40
                    // For compressed/sparse files, use CompressedSize at offset 64
                    let nr_offset = offset + 16;
                    let alloc_offset = offset + 40;
                    let size_offset = offset + 48;
                    if size_offset + 8 <= data.len() {
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
                                u64::from_le_bytes(
                                    data[compressed_size_offset..compressed_size_offset + 8]
                                        .try_into()
                                        .unwrap_or([0; 8]),
                                )
                            } else {
                                // Read AllocatedLength for normal files
                                u64::from_le_bytes(
                                    data[alloc_offset..alloc_offset + 8]
                                        .try_into()
                                        .unwrap_or([0; 8]),
                                )
                            };

                        let size = u64::from_le_bytes(
                            data[size_offset..size_offset + 8]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        if debug_parity && name_len == 0 {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA non-resident, size={}, allocated={} (compressed/sparse={})",
                                frs, size, allocated, use_compressed_size
                            );
                        }
                        (size, allocated)
                    } else if alloc_offset + 8 <= data.len() {
                        // Can read AllocatedSize but not DataSize — use AllocatedSize for both
                        // This handles attributes truncated near record boundary
                        let allocated = u64::from_le_bytes(
                            data[alloc_offset..alloc_offset + 8]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        if debug_parity && name_len == 0 {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA non-resident (truncated), allocated={}",
                                frs, allocated
                            );
                        }
                        (allocated, allocated)
                    } else {
                        if debug_parity && name_len == 0 {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA non-resident, but can't read size/allocated (offset beyond data.len()), size=0",
                                frs
                            );
                        }
                        (0, 0)
                    }
                } else {
                    // Resident: value_length at offset 16
                    // Resident files have no clusters allocated - data is stored in MFT record
                    // C++ correctly shows allocated_size=0 for resident files
                    let len_offset = offset + 16;
                    if len_offset + 4 <= data.len() {
                        let len = u32::from_le_bytes(
                            data[len_offset..len_offset + 4]
                                .try_into()
                                .unwrap_or([0; 4]),
                        ) as u64;
                        if debug_parity && name_len == 0 {
                            eprintln!("[PARITY_DEBUG] FRS={}: $DATA resident, size={}", frs, len);
                        }
                        (len, 0) // allocated_size = 0 for resident files
                    } else {
                        if debug_parity && name_len == 0 {
                            eprintln!(
                                "[PARITY_DEBUG] FRS={}: $DATA resident, but can't read value_length, size=0",
                                frs
                            );
                        }
                        (0, 0)
                    }
                };

                if name_len == 0 {
                    // Default stream
                    default_size = size;
                    default_allocated = allocated;
                    found_data_attr = true;
                    if debug_parity && size == 0 {
                        eprintln!(
                            "[PARITY_DEBUG] FRS={}: Default $DATA stream has ZERO size",
                            frs
                        );
                    }
                } else {
                    // Alternate Data Stream (ADS)
                    let name_offset = offset + attr_header.name_offset as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        let name_u16: SmallVec<[u16; 64]> = name_bytes
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        let stream_name = String::from_utf16_lossy(&name_u16);
                        // Filter out internal Windows streams (names starting with $)
                        // These include $DSC, $REPARSE, $EA, $EA_INFORMATION, $TXF_DATA, $OBJECT_ID
                        if !is_internal_windows_stream(&stream_name) {
                            additional_streams.push((stream_name, size, allocated));
                        }
                    }
                }
            }
            Some(AttributeType::ReparsePoint) => {
                // Parse $REPARSE_POINT to get the reparse tag
                // C++ handles both resident and non-resident reparse points
                // C++ also counts $REPARSE_POINT as a stream (for descendants)
                let (rp_size, rp_allocated) = if attr_header.is_non_resident == 0 {
                    // Resident reparse point (common case)
                    let value_length_bytes = &data[offset + 16..offset + 20];
                    let value_length =
                        u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0, 0, 0, 0]))
                            as u64;

                    let value_offset_bytes = &data[offset + 20..offset + 22];
                    let value_offset =
                        u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0]))
                            as usize;
                    let rp_offset = offset + value_offset;
                    if rp_offset + 4 <= data.len() {
                        // Read reparse tag (first 4 bytes of reparse point data)
                        let tag_bytes = &data[rp_offset..rp_offset + 4];
                        reparse_tag =
                            u32::from_le_bytes(tag_bytes.try_into().unwrap_or([0, 0, 0, 0]));
                    }
                    (value_length, 0_u64) // Resident, allocated=0
                } else {
                    // Non-resident reparse point (rare - large reparse data)
                    let nr_offset = offset + 16;
                    if nr_offset + 48 <= data.len() {
                        let alloc_bytes = &data[nr_offset + 24..nr_offset + 32];
                        let allocated =
                            i64::from_le_bytes(alloc_bytes.try_into().unwrap_or([0; 8]));
                        let size_bytes = &data[nr_offset + 32..nr_offset + 40];
                        let data_size = i64::from_le_bytes(size_bytes.try_into().unwrap_or([0; 8]));
                        (data_size.max(0) as u64, allocated.max(0) as u64)
                    } else {
                        (0_u64, 0_u64)
                    }
                };

                // $REPARSE_POINT is an internal stream — tracked for tree metrics
                // but not emitted as a user-visible output row
                internal_streams.push((rp_size, rp_allocated));
            }
            Some(
                AttributeType::IndexRoot | AttributeType::IndexAllocation | AttributeType::Bitmap,
            ) => {
                // C++ includes $INDEX_ROOT and $INDEX_ALLOCATION with name $I30
                // in directory size. For non-$I30 indexes, C++ counts them as streams.

                // Extract attribute name
                let name_len = attr_header.name_length as usize;
                let (is_i30, _attr_name) = if name_len > 0 {
                    let name_offset = offset + attr_header.name_offset as usize;
                    if name_offset + name_len * 2 <= data.len() {
                        let name_bytes = &data[name_offset..name_offset + name_len * 2];
                        // Check for "$I30" in UTF-16LE
                        let is_i30 =
                            attr_header.name_length == 4 && name_bytes == b"$\x00I\x003\x000\x00";
                        // Decode name for non-$I30 indexes
                        let name = if is_i30 {
                            String::new()
                        } else {
                            let name_u16: SmallVec<[u16; 64]> = name_bytes
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
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
                    // Accumulate $I30 sizes for directories
                    if attr_header.is_non_resident == 0 {
                        let value_length_bytes = &data[offset + 16..offset + 20];
                        let value_length =
                            u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0; 4]))
                                as u64;
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
                            dir_index_size += data_size.max(0) as u64;
                            dir_index_allocated += allocated.max(0) as u64;
                        }
                    }
                } else {
                    // Non-$I30 index - count as stream
                    // Check if primary attribute (LowestVCN == 0)
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
                            let value_length =
                                u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0; 4]))
                                    as u64;
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
                                (data_size.max(0) as u64, allocated.max(0) as u64)
                            } else {
                                (0_u64, 0_u64)
                            }
                        };

                        // Non-$I30 index attributes are internal streams
                        internal_streams.push((size, allocated));
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
                // All these are internal streams — tracked for tree metrics but
                // not emitted as user-visible output rows.
                // Check if primary attribute (LowestVCN == 0)
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
                        let value_length =
                            u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0; 4]))
                                as u64;
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
                            (data_size.max(0) as u64, allocated.max(0) as u64)
                        } else {
                            (0_u64, 0_u64)
                        }
                    };

                    internal_streams.push((size, allocated));
                }
            }
            _ => {
                // Unknown attribute types are internal streams — tracked for
                // tree metrics but not emitted as user-visible output rows.
                // Check if primary attribute (LowestVCN == 0)
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
                        let value_length =
                            u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0; 4]))
                                as u64;
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
                            (data_size.max(0) as u64, allocated.max(0) as u64)
                        } else {
                            (0_u64, 0_u64)
                        }
                    };

                    internal_streams.push((size, allocated));
                }
            }
        }

        offset += attr_header.length as usize;
    }

    // Set directory flag in std_info BEFORE checking for filename
    // This ensures is_directory is set even when $FILE_NAME is in extension record
    if is_directory {
        std_info.set_directory(true);
        // For directories, set default size to directory index size
        if dir_index_size > 0 {
            default_size = dir_index_size;
            default_allocated = dir_index_allocated;
        }
    } else if debug_parity && !found_data_attr && default_size == 0 {
        eprintln!(
            "[PARITY_DEBUG] FRS={}: Non-directory file has NO $DATA attribute, size will be 0",
            frs
        );
    }

    // Handle records without a filename in the base record
    // The $FILE_NAME may be in an extension record - we still need to store stdinfo
    let (name, parent_frs, _namespace, primary_parse_index) = match primary_name {
        Some(n) => n,
        None => {
            // No $FILE_NAME in base record - store stdinfo anyway
            // The extension record will add the name later
            //
            // IMPORTANT: We must still add ADS streams from the base record!
            // The $FILE_NAME may be in an extension record, but the ADS are here.
            // Without this, ADS on files/directories with extension records are lost.

            // Pre-process user-visible ADS streams BEFORE creating the record
            let additional_stream_count = additional_streams.len();
            let mut stream_indices: Vec<u32> = Vec::with_capacity(additional_stream_count);
            for (stream_name, stream_size, stream_allocated) in additional_streams {
                let stream_name_offset = index.add_name(&stream_name);
                let stream_name_len = stream_name.len();
                let stream_is_ascii = stream_name.is_ascii();
                let extension_id = index.intern_extension(&stream_name);
                let stream_name_ref = IndexNameRef::new(
                    stream_name_offset,
                    stream_name_len as u16,
                    stream_is_ascii,
                    extension_id,
                );

                let stream_idx = index.streams.len() as u32;
                index.streams.push(IndexStreamInfo {
                    size: SizeInfo {
                        length: stream_size,
                        allocated: stream_allocated,
                    },
                    next_entry: NO_ENTRY,
                    name: stream_name_ref,
                    // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
                    flags: 8 << 2,
                });
                stream_indices.push(stream_idx);
            }

            // Build internal stream chain for tree-metrics accounting
            let internal_stream_count = internal_streams.len();
            let mut internal_size_total = 0_u64;
            let mut internal_alloc_total = 0_u64;
            let mut first_internal = NO_ENTRY;
            let mut last_internal = NO_ENTRY;
            for (ist_size, ist_allocated) in internal_streams {
                internal_size_total = internal_size_total.saturating_add(ist_size);
                internal_alloc_total = internal_alloc_total.saturating_add(ist_allocated);
                let new_idx = index.internal_streams.len() as u32;
                index
                    .internal_streams
                    .push(crate::index::InternalStreamInfo {
                        size: SizeInfo {
                            length: ist_size,
                            allocated: ist_allocated,
                        },
                        next_entry: NO_ENTRY,
                        flags: 0,
                    });
                if last_internal == NO_ENTRY {
                    first_internal = new_idx;
                } else {
                    index.internal_streams[last_internal as usize].next_entry = new_idx;
                }
                last_internal = new_idx;
            }

            // Now create the record and set up streams
            let record = index.get_or_create(frs);
            record.stdinfo = std_info;
            record.first_stream.size = SizeInfo {
                length: default_size,
                allocated: default_allocated,
            };
            // Set type_name_id for first_stream: 0 for directories ($I30), 8 for files
            // ($DATA)
            record.first_stream.flags = if record.stdinfo.is_directory() {
                0 // type_name_id=0 for directory index ($I30)
            } else {
                8 << 2 // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
            };
            record.internal_streams_size = internal_size_total;
            record.internal_streams_allocated = internal_alloc_total;
            record.first_internal_stream = first_internal;

            // Chain user-visible ADS streams to first_stream
            if !stream_indices.is_empty() {
                // Chain the streams together
                for i in 0..stream_indices.len().saturating_sub(1) {
                    let current_idx = stream_indices[i] as usize;
                    let next_idx = stream_indices[i + 1];
                    index.streams[current_idx].next_entry = next_idx;
                }
                // Attach to first_stream
                let record = index.get_or_create(frs);
                record.first_stream.next_entry = stream_indices[0];
            }

            // stream_count = user-visible streams only (1 default + ADS)
            let record = index.get_or_create(frs);
            record.stream_count = 1 + additional_stream_count as u16;
            // total_stream_count = ALL streams including internal
            record.total_stream_count =
                1 + additional_stream_count as u16 + internal_stream_count as u16;

            // Leave first_name empty - extension record will fill it
            return false;
        }
    };

    // Add primary name to names buffer and get reference
    let name_offset = index.add_name(&name);
    let name_len = name.len();
    let is_ascii = name.is_ascii();
    let extension_id = index.intern_extension(&name);
    let name_ref = IndexNameRef::new(name_offset, name_len as u16, is_ascii, extension_id);

    // Pre-process additional names: add to names buffer and links list BEFORE
    // getting record reference This avoids borrow checker issues with holding
    // &mut record while modifying index
    let additional_count = additional_names.len();
    let mut link_indices: Vec<u32> = Vec::with_capacity(additional_count);
    // Collect parent FRS values for building children array later
    let mut additional_parent_frs: SmallVec<[(u64, u16); 4]> =
        SmallVec::with_capacity(additional_count);
    for (link_name, link_parent, link_parse_idx) in additional_names {
        additional_parent_frs.push((link_parent, link_parse_idx));
        let link_offset = index.add_name(&link_name);
        let link_len = link_name.len();
        let link_is_ascii = link_name.is_ascii();
        let extension_id = index.intern_extension(&link_name);
        let link_name_ref =
            IndexNameRef::new(link_offset, link_len as u16, link_is_ascii, extension_id);

        let link_idx = index.links.len() as u32;
        index.links.push(LinkInfo {
            next_entry: NO_ENTRY, // Will be patched below
            name: link_name_ref,
            parent_frs: link_parent,
        });
        link_indices.push(link_idx);
    }

    // Pre-process user-visible ADS streams: add to names buffer and streams list
    let additional_stream_count = additional_streams.len();
    let mut stream_indices: Vec<u32> = Vec::with_capacity(additional_stream_count);
    for (stream_name, stream_size, stream_allocated) in additional_streams {
        let stream_name_offset = index.add_name(&stream_name);
        let stream_name_len = stream_name.len();
        let stream_is_ascii = stream_name.is_ascii();
        let extension_id = index.intern_extension(&stream_name);
        let stream_name_ref = IndexNameRef::new(
            stream_name_offset,
            stream_name_len as u16,
            stream_is_ascii,
            extension_id,
        );

        let stream_idx = index.streams.len() as u32;
        index.streams.push(IndexStreamInfo {
            size: SizeInfo {
                length: stream_size,
                allocated: stream_allocated,
            },
            next_entry: NO_ENTRY, // Will be patched below
            name: stream_name_ref,
            // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
            flags: 8 << 2,
        });
        stream_indices.push(stream_idx);
    }

    // Build internal stream chain for tree-metrics accounting
    let internal_stream_count = internal_streams.len();
    let mut internal_size_total = 0_u64;
    let mut internal_alloc_total = 0_u64;
    let mut first_internal = NO_ENTRY;
    let mut last_internal = NO_ENTRY;
    for (ist_size, ist_allocated) in internal_streams {
        internal_size_total = internal_size_total.saturating_add(ist_size);
        internal_alloc_total = internal_alloc_total.saturating_add(ist_allocated);
        let new_idx = index.internal_streams.len() as u32;
        index
            .internal_streams
            .push(crate::index::InternalStreamInfo {
                size: SizeInfo {
                    length: ist_size,
                    allocated: ist_allocated,
                },
                next_entry: NO_ENTRY,
                flags: 0,
            });
        if last_internal == NO_ENTRY {
            first_internal = new_idx;
        } else {
            index.internal_streams[last_internal as usize].next_entry = new_idx;
        }
        last_internal = new_idx;
    }

    // Ensure parent exists (create placeholder if needed) - do this before getting
    // our record
    if parent_frs != frs && parent_frs != 0 {
        let _ = index.get_or_create(parent_frs);
    }

    // Now get or create the record in the index - no more index mutations after
    // this
    let record = index.get_or_create(frs);
    record.stdinfo = std_info;
    record.first_stream.size = SizeInfo {
        length: default_size,
        allocated: default_allocated,
    };
    // Set type_name_id for first_stream: 0 for directories ($I30), 8 for files
    // ($DATA)
    record.first_stream.flags = if record.stdinfo.is_directory() {
        0 // type_name_id=0 for directory index ($I30)
    } else {
        8 << 2 // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
    };
    record.first_name = LinkInfo {
        next_entry: NO_ENTRY,
        name: name_ref,
        parent_frs,
    };
    record.name_count = 1 + additional_count as u16;
    // stream_count = user-visible streams only (1 default + ADS)
    record.stream_count = 1 + additional_stream_count as u16;
    // total_stream_count = ALL streams including internal (for tree metrics)
    record.total_stream_count = 1 + additional_stream_count as u16 + internal_stream_count as u16;
    // Internal stream metadata for tree metrics
    record.internal_streams_size = internal_size_total;
    record.internal_streams_allocated = internal_alloc_total;
    record.first_internal_stream = first_internal;
    // Set reparse tag if this is a reparse point
    record.reparse_tag = reparse_tag;

    // Chain the additional links: first_name -> link[0] -> link[1] -> ... ->
    // NO_ENTRY The links were pushed with next_entry = NO_ENTRY, now we chain
    // them
    if !link_indices.is_empty() {
        // Point first_name to the first additional link
        record.first_name.next_entry = link_indices[0];
    }

    // Chain the additional streams: first_stream -> stream[0] -> stream[1] -> ...
    if !stream_indices.is_empty() {
        // Point first_stream to the first additional stream
        record.first_stream.next_entry = stream_indices[0];
    }

    // Chain the links together
    for i in 0..link_indices.len().saturating_sub(1) {
        let current_idx = link_indices[i] as usize;
        let next_idx = link_indices[i + 1];
        index.links[current_idx].next_entry = next_idx;
    }

    // Chain the streams together
    for i in 0..stream_indices.len().saturating_sub(1) {
        let current_idx = stream_indices[i] as usize;
        let next_idx = stream_indices[i + 1];
        index.streams[current_idx].next_entry = next_idx;
    }

    // Build parent-child relationship for tree metrics computation
    // This is critical for compute_tree_metrics() to work correctly.
    // Each name (primary + additional) creates a child entry in its parent.
    // name_index 0 = primary name, 1+ = additional names (hardlinks)

    // Helper to add a child entry to a parent
    let add_child_entry = |index: &mut crate::index::MftIndex, p_frs: u64, name_idx: u16| {
        if p_frs == frs || p_frs == 0 || p_frs == u64::from(NO_ENTRY) {
            return;
        }
        // Ensure parent exists
        let parent_idx = {
            let p_frs_usize = p_frs as usize;
            if p_frs_usize >= index.frs_to_idx.len() {
                index.frs_to_idx.resize(p_frs_usize + 1, NO_ENTRY);
            }
            if index.frs_to_idx[p_frs_usize] == NO_ENTRY {
                // Create placeholder parent
                let new_idx = index.records.len() as u32;
                index.frs_to_idx[p_frs_usize] = new_idx;
                index.records.push(crate::index::FileRecord::new(p_frs));
            }
            index.frs_to_idx[p_frs_usize]
        };

        // Add child entry
        let child_idx = index.children.len() as u32;
        let parent = &mut index.records[parent_idx as usize];
        let old_first_child = parent.first_child;
        parent.first_child = child_idx;

        index.children.push(ChildInfo {
            next_entry: old_first_child,
            child_frs: frs,
            name_index: name_idx,
        });
    };

    // Add child entry for primary name (using C++ parse-order index)
    add_child_entry(index, parent_frs, primary_parse_index);

    // Add child entries for additional names (hardlinks)
    for &(link_parent_frs, link_parse_idx) in additional_parent_frs.iter() {
        add_child_entry(index, link_parent_frs, link_parse_idx);
    }

    true
}
