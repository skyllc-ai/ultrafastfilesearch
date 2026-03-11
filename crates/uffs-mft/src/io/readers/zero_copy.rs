//! Shared zero-copy buffer parsing helpers.

use super::*;

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

        // Apply fixup in-place on the shared buffer (zero-copy)
        let record_slice = &mut buffer_slice[offset..offset + record_size_usize];
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
