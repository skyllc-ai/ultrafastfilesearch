//! IOCP MFT Capture Format.
//!
//! This module provides functionality to save and load MFT data captured in
//! IOCP (I/O Completion Port) completion order. Unlike the sequential `raw.rs`
//! format, this captures the non-deterministic order in which Windows IOCP
//! delivers completed reads.
//!
//! # Purpose
//!
//! When reading MFT via IOCP, chunks complete in non-deterministic order based
//! on disk latency, queue depth, and scheduling. This format captures that
//! exact order, enabling:
//!
//! - Realistic testing of parsers with out-of-order chunk delivery
//! - Reproducing Windows IOCP behavior on non-Windows systems
//! - Debugging ordering-sensitive issues
//!
//! # File Format
//!
//! ```text
//! [Header: 96 bytes]
//!   - Magic: "UFFS-IOCP" (9 bytes)
//!   - Padding: 3 bytes (alignment)
//!   - Version: u32 (4 bytes) - currently 1
//!   - Flags: u32 (4 bytes) - bit 0: compressed
//!   - Record size: u32 (4 bytes) - typically 1024
//!   - Chunk count: u32 (4 bytes)
//!   - Total records: u64 (8 bytes)
//!   - Total data size: u64 (8 bytes) - uncompressed chunk data size
//!   - Volume letter: u8 (1 byte)
//!   - Concurrency: u8 (1 byte) - IOCP concurrency used
//!   - Reserved: 46 bytes
//!
//! [Chunk Index: 32 bytes per chunk]
//!   For each chunk (in IOCP completion order):
//!     - completion_seq: u32 (4 bytes) - completion sequence number
//!     - start_frs: u64 (8 bytes) - first FRS in chunk
//!     - record_count: u32 (4 bytes) - records in chunk
//!     - data_offset: u64 (8 bytes) - offset in data section
//!     - data_size: u32 (4 bytes) - bytes of data
//!     - reserved: u32 (4 bytes)
//!
//! [Chunk Data]
//!   Raw MFT bytes for each chunk, in IOCP completion order
//! ```

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::error::{MftError, Result};

/// Magic bytes for IOCP capture format.
const MAGIC: &[u8; 9] = b"UFFS-IOCP";

/// Current format version.
const VERSION: u32 = 1;

/// Flag: data is zstd compressed.
const FLAG_COMPRESSED: u32 = 0x0001;

/// Header size in bytes.
const HEADER_SIZE: usize = 96;

/// Chunk index entry size in bytes.
const CHUNK_ENTRY_SIZE: usize = 32;

/// IOCP capture file header.
#[derive(Debug, Clone)]
pub struct IocpCaptureHeader {
    /// Format version.
    pub version: u32,
    /// Flags (bit 0: compressed).
    pub flags: u32,
    /// Size of each MFT record in bytes.
    pub record_size: u32,
    /// Number of chunks captured.
    pub chunk_count: u32,
    /// Total MFT records across all chunks.
    pub total_records: u64,
    /// Total uncompressed data size.
    pub total_data_size: u64,
    /// Volume letter (e.g., 'C').
    pub volume_letter: char,
    /// IOCP concurrency level used during capture.
    pub concurrency: u8,
}

impl IocpCaptureHeader {
    /// Returns true if the data is compressed.
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }

    /// Serializes the header to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0_u8; HEADER_SIZE];
        buf[0..9].copy_from_slice(MAGIC);
        // 3 bytes padding at 9..12
        buf[12..16].copy_from_slice(&self.version.to_le_bytes());
        buf[16..20].copy_from_slice(&self.flags.to_le_bytes());
        buf[20..24].copy_from_slice(&self.record_size.to_le_bytes());
        buf[24..28].copy_from_slice(&self.chunk_count.to_le_bytes());
        buf[28..36].copy_from_slice(&self.total_records.to_le_bytes());
        buf[36..44].copy_from_slice(&self.total_data_size.to_le_bytes());
        buf[44] = self.volume_letter as u8;
        buf[45] = self.concurrency;
        // Reserved bytes 46-95 are already zero
        buf
    }

    /// Deserializes the header from bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if magic doesn't match or version is unsupported.
    pub fn from_bytes(buf: &[u8; HEADER_SIZE]) -> Result<Self> {
        if &buf[0..9] != MAGIC {
            return Err(MftError::InvalidData(
                "Invalid IOCP capture file magic".into(),
            ));
        }

        let version = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        if version > VERSION {
            return Err(MftError::InvalidData(format!(
                "Unsupported IOCP capture version: {version}"
            )));
        }

        let flags = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let record_size = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
        let chunk_count = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let total_records = u64::from_le_bytes([
            buf[28], buf[29], buf[30], buf[31], buf[32], buf[33], buf[34], buf[35],
        ]);
        let total_data_size = u64::from_le_bytes([
            buf[36], buf[37], buf[38], buf[39], buf[40], buf[41], buf[42], buf[43],
        ]);
        let volume_letter = if buf[44].is_ascii_alphabetic() {
            char::from(buf[44]).to_ascii_uppercase()
        } else {
            'X'
        };
        let concurrency = buf[45];

        Ok(Self {
            version,
            flags,
            record_size,
            chunk_count,
            total_records,
            total_data_size,
            volume_letter,
            concurrency,
        })
    }
}

/// Metadata for a captured chunk.
#[derive(Debug, Clone)]
pub struct CapturedChunk {
    /// Sequence number when this chunk completed (0 = first completion).
    pub completion_seq: u32,
    /// First FRS number in this chunk.
    pub start_frs: u64,
    /// Number of records in this chunk.
    pub record_count: u32,
    /// Offset of chunk data in the data section.
    pub data_offset: u64,
    /// Size of chunk data in bytes.
    pub data_size: u32,
}

impl CapturedChunk {
    /// Serializes the chunk entry to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; CHUNK_ENTRY_SIZE] {
        let mut buf = [0_u8; CHUNK_ENTRY_SIZE];
        buf[0..4].copy_from_slice(&self.completion_seq.to_le_bytes());
        buf[4..12].copy_from_slice(&self.start_frs.to_le_bytes());
        buf[12..16].copy_from_slice(&self.record_count.to_le_bytes());
        buf[16..24].copy_from_slice(&self.data_offset.to_le_bytes());
        buf[24..28].copy_from_slice(&self.data_size.to_le_bytes());
        // Reserved bytes 28-31 are already zero
        buf
    }

    /// Deserializes a chunk entry from bytes.
    #[must_use]
    pub const fn from_bytes(buf: &[u8; CHUNK_ENTRY_SIZE]) -> Self {
        Self {
            completion_seq: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            start_frs: u64::from_le_bytes([
                buf[4], buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11],
            ]),
            record_count: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            data_offset: u64::from_le_bytes([
                buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
            ]),
            data_size: u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
        }
    }
}

/// Options for IOCP capture.
#[derive(Debug, Clone)]
pub struct IocpCaptureOptions {
    /// Whether to compress the data with zstd.
    pub compress: bool,
    /// Compression level (1-22, default 3).
    pub compression_level: i32,
    /// Volume letter (e.g., 'C', 'D').
    pub volume_letter: char,
    /// IOCP concurrency level.
    pub concurrency: u8,
}

impl Default for IocpCaptureOptions {
    fn default() -> Self {
        Self {
            compress: true,
            compression_level: 3,
            volume_letter: 'X',
            concurrency: 8,
        }
    }
}

/// Writer for IOCP capture format.
///
/// Collects chunks as they complete, then writes them all to a file.
/// This is a two-phase process: collect during IOCP reads, then finalize.
pub struct IocpCaptureWriter {
    /// Captured chunks with their data (in completion order).
    chunks: Vec<(CapturedChunk, Vec<u8>)>,
    /// MFT record size.
    record_size: u32,
    /// Volume letter.
    volume_letter: char,
    /// IOCP concurrency.
    concurrency: u8,
    /// Compression enabled.
    compress: bool,
    /// Compression level.
    compression_level: i32,
    /// Next completion sequence number.
    next_seq: u32,
}

impl IocpCaptureWriter {
    /// Creates a new IOCP capture writer.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Vec::new() is not const-stable in current Rust edition"
    )]
    pub fn new(record_size: u32, options: &IocpCaptureOptions) -> Self {
        Self {
            chunks: Vec::new(),
            record_size,
            volume_letter: options.volume_letter,
            concurrency: options.concurrency,
            compress: options.compress,
            compression_level: options.compression_level,
            next_seq: 0,
        }
    }

    /// Records a chunk as it completes from IOCP.
    ///
    /// Call this for each chunk in the order IOCP delivers them.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "MFT chunk size < 4GB guaranteed by Windows IOCP limits"
    )]
    pub fn record_chunk(&mut self, start_frs: u64, data: Vec<u8>) {
        let record_count = (data.len() / self.record_size as usize) as u32;
        let chunk = CapturedChunk {
            completion_seq: self.next_seq,
            start_frs,
            record_count,
            data_offset: 0, // Will be computed in finalize
            data_size: data.len() as u32,
        };
        self.chunks.push((chunk, data));
        self.next_seq += 1;
    }

    /// Returns the number of chunks captured so far.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Finalizes and writes the capture to a file.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "MFT capture chunk count < 4 billion in practice"
    )]
    pub fn write_to_file<P: AsRef<Path>>(mut self, path: P) -> Result<IocpCaptureHeader> {
        let path = path.as_ref();

        // Calculate data offsets
        let mut data_offset: u64 = 0;
        for (chunk, data) in &mut self.chunks {
            chunk.data_offset = data_offset;
            data_offset += data.len() as u64;
        }

        // Build header
        let total_records: u64 = self
            .chunks
            .iter()
            .map(|(c, _)| u64::from(c.record_count))
            .sum();
        let header = IocpCaptureHeader {
            version: VERSION,
            flags: if self.compress { FLAG_COMPRESSED } else { 0 },
            record_size: self.record_size,
            chunk_count: self.chunks.len() as u32,
            total_records,
            total_data_size: data_offset,
            volume_letter: self.volume_letter,
            concurrency: self.concurrency,
        };

        // Write to file
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(&header.to_bytes())?;

        // Write chunk index
        for (chunk, _) in &self.chunks {
            writer.write_all(&chunk.to_bytes())?;
        }

        // Write chunk data (optionally compressed)
        #[cfg(feature = "zstd")]
        if self.compress {
            let mut encoder = zstd::stream::Encoder::new(&mut writer, self.compression_level)?;
            for (_, data) in &self.chunks {
                encoder.write_all(data)?;
            }
            encoder.finish()?;
        } else {
            for (_, data) in &self.chunks {
                writer.write_all(data)?;
            }
        }

        #[cfg(not(feature = "zstd"))]
        for (_, data) in &self.chunks {
            writer.write_all(data)?;
        }

        writer.flush()?;

        Ok(header)
    }
}

/// Loaded IOCP capture data.
#[derive(Debug)]
pub struct IocpCaptureData {
    /// File header.
    pub header: IocpCaptureHeader,
    /// Chunks in IOCP completion order.
    pub chunks: Vec<CapturedChunk>,
    /// Raw chunk data (concatenated).
    pub data: Vec<u8>,
}

impl IocpCaptureData {
    /// Returns an iterator over chunks with their data slices.
    ///
    /// Chunks are yielded in IOCP completion order (as captured).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "MFT capture data < 4GB fits in usize on 64-bit"
    )]
    pub fn iter_chunks(&self) -> impl Iterator<Item = (&CapturedChunk, &[u8])> {
        self.chunks.iter().map(move |chunk| {
            let start = chunk.data_offset as usize;
            let end = start + chunk.data_size as usize;
            let data = self.data.get(start..end).unwrap_or(&[]);
            (chunk, data)
        })
    }

    /// Returns the record size.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.header.record_size
    }

    /// Returns the volume letter.
    #[must_use]
    pub const fn volume_letter(&self) -> char {
        self.header.volume_letter
    }
}

/// Loads an IOCP capture file.
///
/// # Errors
///
/// Returns an error if reading or parsing fails.
#[expect(
    clippy::cast_possible_truncation,
    reason = "MFT capture data fits in memory on 64-bit systems"
)]
pub fn load_iocp_capture<P: AsRef<Path>>(path: P) -> Result<IocpCaptureData> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    // Read header
    let mut header_buf = [0_u8; HEADER_SIZE];
    reader.read_exact(&mut header_buf)?;
    let header = IocpCaptureHeader::from_bytes(&header_buf)?;

    // Read chunk index
    let mut chunks = Vec::with_capacity(header.chunk_count as usize);
    for _ in 0..header.chunk_count {
        let mut entry_buf = [0_u8; CHUNK_ENTRY_SIZE];
        reader.read_exact(&mut entry_buf)?;
        chunks.push(CapturedChunk::from_bytes(&entry_buf));
    }

    // Read chunk data
    let data = if header.is_compressed() {
        #[cfg(feature = "zstd")]
        {
            let mut decoder = zstd::stream::Decoder::new(reader)?;
            let mut data = Vec::with_capacity(header.total_data_size as usize);
            decoder.read_to_end(&mut data)?;
            data
        }
        #[cfg(not(feature = "zstd"))]
        {
            return Err(MftError::InvalidData(
                "IOCP capture is compressed but zstd feature is disabled".into(),
            ));
        }
    } else {
        let mut data = Vec::with_capacity(header.total_data_size as usize);
        reader.read_to_end(&mut data)?;
        data
    };

    Ok(IocpCaptureData {
        header,
        chunks,
        data,
    })
}

/// Checks if a file is an IOCP capture format by reading the magic bytes.
///
/// # Errors
///
/// Returns an error if reading fails.
pub fn is_iocp_capture<P: AsRef<Path>>(path: P) -> Result<bool> {
    let file = File::open(path.as_ref())?;
    let mut reader = BufReader::new(file);
    let mut magic_buf = [0_u8; 9];
    if reader.read_exact(&mut magic_buf).is_err() {
        return Ok(false);
    }
    Ok(&magic_buf == MAGIC)
}

/// Load IOCP capture and build `MftIndex` using the **exact same pipeline** as
/// Windows LIVE.
///
/// This function replays IOCP capture in the original completion order, using
/// parallel parsing and `MftRecordMerger` exactly as Windows LIVE does. This
/// ensures byte-for-byte parity with live Windows reads.
///
/// # Pipeline (identical to Windows LIVE)
///
/// ```text
/// IOCP chunks (completion order) → parallel parse → MftRecordMerger → merge() → MftIndex
/// ```
///
/// # Errors
///
/// Returns an error if reading, parsing, or index building fails.
#[expect(
    clippy::cast_possible_truncation,
    reason = "MFT record counts fit in usize on 64-bit"
)]
/// Load IOCP capture and build `MftIndex` using the **exact same inline
/// parsing** as Windows LIVE (`parse_record_to_index` directly into
/// `MftIndex`).
///
/// This mirrors the Windows LIVE `SlidingIocpInline` path exactly:
/// 1. Process chunks in IOCP completion order (sequential, not parallel)
/// 2. Apply fixup to each record
/// 3. Call `parse_record_to_index` to build index directly (no merger)
/// 4. Call `compute_tree_metrics()` at the end
///
/// By using the same `parse_record_to_index` function, we ensure byte-for-byte
/// identical results with Windows LIVE, including any bugs or edge cases.
pub fn load_iocp_to_index<P: AsRef<Path>>(path: P) -> Result<crate::index::MftIndex> {
    use tracing::{debug, info};

    use crate::index::MftIndex;
    use crate::io::parse_record_to_index;
    use crate::parse::apply_fixup;

    debug!("[PARITY_TRACE] load_iocp_to_index: ENTER (INLINE parse_record_to_index)");
    let capture = load_iocp_capture(path)?;
    let volume = capture.volume_letter();
    let record_size = capture.record_size() as usize;
    let total_records = capture.header.total_records as usize;

    debug!(
        %volume,
        chunks = capture.chunks.len(),
        total_records,
        "[PARITY_TRACE] load_iocp_to_index config"
    );
    info!(
        volume = %volume,
        chunks = capture.chunks.len(),
        total_records,
        "Loading IOCP capture with INLINE parsing (matching Windows LIVE SlidingIocpInline)"
    );

    // Create empty MftIndex with capacity
    let mut index = MftIndex::with_capacity(volume, total_records);
    let mut records_parsed: usize = 0;
    let mut fixup_failed: usize = 0;

    // Process chunks in IOCP completion order (sequential, matching Windows LIVE)
    // Windows LIVE processes one IOCP completion at a time on the completion thread
    for (chunk, chunk_data) in capture.iter_chunks() {
        let records_in_chunk = chunk.record_count as usize;

        for i in 0..records_in_chunk {
            let offset = i * record_size;
            let end = offset + record_size;
            if end > chunk_data.len() {
                break;
            }

            // Clone to allow fixup (matches Windows LIVE behavior)
            let mut record_buf = chunk_data[offset..end].to_vec();
            if !apply_fixup(&mut record_buf) {
                fixup_failed += 1;
                continue;
            }

            let frs = chunk.start_frs + i as u64;

            if parse_record_to_index(&record_buf, frs, &mut index) {
                records_parsed += 1;
            }
        }
    }

    debug!(
        records_parsed,
        fixup_failed,
        index_entries = index.records.len(),
        "[PARITY_TRACE] inline parsing complete"
    );
    info!(
        records_parsed,
        fixup_failed,
        index_entries = index.records.len(),
        "Inline parsing complete"
    );

    // Compute tree metrics - same as Windows LIVE SlidingIocpInline path
    debug!(
        records = index.records.len(),
        "[PARITY_TRACE] CALLING compute_tree_metrics()"
    );
    let tree_start = std::time::Instant::now();
    index.compute_tree_metrics();
    debug!(
        tree_metrics_ms = tree_start.elapsed().as_millis(),
        "[PARITY_TRACE] compute_tree_metrics() done"
    );

    debug!(
        records = index.records.len(),
        "[PARITY_TRACE] load_iocp_to_index: EXIT"
    );
    Ok(index)
}
