//! Dump raw MFT records from a `.raw` snapshot for specific FRS values.
//!
//! Offline, cross-platform tool operating on `uffs_mft`'s raw MFT format
//! (`*.raw` produced by `uffs_mft save`). This lets us inspect header flags
//! and basic structure for selected records to compare against C++ behavior.

// Standalone binary doesn't use all crate dependencies
#![allow(unused_crate_dependencies)]
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::too_many_lines)]

use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use uffs_mft::raw::{LoadRawOptions, load_raw_mft};
// This binary intentionally pulls in uffs_polars via the uffs-diag crate
// dependency set so that offline diagnostics can share the same Polars
// facade version as the main CLI, even though this specific tool does not
// use it directly.
#[allow(unused_imports)]
use uffs_polars as _;

/// Local copy of `MultiSectorHeader` layout so this tool can run on
/// non-Windows targets (the real `ntfs` module is `cfg(windows)`).
/// This matches the definition in `crates/uffs-mft/src/ntfs.rs`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct MultiSectorHeader {
    /// 4-byte magic value at the start of the record (e.g. `FILE`, `RCRD`).
    magic: u32,
    /// Byte offset to the update sequence array (USA) from the start
    /// of the record.
    usa_offset: u16,
    /// Number of USA entries (including the update sequence number).
    usa_count: u16,
}

/// Header at the start of every NTFS file record segment.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct FileRecordSegmentHeader {
    /// NTFS multi-sector (USA) header at the start of the record.
    multi_sector_header: MultiSectorHeader,
    /// Log file sequence number (LSN) for this record.
    log_file_sequence_number: i64,
    /// Per-record sequence number used for stale handle detection.
    sequence_number: u16,
    /// Number of hard links (directory entries) pointing at this record.
    link_count: u16,
    /// Byte offset from the start of the record to the first attribute.
    first_attribute_offset: u16,
    /// Bitflags describing record state (in-use, directory, etc.).
    flags: u16,
    /// Number of bytes in use within this record.
    bytes_in_use: u32,
    /// Total bytes allocated for this record.
    bytes_allocated: u32,
    /// File reference of the base record if this is an extension record.
    base_file_record_segment: u64,
    /// Next attribute identifier to be assigned.
    next_attribute_number: u16,
    /// Reserved field in the on-disk layout.
    reserved: u16,
    /// Lower 32 bits of the file reference number.
    segment_number_lower: u32,
}

impl FileRecordSegmentHeader {
    /// Returns `true` if the record is currently marked as in-use.
    const fn is_in_use(&self) -> bool {
        (self.flags & 0x0001) != 0
    }

    /// Returns `true` if the record represents a directory.
    const fn is_directory(&self) -> bool {
        (self.flags & 0x0002) != 0
    }

    /// Returns `true` if this record is a base record (not an extension).
    const fn is_base_record(&self) -> bool {
        self.base_file_record_segment == 0
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: dump_mft_records <mft.raw> <frs1> [frs2 frs3 ...]");
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

    for frs_value in frs_values {
        println!("\n============================================================");
        println!("FRS {frs_value}");
        println!("============================================================");

        match raw.get_record(frs_value) {
            Some(data) => dump_record(frs_value, data),
            None => {
                println!(
                    "Record out of range (max FRS = {})",
                    raw.header.record_count.saturating_sub(1)
                );
            }
        }
    }

    Ok(())
}
/// Number of bytes from the start of each record to include in the diagnostic
/// hex dump. This keeps output manageable while still showing header-adjacent
/// data for manual inspection.
const DUMP_BYTES: usize = 64;

/// Dump a single raw MFT record's header and a small hex preview.
#[allow(unsafe_code, clippy::single_call_fn)]
// This helper is intentionally kept separate from `main` because it
// encapsulates a focused, unsafe-heavy dump routine that may be reused from
// tests or future diagnostics, even though it currently has a single call site.
fn dump_record(frs: u64, data: &[u8]) {
    use core::mem::size_of;

    println!("Record {frs}: {} bytes", data.len());

    if data.len() < size_of::<FileRecordSegmentHeader>() {
        println!("  Record too small for FileRecordSegmentHeader");
        return;
    }

    // SAFETY: We've checked that the buffer is large enough for the header.
    let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Copy out of the packed struct fields to avoid unaligned references.
    let ms = header.multi_sector_header;
    let magic = ms.magic;
    let usa_offset = ms.usa_offset;
    let usa_count = ms.usa_count;

    let sequence_number = header.sequence_number;
    let link_count = header.link_count;
    let first_attribute_offset = header.first_attribute_offset;
    let flags = header.flags;
    let bytes_in_use = header.bytes_in_use;
    let bytes_allocated = header.bytes_allocated;
    let base_file_record_segment = header.base_file_record_segment;

    println!("MultiSectorHeader:");
    println!("  magic      = 0x{magic:08X}");
    println!("  usa_offset = {usa_offset}");
    println!("  usa_count  = {usa_count}");

    println!("FileRecordSegmentHeader:");
    println!("  sequence_number        = {sequence_number}");
    println!("  link_count             = {link_count}");
    println!("  first_attribute_offset = {first_attribute_offset}");
    println!("  flags                  = 0x{flags:04X}");
    println!("    is_in_use            = {}", header.is_in_use());
    println!("    is_directory         = {}", header.is_directory());
    println!("  bytes_in_use           = {bytes_in_use}");
    println!("  bytes_allocated        = {bytes_allocated}");
    println!("  base_file_record_seg   = 0x{base_file_record_segment:016X}",);
    println!("  is_base_record         = {}", header.is_base_record());

    // Quick hex dump of the first part of the record for manual inspection.
    let dump_len = core::cmp::min(DUMP_BYTES, data.len());
    let Some(slice) = data.get(..dump_len) else {
        println!("\nNo bytes available to dump (record shorter than header)");
        return;
    };

    println!("\nFirst {dump_len} bytes (hex):");
    for (index, chunk) in slice.chunks(16).enumerate() {
        print!("  {offset:04X}: ", offset = index * 16);
        for byte in chunk {
            print!("{byte:02X} ");
        }
        println!();
    }
}
