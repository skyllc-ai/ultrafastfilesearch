use core::cell::RefCell;

use super::{ParseOptions, ParseResult, apply_fixup, parse_record_forensic, parse_record_full};

// Thread-local buffer for record processing to avoid per-record allocations.
// Each thread gets its own 4KB buffer (enough for any MFT record).
thread_local! {
    static RECORD_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0_u8; 4096]);
}

/// Parses a record using a thread-local buffer to avoid allocation.
///
/// This function copies the record data into a thread-local buffer, applies
/// fixup, and parses it. This avoids per-record heap allocations in hot loops.
///
/// # Arguments
///
/// * `data` - The raw record data (will be copied to thread-local buffer)
/// * `frs` - The File Record Segment number
///
/// # Returns
///
/// `ParseResult::Base` for base records, `ParseResult::Extension` for
/// extension records, or `ParseResult::Skip` if invalid/not in use.
#[must_use]
pub fn parse_record_zero_alloc(data: &[u8], frs: u64) -> ParseResult {
    RECORD_BUFFER.with(|buf| {
        let mut buffer = buf.borrow_mut();

        if buffer.len() < data.len() {
            buffer.resize(data.len(), 0);
        }

        buffer[..data.len()].copy_from_slice(data);

        if !apply_fixup(&mut buffer[..data.len()]) {
            return ParseResult::Skip;
        }

        parse_record_full(&buffer[..data.len()], frs)
    })
}

/// Parses a record with forensic options using a thread-local buffer.
///
/// This is the forensic variant of `parse_record_zero_alloc` that supports
/// deleted, corrupt, and extension record extraction.
///
/// # Arguments
///
/// * `data` - The raw record data (will be copied to thread-local buffer)
/// * `frs` - The File Record Segment number
/// * `options` - Forensic parsing options
///
/// # Returns
///
/// `ParseResult::Base` for records matching options, `ParseResult::Extension`
/// for extension records (when not in forensic mode), or `ParseResult::Skip`.
#[must_use]
pub fn parse_record_zero_alloc_forensic(
    data: &[u8],
    frs: u64,
    options: &ParseOptions,
) -> ParseResult {
    RECORD_BUFFER.with(|buf| {
        let mut buffer = buf.borrow_mut();

        if buffer.len() < data.len() {
            buffer.resize(data.len(), 0);
        }

        buffer[..data.len()].copy_from_slice(data);

        let fixup_ok = apply_fixup(&mut buffer[..data.len()]);

        if !fixup_ok {
            return parse_record_forensic(&buffer[..data.len()], frs, options, true);
        }

        parse_record_forensic(&buffer[..data.len()], frs, options, false)
    })
}
