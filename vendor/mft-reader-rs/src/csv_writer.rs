//! CSV Writer - Exports MFT records to CSV format

use crate::mft_reader::MftRecord;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use csv::Writer;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

/// Convert Windows FILETIME (100-nanosecond intervals since 1601-01-01) to DateTime
fn filetime_to_datetime(filetime: i64) -> Option<DateTime<Utc>> {
    if filetime <= 0 {
        return None;
    }

    // Windows FILETIME epoch is January 1, 1601
    // Unix epoch is January 1, 1970
    // Difference is 11644473600 seconds
    const FILETIME_UNIX_DIFF: i64 = 11644473600;
    const TICKS_PER_SECOND: i64 = 10_000_000;

    let seconds = filetime / TICKS_PER_SECOND - FILETIME_UNIX_DIFF;
    let nanos = ((filetime % TICKS_PER_SECOND) * 100) as u32;

    Utc.timestamp_opt(seconds, nanos).single()
}

/// Format a FILETIME as ISO 8601 string
fn format_filetime(filetime: i64) -> String {
    match filetime_to_datetime(filetime) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => String::new(),
    }
}

/// Format file attributes as a string
fn format_attributes(attrs: u32) -> String {
    let mut result = String::new();

    if attrs & 0x0001 != 0 {
        result.push('R');
    } // Read-only
    if attrs & 0x0002 != 0 {
        result.push('H');
    } // Hidden
    if attrs & 0x0004 != 0 {
        result.push('S');
    } // System
    if attrs & 0x0010 != 0 {
        result.push('D');
    } // Directory
    if attrs & 0x0020 != 0 {
        result.push('A');
    } // Archive
    if attrs & 0x0080 != 0 {
        result.push('N');
    } // Normal
    if attrs & 0x0100 != 0 {
        result.push('T');
    } // Temporary
    if attrs & 0x0200 != 0 {
        result.push('P');
    } // Sparse
    if attrs & 0x0400 != 0 {
        result.push('L');
    } // Reparse point
    if attrs & 0x0800 != 0 {
        result.push('C');
    } // Compressed
    if attrs & 0x1000 != 0 {
        result.push('O');
    } // Offline
    if attrs & 0x2000 != 0 {
        result.push('I');
    } // Not indexed
    if attrs & 0x4000 != 0 {
        result.push('E');
    } // Encrypted

    if result.is_empty() {
        result.push('-');
    }

    result
}

/// Write MFT records to a CSV file
pub fn write_csv<P: AsRef<Path>>(records: &[MftRecord], output_path: P) -> Result<()> {
    let file = File::create(output_path)?;
    write_csv_to_writer(records, file)
}

/// Write MFT records to stdout
pub fn write_csv_stdout(records: &[MftRecord]) -> Result<()> {
    let stdout = io::stdout();
    let handle = stdout.lock();
    write_csv_to_writer(records, handle)
}

/// Write MFT records to any writer
fn write_csv_to_writer<W: Write>(records: &[MftRecord], writer: W) -> Result<()> {
    let mut csv_writer = Writer::from_writer(writer);

    // Write header
    csv_writer.write_record(&[
        "RecordNumber",
        "SequenceNumber",
        "InUse",
        "IsDirectory",
        "ParentRecordNumber",
        "ParentSequenceNumber",
        "FileName",
        "FileSize",
        "AllocatedSize",
        "CreationTime",
        "ModificationTime",
        "AccessTime",
        "ChangeTime",
        "Attributes",
        "AttributeFlags",
        "LinkCount",
        "IsBaseRecord",
    ])?;

    // Write records
    for record in records {
        csv_writer.write_record(&[
            record.record_number.to_string(),
            record.sequence_number.to_string(),
            record.is_in_use.to_string(),
            record.is_directory.to_string(),
            record.parent_record_number.to_string(),
            record.parent_sequence_number.to_string(),
            record.file_name.clone(),
            record.file_size.to_string(),
            record.allocated_size.to_string(),
            format_filetime(record.creation_time),
            format_filetime(record.modification_time),
            format_filetime(record.access_time),
            format_filetime(record.change_time),
            format_attributes(record.file_attributes),
            format!("0x{:08X}", record.file_attributes),
            record.link_count.to_string(),
            record.is_base_record.to_string(),
        ])?;
    }

    csv_writer.flush()?;
    Ok(())
}

