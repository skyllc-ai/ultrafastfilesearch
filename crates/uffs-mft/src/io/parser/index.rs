use core::mem::size_of;

use smallvec::SmallVec;

use super::index_extension::parse_extension_to_index;
use crate::ntfs::is_internal_windows_stream;

/// Parses a record directly into MftIndex (inline parsing for IOCP).
///
/// This function parses the record and adds it directly to the index,
/// creating parent placeholders on-demand. This is the legacy-output parity
/// approach that eliminates the intermediate `ParsedRecord` allocation.
///
/// # Returns
///
/// `true` if a record was added to the index, `false` if skipped.
#[deprecated(note = "Use parse_record_full() + MftRecordMerger + from_parsed_records() instead")]
#[expect(
    unsafe_code,
    reason = "ptr::read for NTFS header and attribute parsing from raw bytes"
)]
#[expect(
    clippy::too_many_lines,
    reason = "monolithic parser kept for performance-critical hot path"
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

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    // SAFETY: We've verified the buffer is large enough for the header.
    let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

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
    // ADS: (stream_name, size, allocated)
    let mut additional_streams: SmallVec<[(String, u64, u64); 4]> = SmallVec::new();

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let attr_header: AttributeRecordHeader =
            unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }

        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    // Parse $STANDARD_INFORMATION
                    let value_offset_bytes = &data[offset + 20..offset + 22];
                    let value_offset =
                        u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0]))
                            as usize;
                    let si_offset = offset + value_offset;
                    if si_offset + size_of::<StandardInformation>() <= data.len() {
                        let si: StandardInformation =
                            unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };
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
                        let fn_attr: FileNameAttribute =
                            unsafe { core::ptr::read(data[fn_offset..].as_ptr().cast()) };
                        let name_len = fn_attr.file_name_length as usize;
                        let name_bytes_offset = fn_offset + size_of::<FileNameAttribute>();
                        if name_bytes_offset + name_len * 2 <= data.len() {
                            let name_bytes =
                                &data[name_bytes_offset..name_bytes_offset + name_len * 2];
                            let name_u16: Vec<u16> = name_bytes
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

                // Parse $DATA - track both default stream and ADS
                let name_len = attr_header.name_length as usize;
                let (size, allocated) = if attr_header.is_non_resident != 0 {
                    // Non-resident: size at offset 48, allocated at offset 40
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
                    // C++ correctly shows allocated_size=0 for resident files
                    let len_offset = offset + 16;
                    if len_offset + 4 <= data.len() {
                        let len = u32::from_le_bytes(
                            data[len_offset..len_offset + 4]
                                .try_into()
                                .unwrap_or([0; 4]),
                        ) as u64;
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
            _ => {}
        }

        offset += attr_header.length as usize;
    }

    // Set directory flag in std_info BEFORE checking for filename
    // This ensures is_directory is set even when $FILE_NAME is in extension record
    if is_directory {
        std_info.set_directory(true);
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

            // Pre-process ADS streams BEFORE creating the record
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
                    flags: 0,
                });
                stream_indices.push(stream_idx);
            }

            // Now create the record and set up streams
            let record = index.get_or_create(frs);
            record.stdinfo = std_info;
            record.first_stream.size = SizeInfo {
                length: default_size,
                allocated: default_allocated,
            };

            // Chain ADS streams to first_stream
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
                record.stream_count = 1 + additional_stream_count as u16;
            }

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

    // Pre-process additional streams (ADS): add to names buffer and streams list
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
            flags: 0,
        });
        stream_indices.push(stream_idx);
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
    record.first_name = LinkInfo {
        next_entry: NO_ENTRY,
        name: name_ref,
        parent_frs,
    };
    record.name_count = 1 + additional_count as u16;
    // stream_count = 1 (default) + additional ADS
    record.stream_count = 1 + additional_stream_count as u16;

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
