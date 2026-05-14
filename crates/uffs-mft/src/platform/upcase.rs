// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Read the NTFS `$UpCase` table from a live Windows volume and
//! persist it as a plain binary file.
//!
//! `$UpCase` (FRS 10) stores 128 KB of UTF-16 uppercase mappings.  The
//! actual table data is **non-resident** — it lives in disk clusters,
//! not in the MFT record.  Windows blocks opening `$UpCase` as a
//! regular file, so we read via: open volume → seek to FRS 10 in
//! MFT → parse DATA attribute data runs → read clusters.
//!
//! # Persistence
//!
//! Saved files are **not encrypted** — the `$UpCase` table is public
//! NTFS specification data containing no user information.  The file
//! format is:
//!
//! 1. [`UpcaseHeader`](crate::platform::upcase::UpcaseHeader) (64 bytes) —
//!    magic, NTFS version, serial, CRC-32
//! 2. Raw table (128 KB) — `[u16; 65_536]` little-endian
//!
//! Total file size: 131,136 bytes.  The raw table starts at offset 64.
//!
//! # Usage
//!
//! ```text
//! uffs-mft save --upcase
//! uffs-mft save --upcase --drive D --output D_upcase.bin
//! ```

use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::ntfs::DataRun;

/// FRS number for `$UpCase`.
#[cfg(windows)]
const UPCASE_FRS: u64 = 10;

/// Expected data size in bytes (65 536 entries × 2).
pub const UPCASE_SIZE_BYTES: usize = 65_536 * 2;

/// Magic bytes identifying a UFFS `$UpCase` file (before encryption).
const UPCASE_MAGIC: &[u8; 8] = b"UFFSUP\0\0";

/// Current upcase file format version.
const UPCASE_FORMAT_VERSION: u32 = 1;

/// Total header size in bytes (fixed, padded with reserved).
const HEADER_SIZE: usize = 64;

// ── Header ────────────────────────────────────────────────────────────

/// Metadata header stored alongside the raw `$UpCase` table.
///
/// ```text
/// Offset  Size  Field
/// 0       8     Magic: b"UFFSUP\0\0"
/// 8       4     Format version (u32 LE) = 1
/// 12      2     NTFS major version (u16 LE)
/// 14      2     NTFS minor version (u16 LE)
/// 16      8     Volume serial number (u64 LE)
/// 24      4     CRC-32 of the raw 128 KB table (u32 LE)
/// 28      8     Timestamp — Unix epoch seconds (u64 LE)
/// 36      1     Drive letter (ASCII uppercase)
/// 37      27    Reserved (zeroed)
/// ─────────────
/// 64            Raw [u16; 65_536] table data (131 072 bytes)
/// ```
#[derive(Debug, Clone)]
pub struct UpcaseHeader {
    /// NTFS major version from the source volume.
    pub ntfs_major: u16,
    /// NTFS minor version from the source volume.
    pub ntfs_minor: u16,
    /// Volume serial number from the source volume.
    pub volume_serial: u64,
    /// CRC-32 of the raw 128 KB table bytes.
    pub table_crc32: u32,
    /// Timestamp (Unix epoch seconds) when the table was captured.
    pub timestamp: u64,
    /// Drive letter (ASCII uppercase).
    pub drive: char,
}

impl UpcaseHeader {
    /// Serializes header + raw table into a byte vector (plaintext).
    ///
    /// All indexing is safe: `buf` is allocated as `HEADER_SIZE +
    /// UPCASE_SIZE_BYTES` (64 + 131072 bytes), and all slices fall within
    /// that range.
    #[expect(
        clippy::indexing_slicing,
        reason = "buf allocated as HEADER_SIZE + UPCASE_SIZE_BYTES; all indices < HEADER_SIZE (64)"
    )]
    fn serialize(&self, table: &[u16; 65_536]) -> Vec<u8> {
        let mut buf = vec![0_u8; HEADER_SIZE + UPCASE_SIZE_BYTES];

        buf[0..8].copy_from_slice(UPCASE_MAGIC);
        buf[8..12].copy_from_slice(&UPCASE_FORMAT_VERSION.to_le_bytes());
        buf[12..14].copy_from_slice(&self.ntfs_major.to_le_bytes());
        buf[14..16].copy_from_slice(&self.ntfs_minor.to_le_bytes());
        buf[16..24].copy_from_slice(&self.volume_serial.to_le_bytes());
        buf[24..28].copy_from_slice(&self.table_crc32.to_le_bytes());
        buf[28..36].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[36] = self.drive as u8;
        // 37..64 reserved (already zeroed)

        let raw: &[u8] = bytemuck::cast_slice(table.as_ref());
        buf[HEADER_SIZE..].copy_from_slice(raw);
        buf
    }

    /// Deserializes header from the first 64 bytes of plaintext.
    ///
    /// All indexing is bounds-safe: the length check at the top guarantees
    /// `data.len() >= HEADER_SIZE + UPCASE_SIZE_BYTES` (> 37 bytes).
    ///
    /// # Errors
    ///
    /// Returns [`MftError`] if the data is too small, has the wrong magic
    /// bytes, or contains an invalid drive letter.
    #[expect(
        clippy::indexing_slicing,
        clippy::missing_asserts_for_indexing,
        reason = "length validated at top of function; all indices < HEADER_SIZE (64)"
    )]
    fn deserialize(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE + UPCASE_SIZE_BYTES {
            return Err(MftError::InvalidData(format!(
                "$UpCase file too small: {} bytes (need {})",
                data.len(),
                HEADER_SIZE + UPCASE_SIZE_BYTES
            )));
        }
        if &data[0..8] != UPCASE_MAGIC {
            return Err(MftError::InvalidData(
                "$UpCase file: wrong magic bytes".into(),
            ));
        }
        let version = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        if version != UPCASE_FORMAT_VERSION {
            return Err(MftError::InvalidData(format!(
                "$UpCase file: unsupported version {version} (expected {UPCASE_FORMAT_VERSION})"
            )));
        }
        Ok(Self {
            ntfs_major: u16::from_le_bytes([data[12], data[13]]),
            ntfs_minor: u16::from_le_bytes([data[14], data[15]]),
            volume_serial: u64::from_le_bytes([
                data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            table_crc32: u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
            timestamp: u64::from_le_bytes([
                data[28], data[29], data[30], data[31], data[32], data[33], data[34], data[35],
            ]),
            drive: data[36] as char,
        })
    }
}

// ── CRC-32 helper ─────────────────────────────────────────────────────

/// Compute CRC-32 of the raw `$UpCase` table bytes.
///
/// Public so callers can build an [`UpcaseHeader`] with the correct
/// checksum before calling [`save_upcase_to_file`].
#[must_use]
pub fn crc32_table(data: &[u8]) -> u32 {
    crc32(data)
}

/// Compute CRC-32 of a byte slice (IEEE / ITU-T V.42, same as PNG/ZIP).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0_i32..8_i32 {
            crc = if crc & 1 != 0 {
                (crc >> 1_i32) ^ 0xEDB8_8320
            } else {
                crc >> 1_i32
            };
        }
    }
    !crc
}

// ── Persistence (plain binary, no encryption) ─────────────────────────

/// Save the `$UpCase` table to a plain binary file.
///
/// The `$UpCase` table is public NTFS specification data with no user
/// content, so encryption is unnecessary.  The file is written
/// atomically (temp + rename) with restricted permissions.
///
/// File format: [`UpcaseHeader`] (64 bytes) + raw table (128 KB).
///
/// # Errors
///
/// Returns an error if file I/O fails.
pub fn save_upcase_to_file(
    path: &std::path::Path,
    header: &UpcaseHeader,
    table: &[u16; 65_536],
) -> Result<()> {
    let data = header.serialize(table);

    crate::cache::atomic_write(path, &data)
        .map_err(|err| MftError::InvalidData(format!("Failed to write $UpCase file: {err}")))?;

    tracing::info!(
        path = %path.display(),
        bytes = data.len(),
        crc32 = format_args!("0x{:08X}", header.table_crc32),
        "Saved $UpCase table"
    );
    Ok(())
}

/// Load the `$UpCase` table from a plain binary file.
///
/// Validates magic bytes, format version, and CRC-32 checksum.
///
/// # Errors
///
/// Returns an error if the file is too small, has wrong magic bytes,
/// unsupported version, or CRC-32 mismatch.
pub fn load_upcase_from_file(path: &std::path::Path) -> Result<(UpcaseHeader, Box<[u16; 65_536]>)> {
    let data = std::fs::read(path).map_err(|err| {
        MftError::InvalidData(format!("Failed to read {}: {err}", path.display()))
    })?;

    let header = UpcaseHeader::deserialize(&data)?;

    // Extract raw table bytes (starts at offset 64).
    let table_bytes = data
        .get(HEADER_SIZE..HEADER_SIZE + UPCASE_SIZE_BYTES)
        .ok_or_else(|| MftError::InvalidData("upcase data too short for table".to_owned()))?;
    let u16_slice: &[u16] = bytemuck::cast_slice(table_bytes);
    let mut table: Box<[u16; 65_536]> = vec![0_u16; 65_536]
        .into_boxed_slice()
        .try_into()
        .map_err(|_size_err| MftError::InvalidData("upcase table size mismatch".to_owned()))?;
    table.copy_from_slice(u16_slice);

    // Verify CRC-32.
    let actual_crc = crc32(table_bytes);
    if actual_crc != header.table_crc32 {
        return Err(MftError::InvalidData(format!(
            "$UpCase CRC-32 mismatch: file=0x{:08X} computed=0x{actual_crc:08X}",
            header.table_crc32
        )));
    }

    tracing::info!(
        path = %path.display(),
        drive = %header.drive,
        ntfs = %format!("{}.{}", header.ntfs_major, header.ntfs_minor),
        "Loaded $UpCase table"
    );

    Ok((header, table))
}

/// Parsed `$UpCase` metadata extracted from FRS 10.
#[cfg(windows)]
#[derive(Debug)]
struct UpcaseDataRuns {
    /// Data run descriptors (VCN → LCN mapping).
    runs: Vec<DataRun>,
    /// Actual data size from the non-resident DATA attribute header.
    data_size: u64,
}

/// Parse a single MFT record's DATA attribute to extract data runs.
///
/// `record_bytes` must be the raw FRS 10 bytes *after* USA fixup.
#[cfg(windows)]
fn parse_data_runs(record_bytes: &[u8]) -> Result<UpcaseDataRuns> {
    use crate::ntfs::{AttributeIterator, AttributeType};

    let mut attrs = AttributeIterator::new(record_bytes)
        .ok_or_else(|| MftError::InvalidData("FRS 10 ($UpCase): invalid record header".into()))?;

    let data_attr = attrs
        .find(|attr| {
            attr.attribute_type() == Some(AttributeType::Data)
                && attr.is_non_resident()
                && attr.header.name_length == 0
        })
        .ok_or_else(|| {
            MftError::InvalidData("FRS 10 ($UpCase): no non-resident unnamed DATA attribute".into())
        })?;

    let nr = data_attr.non_resident_data().ok_or_else(|| {
        MftError::InvalidData("FRS 10 ($UpCase): cannot decode non-resident header".into())
    })?;

    let runs = data_attr.data_runs();
    if runs.is_empty() {
        return Err(MftError::InvalidData(
            "FRS 10 ($UpCase): DATA has no data runs".into(),
        ));
    }

    Ok(UpcaseDataRuns {
        runs,
        data_size: nr.data_size.cast_unsigned(),
    })
}

/// Read the `$UpCase` table from a live NTFS volume.
///
/// Opens the volume, reads FRS 10 from the MFT, parses its data runs,
/// reads the referenced clusters, and returns the 128 KB table.
///
/// # Errors
///
/// Returns [`MftError::PlatformNotSupported`] on non-Windows.
#[cfg(not(windows))]
pub const fn read_upcase_table(_drive: char) -> Result<Box<[u16; 65_536]>> {
    Err(MftError::PlatformNotSupported)
}

/// Read the `$UpCase` table from a live NTFS volume (Windows).
///
/// # Errors
///
/// Returns [`MftError::Io`] if opening the volume, locating `$UpCase`, or
/// reading its data runs via `ReadFile`/`SetFilePointerEx` fails, and
/// [`MftError::InvalidData`] if the assembled table is shorter than the
/// expected 128 KiB (65 536 code-point entries).
#[cfg(windows)]
pub fn read_upcase_table(drive: char) -> Result<Box<[u16; 65_536]>> {
    use crate::parse::apply_fixup;
    use crate::platform::VolumeHandle;

    let handle = VolumeHandle::open(drive)?;
    let vol = handle.volume_data();
    let rs = vol.bytes_per_file_record_segment as usize;
    let mft_offset = handle.mft_byte_offset();
    let frs10_offset = mft_offset + UPCASE_FRS * rs as u64;

    // Read FRS 10 from the MFT on disk.
    let mut record = vec![0_u8; rs];
    volume_read_at(handle.raw_handle(), frs10_offset, &mut record)?;
    apply_fixup(&mut record);

    // Parse data runs.
    let info = parse_data_runs(&record)?;
    if usize::try_from(info.data_size)
        .map_err(|_err| MftError::InvalidData("$UpCase data_size exceeds usize::MAX".to_owned()))?
        != UPCASE_SIZE_BYTES
    {
        return Err(MftError::InvalidData(format!(
            "$UpCase data_size {} != expected {UPCASE_SIZE_BYTES}",
            info.data_size
        )));
    }

    tracing::debug!(
        runs = info.runs.len(),
        data_size = info.data_size,
        "Parsed $UpCase data runs from FRS 10"
    );

    // Read clusters.
    let buf = read_clusters(handle.raw_handle(), &info.runs, vol.bytes_per_cluster)?;

    // Reinterpret as [u16; 65_536].  Allocate directly on the heap via
    // `vec! -> Box<[u16]> -> Box<[u16; N]>` to avoid the 128 KiB stack array
    // that `Box::new([0_u16; 65_536])` would materialize before moving.
    let u16_slice: &[u16] = bytemuck::cast_slice(&buf);
    let mut table: Box<[u16; 65_536]> =
        vec![0_u16; 65_536]
            .into_boxed_slice()
            .try_into()
            .map_err(|_boxed: Box<[u16]>| {
                MftError::InvalidData(
                    "internal: 65_536-element vec<u16> failed conversion to fixed-size array"
                        .to_owned(),
                )
            })?;
    table.copy_from_slice(u16_slice);

    tracing::info!(
        bytes = UPCASE_SIZE_BYTES,
        "Read $UpCase table from live volume"
    );
    Ok(table)
}

// ── Windows I/O helpers ───────────────────────────────────────────────

/// Seek + read from a raw volume handle.
#[cfg(windows)]
fn volume_read_at(
    handle: windows::Win32::Foundation::HANDLE,
    offset: u64,
    buf: &mut [u8],
) -> Result<()> {
    use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

    let seek_pos = i64::try_from(offset).map_err(|err| {
        MftError::InvalidData(format!("$UpCase: offset {offset} exceeds i64::MAX ({err})"))
    })?;

    // SAFETY: SetFilePointerEx is a well-defined Win32 API.
    #[expect(unsafe_code, reason = "FFI: SetFilePointerEx")]
    unsafe {
        SetFilePointerEx(handle, seek_pos, None, FILE_BEGIN).map_err(|err| {
            MftError::InvalidData(format!("$UpCase: seek to offset {offset} failed: {err}"))
        })
    }?;

    let mut bytes_read = 0_u32;
    // SAFETY: ReadFile writes into valid writable `buf`.
    #[expect(unsafe_code, reason = "FFI: ReadFile")]
    unsafe {
        ReadFile(handle, Some(buf), Some(&raw mut bytes_read), None).map_err(|err| {
            MftError::InvalidData(format!("$UpCase: read {} bytes failed: {err}", buf.len()))
        })
    }?;

    if (bytes_read as usize) < buf.len() {
        return Err(MftError::InvalidData(format!(
            "$UpCase: short read: got {bytes_read}/{}",
            buf.len()
        )));
    }
    Ok(())
}

/// Read clusters from data runs into a contiguous buffer.
#[cfg(windows)]
fn read_clusters(
    handle: windows::Win32::Foundation::HANDLE,
    runs: &[DataRun],
    bytes_per_cluster: u32,
) -> Result<Vec<u8>> {
    let bpc = u64::from(bytes_per_cluster);
    let mut buf = vec![0_u8; UPCASE_SIZE_BYTES];
    let mut offset: usize = 0;

    for run in runs {
        let run_byte_len = usize::try_from(run.cluster_count * bpc).map_err(|err| {
            MftError::InvalidData(format!(
                "$UpCase run byte count {} (cluster_count={}, bytes_per_cluster={bpc}) \
                 exceeds usize::MAX ({err})",
                run.cluster_count * bpc,
                run.cluster_count,
            ))
        })?;

        if run.lcn == 0 {
            // Sparse — already zeroed.
            offset += run_byte_len;
            continue;
        }

        let disk_byte = run.lcn * bpc.cast_signed();
        let run_bytes = run_byte_len;
        let read_len = run_bytes.min(UPCASE_SIZE_BYTES - offset);

        let disk_offset = crate::index::nonneg_to_u64(disk_byte);
        let Some(read_window) = buf.get_mut(offset..offset + read_len) else {
            return Err(MftError::InvalidData(format!(
                "$UpCase: run at offset {offset} length {read_len} exceeds buffer size \
                 {UPCASE_SIZE_BYTES}"
            )));
        };
        volume_read_at(handle, disk_offset, read_window)?;
        offset += read_len;
    }

    if offset < UPCASE_SIZE_BYTES {
        return Err(MftError::InvalidData(format!(
            "$UpCase: assembled only {offset}/{UPCASE_SIZE_BYTES} bytes"
        )));
    }
    Ok(buf)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known-valid indices"
)]
#[expect(
    clippy::let_underscore_untyped,
    reason = "test code — type annotation not needed"
)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "test cleanup — ignoring Result from fs operations"
)]
#[expect(clippy::min_ident_chars, reason = "test code — short variable names")]
mod tests {
    use super::*;

    /// Round-trip: build a header + table, save to temp file, load back.
    #[test]
    fn save_and_load_roundtrip() -> Result<()> {
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let raw_table = fold.table();
        #[expect(
            clippy::large_stack_arrays,
            reason = "immediately boxed — no stack allocation"
        )]
        let mut table = Box::new([0_u16; 65_536]);
        table.copy_from_slice(raw_table);

        let raw_bytes: &[u8] = bytemuck::cast_slice(table.as_ref());
        let table_crc = crc32(raw_bytes);

        let header = UpcaseHeader {
            ntfs_major: 3,
            ntfs_minor: 1,
            volume_serial: 0xDEAD_BEEF_CAFE_1234,
            table_crc32: table_crc,
            timestamp: 1_700_000_000,
            drive: 'C',
        };

        let dir = std::env::temp_dir().join("uffs_upcase_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roundtrip.bin");

        save_upcase_to_file(&path, &header, &table)?;

        // File should be exactly 64 + 131072 bytes.
        let meta = std::fs::metadata(&path)
            .map_err(|e| MftError::InvalidData(format!("metadata failed: {e}")))?;
        assert_eq!(meta.len(), (HEADER_SIZE + UPCASE_SIZE_BYTES) as u64);

        let (loaded_hdr, loaded_tbl) = load_upcase_from_file(&path)?;
        assert_eq!(loaded_hdr.ntfs_major, 3);
        assert_eq!(loaded_hdr.ntfs_minor, 1);
        assert_eq!(loaded_hdr.volume_serial, 0xDEAD_BEEF_CAFE_1234);
        assert_eq!(loaded_hdr.table_crc32, table_crc);
        assert_eq!(loaded_hdr.drive, 'C');

        // Table contents must match.
        assert_eq!(loaded_tbl.as_ref(), table.as_ref());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        Ok(())
    }

    /// Load the Windows-captured reference file (`upcase_windows_c.bin`)
    /// and validate header fields and table sanity against the embedded
    /// default.
    #[test]
    fn load_windows_captured_file() -> Result<()> {
        // The reference file lives next to the embedded default.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ref_path = manifest
            .parent()
            .ok_or_else(|| MftError::InvalidData("no parent dir".into()))?
            .join("uffs-text")
            .join("src")
            .join("upcase_windows_c.bin");

        if !ref_path.exists() {
            // Not fatal — the file is only present after a Windows capture.
            eprintln!("Skipping: {ref_path:?} not found");
            return Ok(());
        }

        let (hdr, tbl) = load_upcase_from_file(&ref_path)?;

        // Header sanity.
        assert_eq!(hdr.drive, 'C');
        assert_eq!(hdr.table_crc32, 0xCEE8_CFFA);

        // Table must match the embedded default exactly.
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let embedded = fold.table();
        assert_eq!(
            tbl.as_ref(),
            embedded,
            "Windows-captured table differs from embedded default"
        );

        // Spot-check folding values.
        assert_eq!(tbl[0x61], 0x0041, "a → A");
        assert_eq!(tbl[0x7A], 0x005A, "z → Z");
        assert_eq!(tbl[0x00FC], 0x00DC, "ü → Ü");
        assert_eq!(tbl[0x00E9], 0x00C9, "é → É");
        assert_eq!(tbl[0x4E2D], 0x4E2D, "中 identity");
        Ok(())
    }

    /// CRC-32 of the embedded default table must match the Windows capture.
    #[test]
    fn embedded_crc32_matches_windows() {
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let raw: &[u8] = bytemuck::cast_slice(fold.table());
        let crc = crc32(raw);
        assert_eq!(
            crc, 0xCEE8_CFFA,
            "Embedded default CRC-32 0x{crc:08X} != Windows 0xCEE8CFFA"
        );
    }
}
