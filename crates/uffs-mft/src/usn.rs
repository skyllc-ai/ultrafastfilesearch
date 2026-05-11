// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! # USN Journal Integration (M5 Optimization)
//!
//! This module provides USN (Update Sequence Number) Journal integration for
//! incremental index updates. Instead of rescanning the entire MFT, we can
//! query the USN Journal for changes since the last index build.
//!
//! ## Windows API
//!
//! - `FSCTL_QUERY_USN_JOURNAL` - Get journal info (ID, first/next USN)
//! - `FSCTL_READ_USN_JOURNAL` - Read changes since a given USN
//!
//! ## Change Types
//!
//! - `USN_REASON_FILE_CREATE` - New file created
//! - `USN_REASON_FILE_DELETE` - File deleted
//! - `USN_REASON_RENAME_NEW_NAME` - File renamed
//! - `USN_REASON_DATA_EXTEND/TRUNCATE` - File size changed
//! - `USN_REASON_BASIC_INFO_CHANGE` - Timestamps changed

use std::collections::HashMap;

/// USN Journal information returned by `FSCTL_QUERY_USN_JOURNAL`.
#[derive(Debug, Clone)]
pub struct UsnJournalInfo {
    /// Unique identifier for this journal instance
    pub journal_id: u64,
    /// First valid USN in the journal
    pub first_usn: i64,
    /// Next USN to be assigned
    pub next_usn: i64,
    /// Lowest valid USN (may differ from `first_usn`)
    pub lowest_valid_usn: i64,
    /// Maximum USN (journal size limit)
    pub max_usn: i64,
    /// Maximum size of the journal in bytes
    pub max_size: u64,
    /// Allocation delta (how much journal grows)
    pub allocation_delta: u64,
}

/// A single USN Journal change record.
#[derive(Debug, Clone)]
pub struct UsnRecord {
    /// File Reference Number (FRS)
    pub frs: u64,
    /// Parent directory FRS
    pub parent_frs: u64,
    /// USN of this record
    pub usn: i64,
    /// Reason flags (bitmask of `USN_REASON_*`)
    pub reason: u32,
    /// File attributes
    pub file_attributes: u32,
    /// Filename
    pub filename: String,
}

/// USN reason flags (from Windows SDK).
pub mod reason {
    /// Data in the default data stream was overwritten.
    pub const DATA_OVERWRITE: u32 = 0x0000_0001;
    /// Data in the default data stream was extended.
    pub const DATA_EXTEND: u32 = 0x0000_0002;
    /// Data in the default data stream was truncated.
    pub const DATA_TRUNCATION: u32 = 0x0000_0004;
    /// Data in a named data stream was overwritten.
    pub const NAMED_DATA_OVERWRITE: u32 = 0x0000_0010;
    /// Data in a named data stream was extended.
    pub const NAMED_DATA_EXTEND: u32 = 0x0000_0020;
    /// Data in a named data stream was truncated.
    pub const NAMED_DATA_TRUNCATION: u32 = 0x0000_0040;
    /// A new file or directory was created.
    pub const FILE_CREATE: u32 = 0x0000_0100;
    /// A file or directory was deleted.
    pub const FILE_DELETE: u32 = 0x0000_0200;
    /// Extended attributes were changed.
    pub const EA_CHANGE: u32 = 0x0000_0400;
    /// Security descriptor was changed.
    pub const SECURITY_CHANGE: u32 = 0x0000_0800;
    /// File or directory was renamed (old name).
    pub const RENAME_OLD_NAME: u32 = 0x0000_1000;
    /// File or directory was renamed (new name).
    pub const RENAME_NEW_NAME: u32 = 0x0000_2000;
    /// Indexable content was changed.
    pub const INDEXABLE_CHANGE: u32 = 0x0000_4000;
    /// Basic file attributes were changed.
    pub const BASIC_INFO_CHANGE: u32 = 0x0000_8000;
    /// Hard link was added or removed.
    pub const HARD_LINK_CHANGE: u32 = 0x0001_0000;
    /// Compression state was changed.
    pub const COMPRESSION_CHANGE: u32 = 0x0002_0000;
    /// Encryption state was changed.
    pub const ENCRYPTION_CHANGE: u32 = 0x0004_0000;
    /// Object ID was changed.
    pub const OBJECT_ID_CHANGE: u32 = 0x0008_0000;
    /// Reparse point was changed.
    pub const REPARSE_POINT_CHANGE: u32 = 0x0010_0000;
    /// Named data stream was added or removed.
    pub const STREAM_CHANGE: u32 = 0x0020_0000;
    /// Transacted change.
    pub const TRANSACTED_CHANGE: u32 = 0x0040_0000;
    /// Integrity state was changed.
    pub const INTEGRITY_CHANGE: u32 = 0x0080_0000;
    /// File handle was closed (final record for a change).
    pub const CLOSE: u32 = 0x8000_0000;
}

/// Categorized change type for easier processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// File was created
    Created,
    /// File was deleted
    Deleted,
    /// File was renamed (new name)
    Renamed,
    /// File size changed
    SizeChanged,
    /// File metadata changed (timestamps, attributes)
    MetadataChanged,
    /// Other change (not directly relevant to index)
    Other,
}

impl UsnRecord {
    /// Categorizes this USN record into a `ChangeType`.
    #[must_use]
    pub const fn change_type(&self) -> ChangeType {
        if self.reason & reason::FILE_CREATE != 0 {
            ChangeType::Created
        } else if self.reason & reason::FILE_DELETE != 0 {
            ChangeType::Deleted
        } else if self.reason & reason::RENAME_NEW_NAME != 0 {
            ChangeType::Renamed
        } else if self.reason & (reason::DATA_EXTEND | reason::DATA_TRUNCATION) != 0 {
            ChangeType::SizeChanged
        } else if self.reason & reason::BASIC_INFO_CHANGE != 0 {
            ChangeType::MetadataChanged
        } else {
            ChangeType::Other
        }
    }

    /// Returns true if this is a "close" record (final record for a change).
    #[must_use]
    pub const fn is_close(&self) -> bool {
        self.reason & reason::CLOSE != 0
    }
}

/// Aggregated changes for a single file (consolidates multiple USN records).
// These bools represent independent change flags from USN journal records.
// Using a bitflags pattern would add complexity without benefit for this DTO.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent change flags from USN journal records"
)]
#[derive(Debug, Clone, Default)]
pub struct FileChange {
    /// File Reference Number
    pub frs: u64,
    /// Parent directory FRS (latest)
    pub parent_frs: u64,
    /// Filename (latest)
    pub filename: String,
    /// Was the file created?
    pub created: bool,
    /// Was the file deleted?
    pub deleted: bool,
    /// Was the file renamed?
    pub renamed: bool,
    /// Did the file size change?
    pub size_changed: bool,
    /// Did metadata change?
    pub metadata_changed: bool,
}

/// Aggregates multiple USN records into per-file changes.
#[must_use]
pub fn aggregate_changes(records: &[UsnRecord]) -> HashMap<u64, FileChange> {
    let mut changes: HashMap<u64, FileChange> = HashMap::new();
    for record in records {
        let entry = changes.entry(record.frs).or_insert_with(|| FileChange {
            frs: record.frs,
            ..Default::default()
        });
        entry.parent_frs = record.parent_frs;
        if !record.filename.is_empty() {
            entry.filename.clone_from(&record.filename);
        }
        match record.change_type() {
            ChangeType::Created => entry.created = true,
            ChangeType::Deleted => entry.deleted = true,
            ChangeType::Renamed => entry.renamed = true,
            ChangeType::SizeChanged => entry.size_changed = true,
            ChangeType::MetadataChanged => entry.metadata_changed = true,
            ChangeType::Other => {}
        }
    }
    changes
}

// Re-export platform-specific functions
#[cfg(windows)]
pub use windows_impl::{query_usn_journal, read_targeted_frs_records, read_usn_journal};

/// Windows-specific USN journal helpers backed by `FSCTL_QUERY_USN_JOURNAL`,
/// `FSCTL_READ_USN_JOURNAL`, and targeted FRS reads via `CreateFileW` /
/// `DeviceIoControl`.
///
/// All entries here are re-exported by the parent module under platform gating;
/// non-Windows targets see the no-op fallbacks in `usn::*` instead.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "FFI: Windows API calls (CreateFileW, DeviceIoControl, CloseHandle)"
)]
mod windows_impl {
    use core::mem::size_of;
    use core::ptr;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;

    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL};
    use windows::core::PCWSTR;
    use zerocopy::{FromBytes, Immutable, KnownLayout};

    use super::{UsnJournalInfo, UsnRecord};

    /// Mirror of the Win32 `USN_JOURNAL_DATA_V0` struct populated by
    /// `FSCTL_QUERY_USN_JOURNAL`.  Field order and layout match the
    /// `winioctl.h` definition.
    #[repr(C)]
    #[derive(Default)]
    struct UsnJournalDataV0 {
        /// Unique journal-instance identifier.
        usn_journal_id: u64,
        /// First valid USN in the journal.
        first_usn: i64,
        /// Next USN that will be assigned to a new record.
        next_usn: i64,
        /// Lowest USN still readable from the journal.
        lowest_valid_usn: i64,
        /// Largest USN the journal will accept before wrapping.
        max_usn: i64,
        /// Configured maximum journal size in bytes.
        maximum_size: u64,
        /// Allocation-growth quantum for the journal.
        allocation_delta: u64,
    }

    /// Mirror of the Win32 `READ_USN_JOURNAL_DATA_V0` input struct for
    /// `FSCTL_READ_USN_JOURNAL`.
    #[repr(C)]
    struct ReadUsnJournalDataV0 {
        /// USN to start reading from.
        start_usn: i64,
        /// Bitmask of `USN_REASON_*` flags to filter on.
        reason_mask: u32,
        /// Non-zero: return records only once their file is closed.
        return_only_on_close: u32,
        /// Wait timeout in 100ns units (0 = don't block).
        timeout: u64,
        /// Minimum bytes to wait for before returning (0 = return any).
        bytes_to_wait_for: u64,
        /// Journal identifier (must match `usn_journal_id` from query).
        usn_journal_id: u64,
    }

    /// Mirror of the Win32 `USN_RECORD_V2` fixed-size header.  The filename
    /// follows the header starting at `file_name_offset`.
    #[repr(C, packed)]
    #[derive(Clone, Copy, FromBytes, Immutable, KnownLayout)]
    struct UsnRecordV2Header {
        /// Total record length including trailing filename (bytes).
        record_length: u32,
        /// Major version of this record format.
        major_version: u16,
        /// Minor version of this record format.
        minor_version: u16,
        /// MFT file reference number (FRN) of the target file.
        file_reference_number: u64,
        /// FRN of the parent directory.
        parent_file_reference_number: u64,
        /// USN of this record.
        usn: i64,
        /// FILETIME of the change (100ns ticks since 1601-01-01 UTC).
        time_stamp: i64,
        /// Bitmask of `USN_REASON_*` flags describing the change.
        reason: u32,
        /// Bitmask of `USN_SOURCE_*` flags (e.g. replication-driven edits).
        source_info: u32,
        /// Security-descriptor id (unused on most filesystems).
        security_id: u32,
        /// Windows file attributes (`FILE_ATTRIBUTE_*`).
        file_attributes: u32,
        /// Length of the trailing filename in bytes (UTF-16 LE).
        file_name_length: u16,
        /// Offset from the start of this record to the trailing filename.
        file_name_offset: u16,
    }

    /// Size of a `UsnJournalDataV0` in bytes — always fits in `u32`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`UsnJournalDataV0` is a fixed-layout C struct of 56 bytes; the cast is compile-time bounded and required for the Win32 ioctl size argument"
    )]
    const USN_JOURNAL_DATA_V0_SIZE: u32 = size_of::<UsnJournalDataV0>() as u32;
    /// Size of a `ReadUsnJournalDataV0` in bytes — always fits in `u32`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`ReadUsnJournalDataV0` is a fixed-layout C struct of 40 bytes; the cast is compile-time bounded and required for the Win32 ioctl size argument"
    )]
    const READ_USN_JOURNAL_DATA_V0_SIZE: u32 = size_of::<ReadUsnJournalDataV0>() as u32;

    /// Open a `\\.\X:` volume handle with read access for USN-journal ioctls.
    fn open_volume_handle(volume: char) -> Result<HANDLE, std::io::Error> {
        let path = format!("\\\\.\\{volume}:");
        let wide: Vec<u16> = OsStr::new(&path)
            .encode_wide()
            .chain(core::iter::once(0))
            .collect();
        // USN Journal operations require GENERIC_READ access
        // SAFETY: `wide` is a NUL-terminated UTF-16 path buffer that lives for the
        // duration of the call, optional parameters are `None`, and ownership of any
        // returned handle is transferred to the caller.
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        };
        match handle {
            Ok(raw) if raw != INVALID_HANDLE_VALUE => Ok(raw),
            Ok(_) => Err(std::io::Error::last_os_error()),
            Err(err) => Err(std::io::Error::from_raw_os_error(err.code().0)),
        }
    }

    /// Close a USN-journal volume handle, logging (but not propagating) any
    /// failure from the Win32 `CloseHandle` call.  The handle is treated as
    /// consumed on return regardless of outcome.
    #[expect(
        unsafe_code,
        reason = "CloseHandle is an FFI call; caller guarantees handle validity"
    )]
    fn close_volume_handle(handle: HANDLE) {
        // SAFETY: caller passed a HANDLE returned from `open_volume_handle`
        // which has not yet been closed.
        if let Err(err) = unsafe { CloseHandle(handle) } {
            tracing::debug!(err = ?err, "CloseHandle failed in USN journal path");
        }
    }

    /// Queries the USN Journal for a volume.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if opening the volume handle or issuing
    /// `FSCTL_QUERY_USN_JOURNAL` fails — typically `ERROR_JOURNAL_NOT_ACTIVE`
    /// when the NTFS USN journal has not been enabled for the volume, or
    /// `ERROR_ACCESS_DENIED` when the caller is not elevated.
    pub fn query_usn_journal(volume: char) -> Result<UsnJournalInfo, std::io::Error> {
        let handle = open_volume_handle(volume)?;
        let mut journal_data = UsnJournalDataV0::default();
        let mut bytes_returned: u32 = 0;
        // SAFETY: `handle` is a live volume handle, the output buffer points to
        // writable `UsnJournalDataV0` storage, and `bytes_returned` is a valid
        // out-parameter for the duration of the call.
        let result = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_QUERY_USN_JOURNAL,
                None,                                          // Input buffer
                0,                                             // Input buffer size
                Some(ptr::from_mut(&mut journal_data).cast()), // Output buffer
                USN_JOURNAL_DATA_V0_SIZE,                      // Output buffer size
                Some(&raw mut bytes_returned),                 // Bytes returned
                None,                                          // Overlapped
            )
        };
        close_volume_handle(handle);
        if result.is_err() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(UsnJournalInfo {
            journal_id: journal_data.usn_journal_id,
            first_usn: journal_data.first_usn,
            next_usn: journal_data.next_usn,
            lowest_valid_usn: journal_data.lowest_valid_usn,
            max_usn: journal_data.max_usn,
            max_size: journal_data.maximum_size,
            allocation_delta: journal_data.allocation_delta,
        })
    }

    /// Reads all USN Journal records starting from a given USN.
    ///
    /// Loops the `FSCTL_READ_USN_JOURNAL` ioctl until all changes are consumed,
    /// preventing data loss on busy volumes where a single 64KB buffer would
    /// only return a subset of changes.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the volume handle cannot be opened or
    /// `FSCTL_READ_USN_JOURNAL` fails for any iteration of the
    /// read-loop. `ERROR_JOURNAL_ENTRY_DELETED` is surfaced unchanged so
    /// callers can decide whether to rebuild their checkpoint.
    pub fn read_usn_journal(
        volume: char,
        journal_id: u64,
        start_usn: i64,
    ) -> Result<(Vec<UsnRecord>, i64), std::io::Error> {
        let handle = open_volume_handle(volume)?;
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut all_records = Vec::new();
        let mut current_usn = start_usn;

        loop {
            let read_data = ReadUsnJournalDataV0 {
                start_usn: current_usn,
                reason_mask: 0xFFFF_FFFF,
                return_only_on_close: 0,
                timeout: 0,
                bytes_to_wait_for: 0,
                usn_journal_id: journal_id,
            };
            let mut bytes_returned: u32 = 0;
            // u32 truncation is safe: buffer is a fixed 64 KiB allocation
            // (`6_4 * 1024`), well under u32::MAX.
            let buffer_size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
            // SAFETY: `handle` is a live volume handle, `read_data` and `buffer`
            // provide valid input/output storage for the advertised byte counts,
            // and `bytes_returned` is a valid out-parameter for the call.
            let result = unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_READ_USN_JOURNAL,
                    Some(ptr::from_ref(&read_data).cast()),
                    READ_USN_JOURNAL_DATA_V0_SIZE,
                    Some(buffer.as_mut_ptr().cast()),
                    buffer_size,
                    Some(&raw mut bytes_returned),
                    None,
                )
            };
            if result.is_err() {
                close_volume_handle(handle);
                return Err(std::io::Error::last_os_error());
            }
            // `size_of::<i64>()` is 8, hard-coded as a `u32` literal here.
            if bytes_returned < 8_u32 {
                close_volume_handle(handle);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "FSCTL_READ_USN_JOURNAL returned fewer than 8 bytes",
                ));
            }

            // First 8 bytes of output = next USN to continue from
            let bytes_returned_usize = bytes_returned as usize; // u32→usize is lossless
            let next_usn_slice = buffer.get(..8).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "USN journal buffer shorter than 8 bytes",
                )
            })?;
            let mut next_usn_bytes = [0_u8; 8];
            next_usn_bytes.copy_from_slice(next_usn_slice);
            let next_usn = i64::from_le_bytes(next_usn_bytes);

            // Parse records from this batch
            let mut offset = 8_usize;
            let mut batch_count = 0_usize;
            while offset + size_of::<UsnRecordV2Header>() <= bytes_returned_usize {
                let Some(record_slice) = buffer.get(offset..bytes_returned_usize) else {
                    break;
                };
                let Ok((header, _)) = UsnRecordV2Header::read_from_prefix(record_slice) else {
                    break;
                };
                if header.record_length == 0 {
                    break;
                }
                let name_start = offset + header.file_name_offset as usize;
                let name_end = name_start + header.file_name_length as usize;
                let filename = buffer
                    .get(name_start..name_end)
                    .filter(|_| name_end <= bytes_returned_usize)
                    .map_or_else(String::new, |name_bytes| {
                        let name_u16: Vec<u16> = name_bytes
                            .chunks_exact(2)
                            .map(|pair| {
                                u16::from_le_bytes([
                                    *pair.first().unwrap_or(&0),
                                    *pair.get(1).unwrap_or(&0),
                                ])
                            })
                            .collect();
                        String::from_utf16_lossy(&name_u16)
                    });
                all_records.push(UsnRecord {
                    frs: header.file_reference_number & 0x0000_FFFF_FFFF_FFFF,
                    parent_frs: header.parent_file_reference_number & 0x0000_FFFF_FFFF_FFFF,
                    usn: header.usn,
                    reason: header.reason,
                    file_attributes: header.file_attributes,
                    filename,
                });
                offset += header.record_length as usize;
                batch_count += 1;
            }

            // If no records were returned in this batch, we've consumed
            // everything — stop looping.
            if batch_count == 0 || next_usn == current_usn {
                current_usn = next_usn;
                break;
            }
            current_usn = next_usn;
        }

        close_volume_handle(handle);
        Ok((all_records, current_usn))
    }

    /// Reads specific MFT records by FRS and re-parses them into the index.
    ///
    /// This performs **targeted reads** for individual FRS values identified by
    /// the USN journal, giving each record full data (size, timestamps, flags,
    /// attributes, streams) instead of the incomplete placeholder data that USN
    /// alone provides.
    ///
    /// # Performance
    ///
    /// Each read is a single seek + 1-4KB read. For 1000 records on SSD this
    /// takes ~2ms total. The cost is dominated by I/O latency on HDD (~5ms per
    /// seek).
    ///
    /// # Errors
    ///
    /// Returns an error if the volume cannot be opened or extents cannot be
    /// retrieved. Individual record read failures are logged and skipped.
    pub fn read_targeted_frs_records(
        volume: &crate::platform::VolumeHandle,
        index: &mut crate::index::MftIndex,
        frs_list: &[u64],
    ) -> Result<usize, crate::MftError> {
        use crate::io::MftRecordReader;

        if frs_list.is_empty() {
            return Ok(0);
        }

        // Build extent map for the MFT so we can seek to specific records
        let extents = volume.get_mft_extents()?;
        let extent_map = crate::io::MftExtentMap::new(
            extents,
            volume.volume_data().bytes_per_cluster,
            volume.volume_data().bytes_per_file_record_segment,
        );

        let mut reader = MftRecordReader::new_with_extents(extent_map);
        let handle = volume.raw_handle();
        let mut success_count = 0_usize;

        // Collect extension FRS numbers discovered from $ATTRIBUTE_LIST
        // attributes. Processed in a second pass after all base records.
        let mut extension_frs: Vec<u64> = Vec::new();

        for &frs in frs_list {
            success_count +=
                read_one_targeted_record(&mut reader, handle, index, frs, Some(&mut extension_frs));
        }

        // Second pass: read extension records discovered from $ATTRIBUTE_LIST.
        // `parse_record_to_index` detects extension records (base_frs != 0)
        // and dispatches them to `parse_extension_to_index` automatically.
        extension_frs.sort_unstable();
        extension_frs.dedup();
        if !extension_frs.is_empty() {
            tracing::debug!(
                count = extension_frs.len(),
                "📎 Reading extension MFT records"
            );
        }
        for ext_frs in &extension_frs {
            success_count += read_one_targeted_record(&mut reader, handle, index, *ext_frs, None);
        }

        Ok(success_count)
    }

    /// Read one MFT record by FRS, apply fixup, optionally scan
    /// `$ATTRIBUTE_LIST` for extension FRSes, and parse the result into
    /// `index`.
    ///
    /// `extension_frs_out` is `Some(_)` for the base-record pass (so
    /// extensions are discovered) and `None` for the extension-record pass
    /// (where extension scanning is unnecessary).
    ///
    /// Returns `1` on a successful parse, `0` on any read / fixup / parse
    /// failure (failures are logged at trace level and skipped to keep the
    /// caller's loop simple).
    fn read_one_targeted_record(
        reader: &mut crate::io::MftRecordReader,
        handle: HANDLE,
        index: &mut crate::index::MftIndex,
        frs: u64,
        extension_frs_out: Option<&mut Vec<u64>>,
    ) -> usize {
        use crate::parse::{apply_fixup, parse_record_to_index};

        match reader.read_record(handle, frs) {
            Ok(raw_data) => {
                let mut buf = raw_data.to_vec();
                if !apply_fixup(&mut buf) {
                    return 0;
                }
                if let Some(out) = extension_frs_out {
                    scan_attribute_list_extensions(&buf, frs, out);
                }
                usize::from(parse_record_to_index(&buf, frs, index))
            }
            Err(err) => {
                tracing::trace!(
                    frs,
                    error = %err,
                    "⚠️ Targeted MFT read failed for FRS (skipping)"
                );
                0
            }
        }
    }

    /// Scans a base MFT record's attributes for `$ATTRIBUTE_LIST` (type 0x20)
    /// and extracts the FRS numbers of any extension records it references.
    ///
    /// Hoisted to module scope (instead of being nested inside
    /// [`read_targeted_frs_records`]) so the function item is not declared
    /// after statements in the caller — required by
    /// `clippy::items_after_statements`.
    fn scan_attribute_list_extensions(data: &[u8], base_frs: u64, out: &mut Vec<u64>) {
        use zerocopy::FromBytes as _;

        use crate::ntfs::{
            AttributeListEntry, AttributeRecordHeader, AttributeType, FileRecordSegmentHeader,
        };

        if data.len() < size_of::<FileRecordSegmentHeader>() {
            return;
        }
        let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
            return;
        };

        let mut offset = header.first_attribute_offset as usize;
        let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

        while offset + size_of::<AttributeRecordHeader>() <= max_offset {
            let Some(attr_bytes) = data.get(offset..) else {
                break;
            };
            let Ok((attr, _)) = AttributeRecordHeader::read_from_prefix(attr_bytes) else {
                break;
            };
            if attr.type_code == AttributeType::End as u32 {
                break;
            }
            if attr.length == 0 || offset + attr.length as usize > max_offset {
                break;
            }

            if attr.type_code == AttributeType::AttributeList as u32 && attr.is_non_resident == 0 {
                // Resident $ATTRIBUTE_LIST — parse entries
                let val_offset_raw = data
                    .get(offset + 20..offset + 22)
                    .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
                    .map_or(0, u16::from_le_bytes) as usize;
                let val_length = data
                    .get(offset + 16..offset + 20)
                    .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                    .map_or(0, u32::from_le_bytes) as usize;

                let list_start = offset + val_offset_raw;
                let list_end = core::cmp::min(list_start.saturating_add(val_length), data.len());

                let mut pos = list_start;
                while pos + size_of::<AttributeListEntry>() <= list_end {
                    let Some(entry_bytes) = data.get(pos..list_end) else {
                        break;
                    };
                    let Ok((entry, _)) = AttributeListEntry::read_from_prefix(entry_bytes) else {
                        break;
                    };
                    if usize::from(entry.length) < size_of::<AttributeListEntry>() {
                        break;
                    }
                    let target = entry.target_frs();
                    if target != base_frs && target != 0 {
                        out.push(target);
                    }
                    pos += entry.length as usize;
                }
            }

            offset += attr.length as usize;
        }
    }
}

/// Queries the USN Journal for a volume (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(windows))]
#[expect(
    clippy::std_instead_of_core,
    reason = "core::io::Error is not yet stable — see rust-lang/rust#103765. \
              Remove this expect once `error_in_core` stabilises."
)]
pub fn query_usn_journal(_volume: char) -> Result<UsnJournalInfo, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "USN Journal is only available on Windows",
    ))
}

/// Reads USN Journal records starting from a given USN (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(windows))]
#[expect(
    clippy::std_instead_of_core,
    reason = "core::io::Error is not yet stable — see rust-lang/rust#103765. \
              Remove this expect once `error_in_core` stabilises."
)]
pub fn read_usn_journal(
    _volume: char,
    _journal_id: u64,
    _start_usn: i64,
) -> Result<(Vec<UsnRecord>, i64), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "USN Journal is only available on Windows",
    ))
}
