// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unified MFT record processor.
//!
//! ONE function processes ALL records (base AND extension) through the SAME
//! attribute loop.  This eliminates the dual-parser architecture that caused
//! name-ordering and stream-counting discrepancies.

use core::mem::size_of;

use zerocopy::FromBytes as _;

use crate::index::{
    ChildInfo, IndexNameRef, IndexStreamInfo, InternalStreamInfo, MftIndex, NO_ENTRY, SizeInfo,
    len_to_u16, len_to_u32, u32_as_usize,
};
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    StandardInformation, file_reference_to_frs,
};

/// Decode a UTF-16LE byte slice into `out`, replacing unpaired surrogates
/// with U+FFFD.  Returns the number of bytes written to `out`.
///
/// This avoids the per-call `SmallVec` + `String` allocation that
/// `String::from_utf16_lossy` requires.
#[inline]
fn decode_utf16le_into(bytes: &[u8], out: &mut String) {
    out.clear();
    let mut i = 0_usize;
    while let Some(pair) = bytes
        .get(i..i + 2)
        .and_then(|sl| <[u8; 2]>::try_from(sl).ok())
    {
        let code = u16::from_le_bytes(pair);
        i += 2;
        match code {
            // High surrogate
            0xD800..=0xDBFF => {
                if let Some(low_pair) = bytes
                    .get(i..i + 2)
                    .and_then(|sl| <[u8; 2]>::try_from(sl).ok())
                {
                    let low = u16::from_le_bytes(low_pair);
                    if (0xDC00..=0xDFFF).contains(&low) {
                        i += 2;
                        let cp = 0x1_0000_u32
                            + ((u32::from(code) - 0xD800_u32) << 10_u32)
                            + (u32::from(low) - 0xDC00_u32);
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        } else {
                            out.push(char::REPLACEMENT_CHARACTER);
                        }
                    } else {
                        out.push(char::REPLACEMENT_CHARACTER);
                    }
                } else {
                    out.push(char::REPLACEMENT_CHARACTER);
                }
            }
            // Low surrogate without preceding high
            0xDC00..=0xDFFF => {
                out.push(char::REPLACEMENT_CHARACTER);
            }
            _ => {
                // All non-surrogate u16 values are valid Unicode scalar values.
                // `char::from_u32` is cheap for the common BMP case.
                if let Some(ch) = char::from_u32(u32::from(code)) {
                    out.push(ch);
                }
            }
        }
    }
}

/// Process a single MFT record (base OR extension) in one pass.
///
/// Both base and extension records go through the **same** attribute loop.
/// For extensions, `frs_base` points to the base record; for base records,
/// `frs_base == frs`.
///
/// Records are created with `FileRecord::new_unified()` (internal helper)
/// which starts all counts at 0.  Every accepted `$FILE_NAME` and every
/// first-occurrence stream increments the respective count.
///
/// Returns `true` if the record was successfully processed.
#[expect(
    clippy::too_many_lines,
    reason = "single-pass MFT record processor — kept as one function for correctness"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "single-pass attribute walk — complexity is inherent to NTFS record structure"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "base_ri validated by ensure_record; bounds checked inline"
)]
pub fn process_record(data: &[u8], frs: u64, index: &mut MftIndex, name_buf: &mut String) -> bool {
    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
        return false;
    };

    // Only process valid, in-use FILE records
    let multi_sector_header = header.multi_sector_header;
    if !header.is_in_use() || !multi_sector_header.is_file_record() {
        return false;
    }

    // Determine base FRS (extension records point to their parent base record)
    let frs_base = if header.is_base_record() {
        frs
    } else {
        file_reference_to_frs(header.base_file_record_segment)
    };

    let is_directory = header.is_directory();

    // Get or create the base record (zero-based counts).
    // Cache the record index so we don't repeat the frs→idx lookup for
    // every attribute in this record.
    let base_ri = u32_as_usize(index.ensure_record(frs_base));

    // ── Attribute loop ─────────────────────────────────────────────────
    let mut offset = usize::from(header.first_attribute_offset);
    let max_offset = core::cmp::min(u32_as_usize(header.bytes_in_use), data.len());

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let Ok((attr_header, _)) = AttributeRecordHeader::read_from_prefix(&data[offset..]) else {
            break;
        };

        if attr_header.type_code == AttributeType::END_MARKER {
            break;
        }
        if attr_header.length == 0 || offset + u32_as_usize(attr_header.length) > data.len() {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            // ── $STANDARD_INFORMATION (0x10) ─────────────────────────
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    let vo = usize::from(rd_u16(data, offset + 20));
                    let si_off = offset + vo;
                    if si_off + size_of::<StandardInformation>() <= data.len()
                        && let Ok((si, _)) = StandardInformation::read_from_prefix(&data[si_off..])
                    {
                        // Fast path: map raw NTFS flags directly to our
                        // compact bitmask — skips the intermediate
                        // ExtendedStandardInfo struct entirely.
                        let mut info =
                            crate::index::StandardInfo::from_raw_ntfs_flags(si.file_attributes);
                        info.created = si.creation_time;
                        info.modified = si.modification_time;
                        info.accessed = si.access_time;
                        info.mft_changed = si.mft_change_time;
                        if is_directory {
                            info.set_directory(true);
                        }
                        index.records[base_ri].stdinfo = info;
                    }
                }
            }

            // ── $FILE_NAME (0x30) ─────────────────────────────────────
            // Push-to-front: each new $FILE_NAME overwrites first_name.
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    let vo = usize::from(rd_u16(data, offset + 20));
                    let fn_off = offset + vo;
                    if fn_off + size_of::<FileNameAttribute>() <= data.len()
                        && let Ok((fn_attr, _)) =
                            FileNameAttribute::read_from_prefix(&data[fn_off..])
                        && fn_attr.file_name_namespace != 2
                    {
                        // Skip DOS-only names (namespace 2)
                        let parent_frs = file_reference_to_frs(fn_attr.parent_directory);
                        let name_len = usize::from(fn_attr.file_name_length);
                        let ns = fn_off + size_of::<FileNameAttribute>();

                        if ns + name_len * 2 <= data.len() {
                            let nb = &data[ns..ns + name_len * 2];
                            decode_utf16le_into(nb, name_buf);

                            // Push old first_name to chain
                            // Copy first_name before mutating (borrow checker)
                            let old_valid = index.records[base_ri].first_name.name.is_valid();
                            let old_first = index.records[base_ri].first_name; // Copy
                            if old_valid {
                                let link_idx = len_to_u32(index.links.len());
                                index.links.push(old_first);
                                index.records[base_ri].first_name.next_entry = link_idx;
                            }

                            // Overwrite first_name with the new name
                            let name_off = index.add_name(name_buf);
                            let is_ascii = name_buf.is_ascii();
                            let ext_id = index.intern_extension(name_buf);
                            let name_ref = IndexNameRef::new(
                                name_off,
                                len_to_u16(name_buf.len()),
                                is_ascii,
                                ext_id,
                            );

                            index.records[base_ri].first_name.name = name_ref;
                            index.records[base_ri].first_name.parent_frs = parent_frs;

                            // Build parent-child relationship.
                            // name_index = name_count BEFORE increment
                            let name_index = index.records[base_ri].name_count;

                            if parent_frs != frs_base && parent_frs != u64::from(NO_ENTRY) {
                                let parent_ri = u32_as_usize(index.ensure_record(parent_frs));
                                let child_idx = len_to_u32(index.children.len());
                                let old_fc = index.records[parent_ri].first_child;
                                index.records[parent_ri].first_child = child_idx;

                                index.children.push(ChildInfo {
                                    next_entry: old_fc,
                                    _pad0: [0; 4],
                                    child_frs: frs_base,
                                    name_index,
                                    _pad1: [0; 6],
                                });
                            }

                            // Increment name_count (zero-based, always increment)
                            // (including the first name).
                            index.records[base_ri].name_count += 1;
                        }
                    }
                }
            }

            // ── ALL OTHER ATTRIBUTES ──────────────────────────────────
            _ => {
                // Determine if this is the primary (unnamed) data attribute
                let is_primary = if attr_header.is_non_resident == 0 {
                    true
                } else {
                    let nr = offset + 16;
                    nr + 8 <= data.len()
                        && i64::from_le_bytes(data[nr..nr + 8].try_into().unwrap_or([0; 8])) == 0
                };

                if is_primary {
                    let attr_type = attr_header.type_code;
                    let aname_len = usize::from(attr_header.name_length);

                    // $I30 directory index check
                    let is_i30 = matches!(
                        AttributeType::from_u32(attr_type),
                        Some(
                            AttributeType::Bitmap
                                | AttributeType::IndexRoot
                                | AttributeType::IndexAllocation
                        )
                    ) && aname_len == 4
                        && {
                            let no = offset + usize::from(attr_header.name_offset);
                            no + 8 <= data.len() && &data[no..no + 8] == b"$\x00I\x003\x000\x00"
                        };

                    // Read stream name (non-$I30, named attributes)
                    let has_stream_name = !is_i30 && aname_len > 0;
                    if has_stream_name {
                        let no = offset + usize::from(attr_header.name_offset);
                        if no + aname_len * 2 <= data.len() {
                            let nb = &data[no..no + aname_len * 2];
                            decode_utf16le_into(nb, name_buf);
                        } else {
                            name_buf.clear();
                        }
                    }

                    // Size calculation for primary data attribute
                    let is_badclus_bad =
                        frs_base == 8 && aname_len == 4 && has_stream_name && name_buf == "$Bad";

                    let (size, alloc) = if attr_header.is_non_resident != 0 {
                        let nr = offset + 16;
                        if nr + 48 <= data.len() {
                            let cu = rd_u16(data, nr + 18); // CompressionUnit
                            let alloc_size = if cu > 0 {
                                rd_u64(data, nr + 48) // CompressedSize
                            } else if is_badclus_bad {
                                rd_u64(data, nr + 40) // InitializedSize
                            } else {
                                rd_u64(data, nr + 24) // AllocatedSize
                            };
                            let logical_size = if is_badclus_bad {
                                rd_u64(data, nr + 40) // InitializedSize
                            } else {
                                rd_u64(data, nr + 32) // DataSize
                            };
                            (logical_size, alloc_size)
                        } else {
                            (0, 0)
                        }
                    } else {
                        (u64::from(rd_u32(data, offset + 16)), 0)
                    };

                    // ── Classify and store ───────────────────────────
                    if is_i30 {
                        // $I30: accumulate into first_stream (directory index)
                        let rec = &mut index.records[base_ri];
                        rec.stdinfo.set_directory(true);
                        rec.first_stream.flags = 0; // type_name_id=0 for $I30

                        rec.first_stream.size.length += size;
                        rec.first_stream.size.allocated += alloc;
                        // Increment counts once for the first $I30 attribute;
                        // subsequent $I30 attrs ($INDEX_ALLOCATION, $BITMAP)
                        // accumulate size without creating new stream entries.
                        if !rec.has_i30_stream() {
                            rec.set_has_i30_stream();
                            rec.stream_count += 1;
                            rec.total_stream_count += 1;
                        }
                    } else if attr_type == AttributeType::DATA_TYPE && aname_len == 0 {
                        // Unnamed $DATA: default stream
                        let rec = &mut index.records[base_ri];
                        // Increment counts once for the first unnamed $DATA;
                        // subsequent unnamed $DATA (from extension records)
                        // accumulate size only.
                        if !rec.has_default_data() {
                            rec.stream_count += 1;
                            rec.total_stream_count += 1;
                        }
                        rec.set_has_default_data();
                        rec.first_stream.size.length += size;
                        rec.first_stream.size.allocated += alloc;
                        rec.first_stream.flags = 8_u8 << 2_u8; // type_name_id=8 for $DATA
                    } else if attr_type == AttributeType::DATA_TYPE && aname_len > 0 {
                        // Named $DATA: ADS (user-visible stream).
                        // Output layer filters internal streams.
                        if has_stream_name && !name_buf.is_empty() {
                            let sn_off = index.add_name(name_buf);
                            let is_ascii = name_buf.is_ascii();
                            let ext_id = index.intern_extension(name_buf);
                            let nr = IndexNameRef::new(
                                sn_off,
                                len_to_u16(name_buf.len()),
                                is_ascii,
                                ext_id,
                            );
                            let si = len_to_u32(index.streams.len());
                            index.streams.push(IndexStreamInfo {
                                size: SizeInfo {
                                    length: size,
                                    allocated: alloc,
                                },
                                next_entry: NO_ENTRY,
                                name: nr,
                                flags: 8_u8 << 2_u8,
                                _pad0: [0; 3],
                            });

                            // Chain to record's stream list
                            let old_next = index.records[base_ri].first_stream.next_entry;
                            // Find end of chain
                            if old_next == NO_ENTRY {
                                index.records[base_ri].first_stream.next_entry = si;
                            } else {
                                let mut tail = old_next;
                                while index.streams[u32_as_usize(tail)].next_entry != NO_ENTRY {
                                    tail = index.streams[u32_as_usize(tail)].next_entry;
                                }
                                index.streams[u32_as_usize(tail)].next_entry = si;
                            }
                            index.records[base_ri].stream_count += 1;
                            index.records[base_ri].total_stream_count += 1;
                        }
                    } else {
                        // All other attribute types: internal stream
                        let ist_idx = len_to_u32(index.internal_streams.len());
                        index.internal_streams.push(InternalStreamInfo {
                            size: SizeInfo {
                                length: size,
                                allocated: alloc,
                            },
                            next_entry: NO_ENTRY,
                            flags: 0,
                        });

                        // Chain to record's internal stream list
                        let head = index.records[base_ri].first_internal_stream;
                        if head == NO_ENTRY {
                            index.records[base_ri].first_internal_stream = ist_idx;
                        } else {
                            // Walk chain to find tail
                            let mut tail = head;
                            while index.internal_streams[u32_as_usize(tail)].next_entry != NO_ENTRY
                            {
                                tail = index.internal_streams[u32_as_usize(tail)].next_entry;
                            }
                            index.internal_streams[u32_as_usize(tail)].next_entry = ist_idx;
                        }
                        let rec = &mut index.records[base_ri];
                        rec.internal_streams_size += size;
                        rec.internal_streams_allocated += alloc;
                        rec.total_stream_count += 1;
                    }

                    // Extract reparse tag from $REPARSE_POINT
                    if attr_type == AttributeType::REPARSE_POINT_TYPE
                        && attr_header.is_non_resident == 0
                    {
                        let vo = usize::from(rd_u16(data, offset + 20));
                        let rp = offset + vo;
                        if rp + 4 <= data.len() {
                            let tag =
                                u32::from_le_bytes(data[rp..rp + 4].try_into().unwrap_or([0; 4]));
                            index.records[base_ri].reparse_tag = tag;
                        }
                    }
                }
            }
        }

        offset += u32_as_usize(attr_header.length);
    }

    // Set directory flag from header if not already set
    if is_directory {
        index.records[base_ri].stdinfo.set_directory(true);
    }

    true
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Read a little-endian u16 from the given offset, returning 0 if out of
/// bounds.
#[inline]
fn rd_u16(buf: &[u8], off: usize) -> u16 {
    buf.get(off..off + 2)
        .and_then(|sl| <[u8; 2]>::try_from(sl).ok())
        .map_or(0, u16::from_le_bytes)
}

/// Read a little-endian u32 from the given offset, returning 0 if out of
/// bounds.
#[inline]
fn rd_u32(buf: &[u8], off: usize) -> u32 {
    buf.get(off..off + 4)
        .and_then(|sl| <[u8; 4]>::try_from(sl).ok())
        .map_or(0, u32::from_le_bytes)
}

/// Read a little-endian u64 from the given offset, returning 0 if out of
/// bounds.
#[inline]
fn rd_u64(buf: &[u8], off: usize) -> u64 {
    buf.get(off..off + 8)
        .and_then(|sl| <[u8; 8]>::try_from(sl).ok())
        .map_or(0, u64::from_le_bytes)
}
