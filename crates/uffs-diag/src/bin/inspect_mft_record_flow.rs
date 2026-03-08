//! Inspect the full Rust raw->fixup->parse pipeline for specific FRS.
//!
//! Offline, cross-platform tool that loads a `.raw` MFT snapshot produced by
//! `uffs_mft save`, then for selected FRS runs the same `apply_fixup` and
//! `parse_record_full` logic used by the Windows reader. This lets us pinpoint
//! where a record is being dropped (fixup vs parse) compared to the reference
//! CSV.

#![expect(
    unused_crate_dependencies,
    reason = "standalone binary doesn't use all crate dependencies"
)]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "diagnostic tool — stdout/stderr output is intentional"
)]

use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use uffs_mft::{load_raw_mft, LoadRawOptions, RawMftData};
// Keep uffs_polars dependency wired in for version-locking, even though this
// binary does not use it directly.
#[expect(
    unused_imports,
    reason = "version-locks uffs_polars with diagnostic crate"
)]
use uffs_polars as _;

/// Local copy of `FileRecordSegmentHeader` so this binary can run on
/// non-Windows targets (the real NTFS structs are cfg(windows)).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct FileRecordSegmentHeader {
    /// Embedded multi-sector header containing NTFS magic and USA metadata.
    multi_sector_header: MultiSectorHeader,
    /// Log file sequence number (LSN) associated with this record.
    log_file_sequence_number: i64,
    /// Sequence number used together with the FRS to form a full file
    /// reference.
    sequence_number: u16,
    /// Count of hard links (directory entries) to this record.
    link_count: u16,
    /// Byte offset from the start of the record to the first attribute.
    first_attribute_offset: u16,
    /// NTFS record flags (in-use, directory, etc.).
    flags: u16,
    /// Number of bytes in use within this FILE record.
    bytes_in_use: u32,
    /// Allocated size of this FILE record in bytes.
    bytes_allocated: u32,
    /// Base file record this record extends (0 for base records).
    base_file_record_segment: u64,
    /// Next attribute identifier that will be assigned.
    next_attribute_number: u16,
    /// Reserved field in the NTFS on-disk layout.
    reserved: u16,
    /// Lower 32 bits of the segment number for this record.
    segment_number_lower: u32,
}

/// Local copy of `MultiSectorHeader` layout (matches
/// crates/uffs-mft/src/ntfs.rs).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct MultiSectorHeader {
    /// Four-byte NTFS magic value (e.g. FILE, RCRD, INDX).
    magic: u32,
    /// Byte offset to the update sequence array (USA).
    usa_offset: u16,
    /// Number of USA entries, including the sequence number.
    usa_count: u16,
}

impl FileRecordSegmentHeader {
    /// Returns `true` if the record is marked as in-use in its flag bits.
    const fn is_in_use(&self) -> bool {
        (self.flags & 0x0001) != 0
    }

    /// Returns `true` if the record is marked as a directory in its flag bits.
    const fn is_directory(&self) -> bool {
        (self.flags & 0x0002) != 0
    }

    /// Returns `true` when this record is a base record (not an extension).
    const fn is_base_record(&self) -> bool {
        self.base_file_record_segment == 0
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: inspect_mft_record_flow <mft.raw> <frs1> [frs2 frs3 ...]");
        std::process::exit(1);
    }

    let raw_path = args.get(1).map(String::as_str).ok_or_else(|| {
        anyhow::anyhow!("Expected <mft.raw> path argument to be present after length check",)
    })?;

    let frs_values: Vec<u64> = args
        .get(2..)
        .ok_or_else(|| anyhow::anyhow!("Expected at least one FRS argument after <mft.raw>"))?
        .iter()
        .map(|arg| arg.parse::<u64>())
        .collect::<core::result::Result<_, _>>()
        .context("Failed to parse FRS arguments as u64")?;

    let path = Path::new(raw_path);
    let raw = load_raw_mft(path, &LoadRawOptions::default())
        .with_context(|| format!("Failed to load raw MFT from {}", path.display()))?;

    println!(
        "Loaded raw MFT: {} records, record_size={} bytes",
        raw.header.record_count, raw.header.record_size
    );

    for frs in frs_values {
        inspect_record_flow(&raw, frs);
    }

    Ok(())
}

/// Inspect a single FRS end-to-end from raw bytes through parsed header
/// (and, on Windows, full fixup + `parse_record_full`).
#[expect(
    clippy::single_call_fn,
    reason = "encapsulates the full record inspection pipeline"
)]
#[expect(
    clippy::too_many_lines,
    reason = "sequential diagnostic dump — splitting would reduce clarity"
)]
fn inspect_record_flow(raw: &RawMftData, frs: u64) {
    use core::mem::size_of;

    println!("\n============================================================");
    println!("FRS {frs} - raw -> fixup -> parse_record_full");
    println!("============================================================");

    let Some(record) = raw.get_record(frs) else {
        println!(
            "Record out of range (max FRS = {})",
            raw.header.record_count.saturating_sub(1)
        );
        return;
    };

    println!("Raw record size: {} bytes", record.len());

    if record.len() < size_of::<FileRecordSegmentHeader>() {
        println!("Record too small for FileRecordSegmentHeader");
        return;
    }

    // First, dump basic header fields without fixup so we can compare to
    // dump_mft_records.
    let hdr = read_header(record);
    let ms = hdr.multi_sector_header;
    let magic = ms.magic;
    let usa_offset = ms.usa_offset;
    let usa_count = ms.usa_count;
    let flags = hdr.flags;
    let base = hdr.base_file_record_segment;
    println!("Header (pre-fixup):");
    println!("  magic      = 0x{magic:08X}");
    println!("  usa_offset = {usa_offset}");
    println!("  usa_count  = {usa_count}");
    println!("  flags      = 0x{flags:04X}");
    println!("    is_in_use    = {}", hdr.is_in_use());
    println!("    is_directory = {}", hdr.is_directory());
    println!("  base_file_record_segment = 0x{base:016X}");
    println!("  is_base_record           = {}", hdr.is_base_record());

    // Now run the same fixup + parse pipeline as Windows reader.
    // NOTE: On non-Windows targets we do not have access to the full
    // `apply_fixup` + `parse_record_full` pipeline because those helpers are
    // behind `cfg(windows)` in the library. This diagnostic focuses on
    // verifying that the on-disk header for a given FRS matches what
    // `dump_mft_records` reports and what the reference reader sees.

    // On Windows we can run the full fixup + parse pipeline, because the
    // helpers live in the uffs-mft crate behind cfg(windows).
    #[cfg(windows)]
    {
        use uffs_diag::uffs_mft_helpers_windows::run_fixup_and_parse_for_frs;

        run_fixup_and_parse_for_frs(raw, frs);
    }

    // On non-Windows targets we only perform the header dump. The full NTFS
    // parsing pipeline depends on Windows-specific structs and is not
    // available here.
    #[cfg(not(windows))]
    {
        println!("(fixup + full parse not available on this platform; header dump only)",);
    }
}

/// Interpret the leading bytes of a record as a `FileRecordSegmentHeader`.
#[expect(
    unsafe_code,
    reason = "ptr::read from validated buffer for packed NTFS struct"
)]
#[expect(
    clippy::missing_const_for_fn,
    reason = "unsafe block prevents const evaluation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "encapsulates unsafe header read for clarity"
)]
fn read_header(record: &[u8]) -> FileRecordSegmentHeader {
    // SAFETY: Caller has checked that `record` is large enough for the header.
    unsafe { core::ptr::read(record.as_ptr().cast()) }
}
