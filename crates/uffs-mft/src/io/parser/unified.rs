//! Unified MFT record processor — mirrors C++ `load()` exactly.
//!
//! ONE function processes ALL records (base AND extension) through the SAME
//! attribute loop.  This eliminates the dual-parser architecture that caused
//! name-ordering and stream-counting discrepancies.
//!
//! C++ reference: `ntfs_index_load.hpp` lines 228-463.

use core::mem::size_of;

use smallvec::SmallVec;
use zerocopy::FromBytes;

use crate::index::{
    ChildInfo, IndexNameRef, IndexStreamInfo, InternalStreamInfo, MftIndex, NO_ENTRY, SizeInfo,
};
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FileNameAttribute, FileRecordSegmentHeader,
    StandardInformation, file_reference_to_frs, filetime_to_unix_micros,
};

/// Process a single MFT record (base OR extension) — mirrors C++ `load()`.
///
/// Both base and extension records go through the **same** attribute loop.
/// For extensions, `frs_base` points to the base record; for base records,
/// `frs_base == frs`.  This is identical to C++ lines 231-234.
///
/// Returns `true` if the record was successfully processed.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors C++ monolithic load() for exact parity"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "direct C++ port — complexity matches reference"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "NTFS field sizes bounded by record layout"
)]
pub fn process_record(data: &[u8], frs: u64, index: &mut MftIndex) -> bool {
    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return false;
    }

    let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
        return false;
    };

    // C++ line 229: only process valid, in-use FILE records
    let multi_sector_header = header.multi_sector_header;
    if !header.is_in_use() || !multi_sector_header.is_file_record() {
        return false;
    }

    // C++ lines 231-233: determine base FRS
    let frs_base = if header.is_base_record() {
        frs
    } else {
        file_reference_to_frs(header.base_file_record_segment)
    };

    let is_directory = header.is_directory();

    // C++ line 234: get or create the base record
    let _ = index.get_or_create(frs_base);

    // ── Attribute loop (C++ lines 240-461) ──────────────────────────────
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let Ok((attr_header, _)) = AttributeRecordHeader::read_from_prefix(&data[offset..]) else {
            break;
        };

        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }
        if attr_header.length == 0 || offset + attr_header.length as usize > data.len() {
            break;
        }

        match AttributeType::from_u32(attr_header.type_code) {
            // ── $STANDARD_INFORMATION (0x10) — C++ lines 249-259 ────
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    let vo = rd_u16(data, offset + 20) as usize;
                    let si_off = offset + vo;
                    if si_off + size_of::<StandardInformation>() <= data.len() {
                        if let Ok((si, _)) = StandardInformation::read_from_prefix(&data[si_off..])
                        {
                            let ext = crate::ntfs::ExtendedStandardInfo::from_attributes(
                                si.file_attributes,
                            );
                            let mut info = crate::index::StandardInfo::from_extended(&ext);
                            info.created = filetime_to_unix_micros(si.creation_time);
                            info.modified = filetime_to_unix_micros(si.modification_time);
                            info.accessed = filetime_to_unix_micros(si.access_time);
                            info.mft_changed = filetime_to_unix_micros(si.mft_change_time);
                            if is_directory {
                                info.set_directory(true);
                            }
                            let rec = index.get_or_create(frs_base);
                            rec.stdinfo = info;
                        }
                    }
                }
            }

            // ── $FILE_NAME (0x30) — C++ lines 264-309 ──────────────
            // Push-to-front: each new $FILE_NAME overwrites first_name.
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    let vo = rd_u16(data, offset + 20) as usize;
                    let fn_off = offset + vo;
                    if fn_off + size_of::<FileNameAttribute>() <= data.len() {
                        if let Ok((fn_attr, _)) =
                            FileNameAttribute::read_from_prefix(&data[fn_off..])
                        {
                            // C++ line 271: skip DOS-only names
                            if fn_attr.file_name_namespace != 2 {
                                let parent_frs = file_reference_to_frs(fn_attr.parent_directory);
                                let name_len = fn_attr.file_name_length as usize;
                                let ns = fn_off + size_of::<FileNameAttribute>();

                                if ns + name_len * 2 <= data.len() {
                                    let nb = &data[ns..ns + name_len * 2];
                                    let u16s: SmallVec<[u16; 64]> = nb
                                        .chunks_exact(2)
                                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                        .collect();
                                    let name = String::from_utf16_lossy(&u16s);

                                    // C++ lines 273-278: push old first_name to chain
                                    // Copy first_name before mutating (borrow checker)
                                    let rec = index.get_or_create(frs_base);
                                    let old_valid = rec.first_name.name.is_valid();
                                    let old_first = rec.first_name; // Copy
                                    if old_valid {
                                        let link_idx = index.links.len() as u32;
                                        index.links.push(old_first);
                                        let rec = index.get_or_create(frs_base);
                                        rec.first_name.next_entry = link_idx;
                                    }

                                    // C++ lines 281-289: overwrite first_name
                                    let name_off = index.add_name(&name);
                                    let is_ascii = name.is_ascii();
                                    let ext_id = index.intern_extension(&name);
                                    let name_ref = IndexNameRef::new(
                                        name_off,
                                        name.len() as u16,
                                        is_ascii,
                                        ext_id,
                                    );

                                    let rec = index.get_or_create(frs_base);
                                    rec.first_name.name = name_ref;
                                    rec.first_name.parent_frs = parent_frs;

                                    // C++ lines 293-304: build parent-child
                                    // name_index = name_count (C++ line 302)
                                    let name_index = rec.name_count;

                                    if parent_frs != frs_base && parent_frs != u64::from(NO_ENTRY) {
                                        let _ = index.get_or_create(parent_frs);
                                        let child_idx = index.children.len() as u32;
                                        let parent_rec = index.get_or_create(parent_frs);
                                        let old_fc = parent_rec.first_child;
                                        parent_rec.first_child = child_idx;

                                        index.children.push(ChildInfo {
                                            next_entry: old_fc,
                                            child_frs: frs_base,
                                            name_index,
                                        });
                                    }

                                    // C++ line 307: ++name_count
                                    // C++ starts at 0 and always increments.
                                    // Rust starts at 1 (FileRecord::new), so
                                    // only increment for the 2nd+ name.
                                    let rec = index.get_or_create(frs_base);
                                    if old_valid {
                                        rec.name_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── ALL OTHER ATTRIBUTES — C++ lines 315-459 ───────────
            _ => {
                // C++ line 358: is_primary_attribute
                let is_primary = if attr_header.is_non_resident == 0 {
                    true
                } else {
                    let nr = offset + 16;
                    nr + 8 <= data.len()
                        && i64::from_le_bytes(data[nr..nr + 8].try_into().unwrap_or([0; 8])) == 0
                };

                if is_primary {
                    let attr_type = attr_header.type_code;
                    let aname_len = attr_header.name_length as usize;

                    // C++ lines 361-366: $I30 check
                    let is_i30 = matches!(
                        AttributeType::from_u32(attr_type),
                        Some(
                            AttributeType::Bitmap
                                | AttributeType::IndexRoot
                                | AttributeType::IndexAllocation
                        )
                    ) && aname_len == 4
                        && {
                            let no = offset + attr_header.name_offset as usize;
                            no + 8 <= data.len() && &data[no..no + 8] == b"$\x00I\x003\x000\x00"
                        };

                    // Read stream name (non-$I30, named attributes)
                    let stream_name: Option<String> = if !is_i30 && aname_len > 0 {
                        let no = offset + attr_header.name_offset as usize;
                        if no + aname_len * 2 <= data.len() {
                            let nb = &data[no..no + aname_len * 2];
                            let u16s: SmallVec<[u16; 64]> = nb
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            Some(String::from_utf16_lossy(&u16s))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // C++ lines 430-452: size calculation
                    let is_badclus_bad =
                        frs_base == 8 && aname_len == 4 && stream_name.as_deref() == Some("$Bad");

                    let (size, alloc) = if attr_header.is_non_resident != 0 {
                        let nr = offset + 16;
                        if nr + 48 <= data.len() {
                            let cu = rd_u16(data, nr + 18); // CompressionUnit
                            let a = if cu > 0 {
                                rd_u64(data, nr + 48) // CompressedSize
                            } else if is_badclus_bad {
                                rd_u64(data, nr + 40) // InitializedSize
                            } else {
                                rd_u64(data, nr + 24) // AllocatedSize
                            };
                            let l = if is_badclus_bad {
                                rd_u64(data, nr + 40) // InitializedSize
                            } else {
                                rd_u64(data, nr + 32) // DataSize
                            };
                            (l, a)
                        } else {
                            (0, 0)
                        }
                    } else {
                        (u64::from(rd_u32(data, offset + 16)), 0)
                    };

                    // ── Classify and store ───────────────────────────
                    if is_i30 {
                        // $I30: accumulate into first_stream (directory index)
                        let rec = index.get_or_create(frs_base);
                        rec.stdinfo.set_directory(true);
                        rec.first_stream.flags = 0; // type_name_id=0 for $I30
                        rec.first_stream.size.length += size;
                        rec.first_stream.size.allocated += alloc;
                    } else if attr_type == AttributeType::Data as u32 && aname_len == 0 {
                        // Unnamed $DATA: default stream
                        let rec = index.get_or_create(frs_base);
                        rec.set_has_default_data();
                        rec.first_stream.size.length += size;
                        rec.first_stream.size.allocated += alloc;
                        rec.first_stream.flags = 8 << 2; // type_name_id=8 for $DATA
                    } else if attr_type == AttributeType::Data as u32 && aname_len > 0 {
                        // Named $DATA: ADS (user-visible stream)
                        // C++ creates a stream entry; output layer filters internals
                        if let Some(ref sn) = stream_name {
                            let sn_off = index.add_name(sn);
                            let is_ascii = sn.is_ascii();
                            let ext_id = index.intern_extension(sn);
                            let nr = IndexNameRef::new(sn_off, sn.len() as u16, is_ascii, ext_id);
                            let si = index.streams.len() as u32;
                            index.streams.push(IndexStreamInfo {
                                size: SizeInfo {
                                    length: size,
                                    allocated: alloc,
                                },
                                next_entry: NO_ENTRY,
                                name: nr,
                                flags: 8 << 2, // type_name_id=8
                            });

                            // Chain to record's stream list
                            let rec = index.get_or_create(frs_base);
                            let old_next = rec.first_stream.next_entry;
                            // Find end of chain
                            if old_next == NO_ENTRY {
                                rec.first_stream.next_entry = si;
                            } else {
                                let mut tail = old_next;
                                while index.streams[tail as usize].next_entry != NO_ENTRY {
                                    tail = index.streams[tail as usize].next_entry;
                                }
                                index.streams[tail as usize].next_entry = si;
                            }
                            let rec = index.get_or_create(frs_base);
                            rec.stream_count += 1;
                            rec.total_stream_count += 1;
                        }
                    } else {
                        // All other attribute types: internal stream
                        let ist_idx = index.internal_streams.len() as u32;
                        index.internal_streams.push(InternalStreamInfo {
                            size: SizeInfo {
                                length: size,
                                allocated: alloc,
                            },
                            next_entry: NO_ENTRY,
                            flags: 0,
                        });

                        // Chain to record's internal stream list
                        // (split borrows: read first_internal_stream, walk chain,
                        // then mutate record)
                        let rec = index.get_or_create(frs_base);
                        let head = rec.first_internal_stream;
                        if head == NO_ENTRY {
                            rec.first_internal_stream = ist_idx;
                        } else {
                            // Walk chain to find tail (drop rec borrow first)
                            let mut tail = head;
                            while index.internal_streams[tail as usize].next_entry != NO_ENTRY {
                                tail = index.internal_streams[tail as usize].next_entry;
                            }
                            index.internal_streams[tail as usize].next_entry = ist_idx;
                        }
                        let rec = index.get_or_create(frs_base);
                        rec.internal_streams_size += size;
                        rec.internal_streams_allocated += alloc;
                        rec.total_stream_count += 1;
                    }

                    // Extract reparse tag from $REPARSE_POINT
                    if attr_type == AttributeType::ReparsePoint as u32
                        && attr_header.is_non_resident == 0
                    {
                        let vo = rd_u16(data, offset + 20) as usize;
                        let rp = offset + vo;
                        if rp + 4 <= data.len() {
                            let tag =
                                u32::from_le_bytes(data[rp..rp + 4].try_into().unwrap_or([0; 4]));
                            let rec = index.get_or_create(frs_base);
                            rec.reparse_tag = tag;
                        }
                    }
                }
            }
        }

        offset += attr_header.length as usize;
    }

    // Set directory flag from header if not already set
    if is_directory {
        let rec = index.get_or_create(frs_base);
        rec.stdinfo.set_directory(true);
    }

    true
}

// ── Helpers ─────────────────────────────────────────────────────────────

#[inline]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    if o + 2 <= d.len() {
        u16::from_le_bytes(d[o..o + 2].try_into().unwrap_or([0; 2]))
    } else {
        0
    }
}

#[inline]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    if o + 4 <= d.len() {
        u32::from_le_bytes(d[o..o + 4].try_into().unwrap_or([0; 4]))
    } else {
        0
    }
}

#[inline]
fn rd_u64(d: &[u8], o: usize) -> u64 {
    if o + 8 <= d.len() {
        u64::from_le_bytes(d[o..o + 8].try_into().unwrap_or([0; 8]))
    } else {
        0
    }
}
