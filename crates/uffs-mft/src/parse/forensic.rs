// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Forensic parsing entry points and subparsers for special-record handling.

mod base;
mod extension;

use base::parse_base_record;
use extension::parse_extension_record;
use zerocopy::FromBytes as _;

use super::{ParseOptions, ParseResult, ParsedRecord};

/// Parses an MFT record with forensic options.
///
/// This function extends `parse_record_full` to support forensic analysis:
/// - `include_deleted`: Returns deleted records (`FRH_IN_USE` not set)
/// - `include_corrupt`: Returns records with corrupt fixup (handled by caller)
/// - `include_extensions`: Returns extension records as separate
///   `ParsedRecord`s
///
/// # Arguments
///
/// * `data` - The raw record data (after fixup, or raw if checking corrupt)
/// * `frs` - The File Record Segment number
/// * `options` - Forensic parsing options
/// * `is_corrupt` - True if fixup failed (set by caller)
///
/// # Returns
///
/// `ParseResult::Base` for all records matching options, or
/// `ParseResult::Skip`.
#[must_use]
pub fn parse_record_forensic(
    data: &[u8],
    frs: u64,
    options: ParseOptions,
    is_corrupt: bool,
) -> ParseResult {
    use core::mem::size_of;

    use crate::ntfs::{FileRecordSegmentHeader, file_reference_to_frs};

    if is_corrupt {
        if !options.include_corrupt {
            return ParseResult::Skip;
        }
        return ParseResult::Base(ParsedRecord {
            frs,
            name: format!("<CORRUPT:{frs}>"),
            is_corrupt: true,
            ..Default::default()
        });
    }

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return ParseResult::Skip;
    }

    let Ok((header, _)) = FileRecordSegmentHeader::read_from_prefix(data) else {
        return ParseResult::Skip;
    };

    let is_deleted = !header.is_in_use();
    if is_deleted && !options.include_deleted {
        return ParseResult::Skip;
    }

    let multi_sector_header = header.multi_sector_header;
    if !multi_sector_header.is_file_record() {
        return ParseResult::Skip;
    }

    let is_extension_record = !header.is_base_record();
    let base_frs_value = if is_extension_record {
        file_reference_to_frs(header.base_file_record_segment)
    } else {
        0
    };

    if is_extension_record && !options.include_extensions {
        return parse_extension_record(data, frs, &header, base_frs_value);
    }

    parse_base_record(
        data,
        frs,
        &header,
        is_deleted,
        is_extension_record,
        base_frs_value,
    )
}
