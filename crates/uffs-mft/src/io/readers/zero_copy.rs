// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared zero-copy buffer parsing helpers.
//!
//! **Module-scoped cast justification:** `as usize` casts here convert NTFS
//! on-disk record sizes (`u32`) into `usize` for buffer slicing.  `usize` is
//! ≥ 32 bits on every supported target.
#![expect(
    clippy::cast_possible_truncation,
    reason = "NTFS record-size (u32 -> usize) casts are lossless on supported 32/64-bit targets"
)]

use super::prelude::*;

/// Inner zero-copy parsing function that works with raw parameters.
///
/// This is used by both `ReadBuffer` and `OverlappedRead` parsing paths.
pub(super) fn parse_buffer_zero_copy_inner(
    buffer_slice: &mut [u8],
    bytes_read: usize,
    chunk: &ReadChunk,
    record_size: u32,
    merge_extensions: bool,
) -> Vec<ParseResult> {
    let skip_begin = chunk.skip_begin as usize;
    let effective_count = chunk.effective_record_count() as usize;
    let record_size_usize = record_size as usize;
    let start_frs = chunk.start_frs;

    let mut results = Vec::with_capacity(effective_count);

    for i in 0..effective_count {
        let offset = (skip_begin + i) * record_size_usize;
        if offset + record_size_usize > bytes_read {
            break;
        }

        let frs = start_frs + skip_begin as u64 + i as u64;

        let Some(record_slice) = buffer_slice.get_mut(offset..offset + record_size_usize) else {
            break;
        };

        // Apply fixup in-place on the shared buffer (zero-copy)
        if !apply_fixup(record_slice) {
            continue;
        }

        // Parse record from the fixed-up slice (no copy needed)
        if merge_extensions {
            results.push(parse_record_full(record_slice, frs));
        } else if let Some(rec) = parse_record(record_slice, frs) {
            results.push(ParseResult::Base(rec));
        }
    }

    results
}
