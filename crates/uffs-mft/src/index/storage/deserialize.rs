// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Binary deserialization for `MftIndex` snapshots.
use core::mem::size_of;

use super::IndexHeader;
use crate::index::{
    ChildInfo, ExtensionTable, FileRecord, IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex,
    MftStats, NO_ENTRY, SizeInfo, StandardInfo,
};
use crate::platform::DriveLetter;
impl MftIndex {
    /// Deserializes an index from a byte slice.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is corrupted or incompatible.
    // This function intentionally keeps sequential binary parsing together for
    // performance/maintainability; splitting would add call overhead and make the format harder
    // to follow. The u64->usize casts are safe on the 64-bit Windows target, and complexity comes
    // from version-conditional field reads (v3/v4/v5/v6).
    #[expect(
        clippy::too_many_lines,
        reason = "binary deserialization has many sequential field reads"
    )]
    #[expect(
        clippy::cognitive_complexity,
        reason = "version-conditional fields (v3/v4/v5/v6)"
    )]
    #[expect(
        clippy::items_after_statements,
        reason = "profiling variables must precede items for timing accuracy"
    )]
    pub fn deserialize(data: &[u8]) -> Result<(Self, IndexHeader), &'static str> {
        let profile = std::env::var_os("UFFS_CACHE_PROFILE").is_some();
        let t_deser_start = std::time::Instant::now();

        const FRS_TO_IDX_ENTRY_BYTES: usize = 4;
        const LINK_INFO_BYTES: usize = 20;
        const STREAM_INFO_BYTES: usize = 29;
        const CHILD_INFO_BYTES: usize = 14;
        const EXTENSION_ENTRY_BYTES: usize = 16;
        const EXTENSION_ENTRY_TRAILER_BYTES: usize = 12;

        /// # Errors
        ///
        /// Returns the provided error string if the count overflows `usize`
        /// or the multiplication overflows.
        fn checked_section_bytes(
            count: u64,
            entry_size: usize,
            too_large_error: &'static str,
        ) -> Result<usize, &'static str> {
            let count_usize = usize::try_from(count).map_err(|_err| too_large_error)?;
            count_usize.checked_mul(entry_size).ok_or(too_large_error)
        }

        /// # Errors
        ///
        /// Returns the provided error string if `required` bytes exceed the
        /// remaining data.
        const fn ensure_remaining(
            data_len: usize,
            pos: usize,
            required: usize,
            exceeds_error: &'static str,
        ) -> Result<(), &'static str> {
            if required > data_len.saturating_sub(pos) {
                return Err(exceeds_error);
            }
            Ok(())
        }

        if data.len() < 96 {
            return Err("Data too short for header");
        }

        let mut pos = 0;

        // Helper macro to read bytes safely
        macro_rules! read_u8 {
            () => {{
                let val = *data.get(pos).ok_or("Unexpected end of data")?;
                pos += 1;
                val
            }};
        }
        macro_rules! read_u16 {
            () => {{
                let bytes: [u8; 2] = data
                    .get(pos..pos + 2)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_err| "Invalid u16 slice")?;
                let val = u16::from_le_bytes(bytes);
                pos += 2;
                val
            }};
        }
        macro_rules! read_u32 {
            () => {{
                let bytes: [u8; 4] = data
                    .get(pos..pos + 4)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_err| "Invalid u32 slice")?;
                let val = u32::from_le_bytes(bytes);
                pos += 4;
                val
            }};
        }
        macro_rules! read_u64 {
            () => {{
                let bytes: [u8; 8] = data
                    .get(pos..pos + 8)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_err| "Invalid u64 slice")?;
                let val = u64::from_le_bytes(bytes);
                pos += 8;
                val
            }};
        }
        macro_rules! read_i64 {
            () => {{
                let bytes: [u8; 8] = data
                    .get(pos..pos + 8)
                    .ok_or("Unexpected end of data")?
                    .try_into()
                    .map_err(|_err| "Invalid i64 slice")?;
                let val = i64::from_le_bytes(bytes);
                pos += 8;
                val
            }};
        }

        // Read header
        let mut magic = [0_u8; 8];
        magic.copy_from_slice(data.get(pos..pos + 8).ok_or("Unexpected end of data")?);
        pos += 8;

        let version = read_u32!();
        // Wire format: u32-LE drive byte, originally written as `char as
        // u32` (5a) or `DriveLetter::as_byte() as u32` (5b+).  Both
        // produce the same 4 bytes for ASCII A..=Z, so v0..=v13 files
        // round-trip cleanly.
        let volume_raw = read_u32!();
        let volume = char::from_u32(volume_raw)
            .and_then(|ch| DriveLetter::parse(ch).ok())
            .ok_or("Invalid volume drive letter")?;
        let volume_serial = read_u64!();
        let usn_journal_id = read_u64!();
        let next_usn = crate::usn::Usn::new(read_i64!());
        let created_at = read_u64!();
        let record_count = read_u64!();
        let names_size = read_u64!();
        let links_count = read_u64!();
        let streams_count = read_u64!();
        let children_count = read_u64!();
        // v12+: build_epoch (Unix microseconds); 0 for older versions.
        let build_epoch = if version >= 12 { read_u64!() } else { 0 };

        let header = IndexHeader {
            magic,
            version,
            volume,
            volume_serial,
            usn_journal_id,
            next_usn,
            created_at,
            record_count,
            names_size,
            links_count,
            streams_count,
            children_count,
            build_epoch,
        };

        header.validate()?;

        let t_parse_start = std::time::Instant::now();

        // Read frs_to_idx table
        let frs_to_idx_len = read_u64!();
        let frs_to_idx_bytes = checked_section_bytes(
            frs_to_idx_len,
            FRS_TO_IDX_ENTRY_BYTES,
            "FRS table too large",
        )?;
        ensure_remaining(
            data.len(),
            pos,
            frs_to_idx_bytes,
            "FRS table exceeds remaining data",
        )?;
        let frs_slice = data
            .get(pos..pos + frs_to_idx_bytes)
            .ok_or("FRS table truncated")?;
        let frs_to_idx: Vec<u32> = super::aligned_vec_from_bytes(frs_slice);
        pos += frs_to_idx_bytes;

        // Read records — v10 uses bytemuck bulk copy (240B/rec),
        //                 v3-v9 use field-by-field (variable size/rec).
        let record_size_bytes = match version {
            3 => 121,
            4 => 157,
            5 => 181,
            6 => 185,
            7 => 193,
            8 | 9 => 195,
            10..=13 => 240, // Pod layout with padding
            _ => return Err("Unsupported index version"),
        };
        let record_bytes =
            checked_section_bytes(record_count, record_size_bytes, "Record section too large")?;
        ensure_remaining(
            data.len(),
            pos,
            record_bytes,
            "Record section exceeds remaining data",
        )?;
        // v10+: alignment-safe bulk copy into aligned Vec<FileRecord>
        let records = if version >= 10 {
            let slice = data
                .get(pos..pos + record_bytes)
                .ok_or("Record section truncated")?;
            let recs: Vec<FileRecord> = super::aligned_vec_from_bytes(slice);
            pos += record_bytes;
            recs
        } else {
            // v3-v9: field-by-field deserialization
            let mut records = Vec::with_capacity(
                usize::try_from(record_count).map_err(|_err| "Record section too large")?,
            );
            for _ in 0..record_count {
                let frs = read_u64!();
                // Version 4+: sequence_number and namespace (read sequentially to avoid
                // unsequenced reads)
                let sequence_number = if version >= 4 { read_u16!() } else { 0 };
                let namespace = if version >= 4 { read_u8!() } else { 1 }; // Default: Win32
                let forensic_flags = if version >= 4 { read_u8!() } else { 0 }; // Version 7: renamed from reserved
                // Version 5+: LSN (Log File Sequence Number)
                let lsn = if version >= 5 { read_u64!() } else { 0 };
                // Version 6+: reparse_tag
                let reparse_tag = if version >= 6 { read_u32!() } else { 0 };
                // Version 7+: base_frs for extension records
                let base_frs = if version >= 7 { read_u64!() } else { 0 };
                // StandardInfo
                let created = read_i64!();
                let modified = read_i64!();
                let accessed = read_i64!();
                let mft_changed = read_i64!();
                let raw_flags = read_u32!();
                // v8 and earlier: flags used a remapped internal bit layout.
                // v9+: flags store raw NTFS FILE_ATTRIBUTE_* bits directly.
                let flags = if version <= 8 {
                    v8_flags_to_raw_ntfs(raw_flags)
                } else {
                    raw_flags
                };
                // Version 5+: NTFS 3.0+ forensic fields
                let usn = if version >= 5 { read_u64!() } else { 0 };
                let security_id = if version >= 5 { read_u32!() } else { 0 };
                let owner_id = if version >= 5 { read_u32!() } else { 0 };
                // Counts
                let name_count = read_u16!();
                let rec_stream_count = read_u16!();
                // Version 8+: total_stream_count for full tree-metrics accounting
                // For older versions, default to stream_count (user-visible = total)
                let total_stream_count = if version >= 8 {
                    read_u16!()
                } else {
                    rec_stream_count
                };
                let first_child = read_u32!();
                // first_name (LinkInfo)
                let link_next_entry = read_u32!();
                let link_name_offset = read_u32!();
                let link_name_meta = read_u32!();
                let link_parent_frs = read_u64!();
                // first_stream (IndexStreamInfo)
                let stream_size_length = read_u64!();
                let stream_size_allocated = read_u64!();
                let stream_next_entry = read_u32!();
                let stream_name_offset = read_u32!();
                let stream_name_meta = read_u32!();
                let stream_flags = read_u8!();
                // Tree metrics (Version 3+)
                let descendants = if version >= 3 { read_u32!() } else { 0 };
                let treesize = if version >= 3 { read_u64!() } else { 0 };
                let tree_allocated = if version >= 3 { read_u64!() } else { 0 };
                // $FILE_NAME timestamps (Version 4+, read sequentially)
                let fn_created = if version >= 4 { read_i64!() } else { 0 };
                let fn_modified = if version >= 4 { read_i64!() } else { 0 };
                let fn_accessed = if version >= 4 { read_i64!() } else { 0 };
                let fn_mft_changed = if version >= 4 { read_i64!() } else { 0 };

                records.push(FileRecord {
                    // Legacy v3-v9 deserialization boundary: raw `u64` from
                    // disk is lifted into typed `Frs` at the struct-literal
                    // construction site.  v10+ Pod paths above are
                    // bit-identical via `repr(transparent)`.
                    frs: crate::frs::Frs::new(frs),
                    sequence_number,
                    namespace,
                    forensic_flags,
                    _pad0: [0; 4],
                    lsn,
                    reparse_tag,
                    _pad1: [0; 4],
                    base_frs: crate::frs::Frs::new(base_frs),
                    stdinfo: StandardInfo {
                        created,
                        modified,
                        accessed,
                        mft_changed,
                        flags,
                        _pad0: [0; 4],
                        usn,
                        security_id,
                        owner_id,
                    },
                    name_count,
                    stream_count: rec_stream_count,
                    total_stream_count,
                    _pad2: [0; 2],
                    first_internal_stream: NO_ENTRY,
                    first_child,
                    first_name: LinkInfo {
                        next_entry: link_next_entry,
                        name: IndexNameRef {
                            offset: link_name_offset,
                            meta: link_name_meta,
                        },
                        _pad0: [0; 4],
                        parent_frs: crate::frs::ParentFrs::new(link_parent_frs),
                    },
                    first_stream: IndexStreamInfo {
                        size: SizeInfo {
                            length: stream_size_length,
                            allocated: stream_size_allocated,
                        },
                        next_entry: stream_next_entry,
                        name: IndexNameRef {
                            offset: stream_name_offset,
                            meta: stream_name_meta,
                        },
                        flags: stream_flags,
                        _pad0: [0; 3],
                    },
                    fn_created,
                    fn_modified,
                    fn_accessed,
                    fn_mft_changed,
                    descendants,
                    _pad3: [0; 4],
                    treesize,
                    tree_allocated,
                    internal_streams_size: 0,
                    internal_streams_allocated: 0,
                });
            }
            records
        }; // end v3-v9 else block

        // Read names
        let names_len = usize::try_from(names_size).map_err(|_err| "Names section too large")?;
        ensure_remaining(
            data.len(),
            pos,
            names_len,
            "Names section exceeds remaining data",
        )?;
        let names_end = pos
            .checked_add(names_len)
            .ok_or("Names section too large")?;
        let names_bytes = data.get(pos..names_end).ok_or("Unexpected end of data")?;
        let names = String::from_utf8(names_bytes.to_vec())
            .map_err(|_utf8_err| "Invalid UTF-8 in names")?;
        pos = names_end;

        // Read links (overflow links)
        let links = if version >= 10 {
            // v10: Pod layout — 24 bytes per LinkInfo (with padding)
            let link_pod_bytes = checked_section_bytes(
                links_count,
                size_of::<LinkInfo>(),
                "Links section too large",
            )?;
            ensure_remaining(
                data.len(),
                pos,
                link_pod_bytes,
                "Links section exceeds remaining data",
            )?;
            let slice = data
                .get(pos..pos + link_pod_bytes)
                .ok_or("Links truncated")?;
            let result: Vec<LinkInfo> = super::aligned_vec_from_bytes(slice);
            pos += link_pod_bytes;
            result
        } else {
            let link_bytes =
                checked_section_bytes(links_count, LINK_INFO_BYTES, "Links section too large")?;
            ensure_remaining(
                data.len(),
                pos,
                link_bytes,
                "Links section exceeds remaining data",
            )?;
            let mut links = Vec::with_capacity(
                usize::try_from(links_count).map_err(|_err| "Links section too large")?,
            );
            for _ in 0..links_count {
                let next_entry = read_u32!();
                let name_offset = read_u32!();
                let name_meta = read_u32!();
                let parent_frs = read_u64!();
                links.push(LinkInfo {
                    next_entry,
                    name: IndexNameRef {
                        offset: name_offset,
                        meta: name_meta,
                    },
                    _pad0: [0; 4],
                    parent_frs: crate::frs::ParentFrs::new(parent_frs),
                });
            }
            links
        };

        // Read streams (overflow streams)
        let streams = if version >= 10 {
            // v10: Pod layout — 32 bytes per IndexStreamInfo (with padding)
            let stream_pod_bytes = checked_section_bytes(
                streams_count,
                size_of::<IndexStreamInfo>(),
                "Streams section too large",
            )?;
            ensure_remaining(
                data.len(),
                pos,
                stream_pod_bytes,
                "Streams section exceeds remaining data",
            )?;
            let slice = data
                .get(pos..pos + stream_pod_bytes)
                .ok_or("Streams truncated")?;
            let result: Vec<IndexStreamInfo> = super::aligned_vec_from_bytes(slice);
            pos += stream_pod_bytes;
            result
        } else {
            let stream_bytes = checked_section_bytes(
                streams_count,
                STREAM_INFO_BYTES,
                "Streams section too large",
            )?;
            ensure_remaining(
                data.len(),
                pos,
                stream_bytes,
                "Streams section exceeds remaining data",
            )?;
            let mut streams = Vec::with_capacity(
                usize::try_from(streams_count).map_err(|_err| "Streams section too large")?,
            );
            for _ in 0..streams_count {
                let size_length = read_u64!();
                let size_allocated = read_u64!();
                let next_entry = read_u32!();
                let name_offset = read_u32!();
                let name_meta = read_u32!();
                let flags = read_u8!();
                streams.push(IndexStreamInfo {
                    size: SizeInfo {
                        length: size_length,
                        allocated: size_allocated,
                    },
                    next_entry,
                    name: IndexNameRef {
                        offset: name_offset,
                        meta: name_meta,
                    },
                    flags,
                    _pad0: [0; 3],
                });
            }
            streams
        };

        // Read children — v11+ Pod bulk copy, v3-v10 element-by-element
        let children = if version >= 11 {
            let child_pod_bytes = checked_section_bytes(
                children_count,
                size_of::<ChildInfo>(),
                "Children section too large",
            )?;
            ensure_remaining(
                data.len(),
                pos,
                child_pod_bytes,
                "Children section exceeds remaining data",
            )?;
            let slice = data
                .get(pos..pos + child_pod_bytes)
                .ok_or("Children truncated")?;
            let result: Vec<ChildInfo> = super::aligned_vec_from_bytes(slice);
            pos += child_pod_bytes;
            result
        } else {
            let child_bytes = checked_section_bytes(
                children_count,
                CHILD_INFO_BYTES,
                "Children section too large",
            )?;
            ensure_remaining(
                data.len(),
                pos,
                child_bytes,
                "Children section exceeds remaining data",
            )?;
            let mut children = Vec::with_capacity(
                usize::try_from(children_count).map_err(|_err| "Children section too large")?,
            );
            for _ in 0..children_count {
                let next_entry = read_u32!();
                let child_frs = read_u64!();
                let name_index = read_u16!();
                children.push(ChildInfo {
                    next_entry,
                    _pad0: [0; 4],
                    child_frs: crate::frs::Frs::new(child_frs),
                    name_index,
                    _pad1: [0; 6],
                });
            }
            children
        };

        // Read ExtensionTable
        let extension_count = read_u32!() as usize;
        let extension_entries = extension_count.saturating_sub(1);
        let min_extension_bytes = extension_entries
            .checked_mul(EXTENSION_ENTRY_BYTES)
            .ok_or("Extension table too large")?;
        ensure_remaining(
            data.len(),
            pos,
            min_extension_bytes,
            "Extension table exceeds remaining data",
        )?;
        let mut extensions = ExtensionTable::new();

        // Read each extension (starting from index 1, since 0 is NO_EXTENSION)
        for _ in 1..extension_count {
            // String length (u32)
            let str_len = read_u32!() as usize;
            let required = str_len
                .checked_add(EXTENSION_ENTRY_TRAILER_BYTES)
                .ok_or("Extension table too large")?;
            ensure_remaining(
                data.len(),
                pos,
                required,
                "Extension table exceeds remaining data",
            )?;
            let str_end = pos
                .checked_add(str_len)
                .ok_or("Extension table too large")?;

            // String bytes
            let str_bytes = data.get(pos..str_end).ok_or("Unexpected end of data")?;
            let ext_str = core::str::from_utf8(str_bytes)
                .map_err(|_e| "Invalid UTF-8 in extension string")?;
            pos = str_end;

            // Count (u32)
            let count = read_u32!();

            // Bytes (u64)
            let bytes = read_u64!();

            // Intern the extension and update counts/bytes
            let ext_id = extensions.intern(ext_str);

            // Set the counts and bytes directly
            let ext_idx = ext_id as usize;
            if let Some(count_slot) = extensions.counts.get_mut(ext_idx) {
                *count_slot = count;
            }
            if let Some(bytes_slot) = extensions.bytes.get_mut(ext_idx) {
                *bytes_slot = bytes;
            }
        }

        let parse_ms = t_parse_start.elapsed().as_millis();

        // ─── v10: Read ExtensionIndex CSR ─────────────────────────────
        let t_ext = std::time::Instant::now();
        let extension_index = if version >= 10 && pos + 4 <= data.len() {
            let offsets_count = read_u32!() as usize;
            if offsets_count > 0 {
                let offsets_bytes = offsets_count * 4;
                ensure_remaining(
                    data.len(),
                    pos,
                    offsets_bytes,
                    "ExtensionIndex offsets exceed remaining data",
                )?;
                let off_slice = data
                    .get(pos..pos + offsets_bytes)
                    .ok_or("ExtensionIndex offsets truncated")?;
                let ext_offsets: Vec<u32> = super::aligned_vec_from_bytes(off_slice);
                pos += offsets_bytes;

                let total_postings = ext_offsets.last().copied().unwrap_or(0) as usize;
                let postings_bytes = total_postings * 4;
                ensure_remaining(
                    data.len(),
                    pos,
                    postings_bytes,
                    "ExtensionIndex postings exceed remaining data",
                )?;
                let post_slice = data
                    .get(pos..pos + postings_bytes)
                    .ok_or("ExtensionIndex postings truncated")?;
                let ext_postings: Vec<u32> = super::aligned_vec_from_bytes(post_slice);
                // postings_bytes intentionally not consumed — this is the last section.
                // The variable exists for clarity in the sequential decode pattern.

                Some(super::super::ExtensionIndex {
                    offsets: ext_offsets,
                    postings: ext_postings,
                })
            } else {
                None
            }
        } else {
            None
        };
        let ext_idx_ms = t_ext.elapsed().as_millis();

        let mut index = Self {
            volume,
            records,
            frs_to_idx,
            names,
            links,
            streams,
            internal_streams: Vec::new(),
            children,
            stats: MftStats::new(),
            extensions,
            extension_index,
            forensic_mode: false,
            reserved_allocated_bytes: 0,
            build_epoch,
        };

        // Compute stats from loaded data
        let t_stats = std::time::Instant::now();
        index.recompute_stats();
        let stats_ms = t_stats.elapsed().as_millis();

        // If loading an old version (< 3) without tree metrics, recompute them
        let tree_ms = if version < 3 {
            tracing::debug!("Old index version {version} - recomputing tree metrics");
            let t_tree = std::time::Instant::now();
            index.compute_tree_metrics();
            t_tree.elapsed().as_millis()
        } else {
            0
        };

        // v9 and below: ExtensionIndex was not persisted — rebuild from records
        let ext_rebuild_ms = if index.extension_index.is_none() {
            let t_rebuild = std::time::Instant::now();
            index.build_extension_index();
            t_rebuild.elapsed().as_millis()
        } else {
            0
        };

        let total_deser_ms = t_deser_start.elapsed().as_millis();

        if profile {
            let ext_mode = if ext_rebuild_ms > 0 {
                "CSR_rebuild"
            } else {
                "CSR_load"
            };
            let ext_ms = if ext_rebuild_ms > 0 {
                ext_rebuild_ms
            } else {
                ext_idx_ms
            };
            tracing::debug!(
                target: "cache_profile",
                parse_ms = %parse_ms,
                stats_ms = %stats_ms,
                tree_ms = %tree_ms,
                ext_mode,
                ext_ms = %ext_ms,
                version,
                total_ms = %total_deser_ms,
                "mft_deserialize"
            );
        }

        Ok((index, header))
    }
}

/// Convert v8 (and earlier) remapped `StandardInfo` flags to raw NTFS
/// `FILE_ATTRIBUTE_*` bits.
///
/// Input: old v8 bit positions (frozen format values).
/// Output: raw NTFS constants via `StandardInfo::IS_*`.
const fn v8_flags_to_raw_ntfs(old: u32) -> u32 {
    let mut ntfs = 0_u32;
    // v8 bit position → raw NTFS constant
    if old & (1_u32 << 0_u32) != 0 {
        ntfs |= StandardInfo::IS_READONLY;
    }
    if old & (1_u32 << 1_u32) != 0 {
        ntfs |= StandardInfo::IS_ARCHIVE;
    }
    if old & (1_u32 << 2_u32) != 0 {
        ntfs |= StandardInfo::IS_SYSTEM;
    }
    if old & (1_u32 << 3_u32) != 0 {
        ntfs |= StandardInfo::IS_HIDDEN;
    }
    if old & (1_u32 << 4_u32) != 0 {
        ntfs |= StandardInfo::IS_OFFLINE;
    }
    if old & (1_u32 << 5_u32) != 0 {
        ntfs |= StandardInfo::IS_NOT_INDEXED;
    }
    if old & (1_u32 << 6_u32) != 0 {
        ntfs |= StandardInfo::IS_NO_SCRUB_DATA;
    }
    if old & (1_u32 << 7_u32) != 0 {
        ntfs |= StandardInfo::IS_INTEGRITY_STREAM;
    }
    if old & (1_u32 << 8_u32) != 0 {
        ntfs |= StandardInfo::IS_PINNED;
    }
    if old & (1_u32 << 9_u32) != 0 {
        ntfs |= StandardInfo::IS_UNPINNED;
    }
    if old & (1_u32 << 10_u32) != 0 {
        ntfs |= StandardInfo::IS_DIRECTORY;
    }
    if old & (1_u32 << 11_u32) != 0 {
        ntfs |= StandardInfo::IS_COMPRESSED;
    }
    if old & (1_u32 << 12_u32) != 0 {
        ntfs |= StandardInfo::IS_ENCRYPTED;
    }
    if old & (1_u32 << 13_u32) != 0 {
        ntfs |= StandardInfo::IS_SPARSE;
    }
    if old & (1_u32 << 14_u32) != 0 {
        ntfs |= StandardInfo::IS_REPARSE;
    }
    if old & (1_u32 << 15_u32) != 0 {
        ntfs |= StandardInfo::IS_TEMPORARY;
    }
    if old & (1_u32 << 16_u32) != 0 {
        ntfs |= StandardInfo::IS_VIRTUAL;
    }
    // Preserve DELETED_FLAG (bit 31) — internal USN marker, not an NTFS attribute
    if old & 0x8000_0000 != 0 {
        ntfs |= 0x8000_0000;
    }
    ntfs
}
