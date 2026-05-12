// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shared zero-copy buffer parsing helpers.

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
    let skip_begin = frs_to_usize(chunk.skip_begin);
    let effective_count = frs_to_usize(chunk.effective_record_count());
    let record_size_usize = u32_as_usize(record_size);
    let start_frs = chunk.start_frs;

    let mut results = Vec::with_capacity(effective_count);

    for i in 0..effective_count {
        let offset = (skip_begin + i) * record_size_usize;
        if offset + record_size_usize > bytes_read {
            break;
        }

        let frs = start_frs + usize_to_u64(skip_begin) + usize_to_u64(i);

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
