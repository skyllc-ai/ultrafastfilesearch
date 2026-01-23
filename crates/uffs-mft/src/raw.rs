//! RAW MFT Persistence.
//!
//! This module provides functionality to save and load complete raw MFT bytes
//! for offline analysis. This allows:
//! - Saving MFT data without requiring admin privileges later
//! - Analyzing MFT data on non-Windows systems
//! - Sharing MFT snapshots for forensic analysis
//!
//! # File Format
//!
//! The raw MFT file format is:
//! ```text
//! [Header: 64 bytes]
//!   - Magic: "UFFS-MFT" (8 bytes)
//!   - Version: u32 (4 bytes)
//!   - Flags: u32 (4 bytes) - bit 0: compressed
//!   - Record size: u32 (4 bytes)
//!   - Record count: u64 (8 bytes)
//!   - Original size: u64 (8 bytes) - uncompressed size
//!   - Compressed size: u64 (8 bytes) - 0 if not compressed
//!   - Reserved: 20 bytes
//! [Data: variable]
//!   - Raw MFT bytes (optionally zstd compressed)
//! ```

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::error::{MftError, Result};

/// Magic bytes for raw MFT file format.
const MAGIC: &[u8; 8] = b"UFFS-MFT";

/// Current file format version.
const VERSION: u32 = 1;

/// Flag: data is zstd compressed.
const FLAG_COMPRESSED: u32 = 0x0001;

/// Header size in bytes.
const HEADER_SIZE: usize = 64;

/// Raw MFT file header.
#[derive(Debug, Clone)]
pub struct RawMftHeader {
    /// File format version.
    pub version: u32,
    /// Flags (bit 0: compressed).
    pub flags: u32,
    /// Size of each MFT record in bytes.
    pub record_size: u32,
    /// Number of MFT records.
    pub record_count: u64,
    /// Original uncompressed size.
    pub original_size: u64,
    /// Compressed size (0 if not compressed).
    pub compressed_size: u64,
}

impl RawMftHeader {
    /// Returns true if the data is compressed.
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }

    /// Serializes the header to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0_u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(MAGIC);
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.flags.to_le_bytes());
        buf[16..20].copy_from_slice(&self.record_size.to_le_bytes());
        buf[20..28].copy_from_slice(&self.record_count.to_le_bytes());
        buf[28..36].copy_from_slice(&self.original_size.to_le_bytes());
        buf[36..44].copy_from_slice(&self.compressed_size.to_le_bytes());
        // Reserved bytes 44-63 are already zero
        buf
    }

    /// Deserializes the header from bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic bytes don't match or version is
    /// unsupported.
    pub fn from_bytes(buf: &[u8; HEADER_SIZE]) -> Result<Self> {
        // Check magic
        if &buf[0..8] != MAGIC {
            return Err(MftError::InvalidData("Invalid raw MFT file magic".into()));
        }

        let version = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        if version > VERSION {
            return Err(MftError::InvalidData(format!(
                "Unsupported raw MFT version: {version}"
            )));
        }

        let flags = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let record_size = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let record_count = u64::from_le_bytes([
            buf[20], buf[21], buf[22], buf[23], buf[24], buf[25], buf[26], buf[27],
        ]);
        let original_size = u64::from_le_bytes([
            buf[28], buf[29], buf[30], buf[31], buf[32], buf[33], buf[34], buf[35],
        ]);
        let compressed_size = u64::from_le_bytes([
            buf[36], buf[37], buf[38], buf[39], buf[40], buf[41], buf[42], buf[43],
        ]);

        Ok(Self {
            version,
            flags,
            record_size,
            record_count,
            original_size,
            compressed_size,
        })
    }
}

/// Options for saving raw MFT.
#[derive(Debug, Clone)]
pub struct SaveRawOptions {
    /// Whether to compress the data with zstd.
    pub compress: bool,
    /// Compression level (1-22, default 3).
    pub compression_level: i32,
}

impl Default for SaveRawOptions {
    fn default() -> Self {
        Self {
            compress: true,
            compression_level: 3,
        }
    }
}

/// Options for loading raw MFT.
#[derive(Debug, Clone, Default)]
pub struct LoadRawOptions {
    /// If true, only load the header without reading data.
    pub header_only: bool,
}

/// Loaded raw MFT data.
#[derive(Debug)]
pub struct RawMftData {
    /// Header information.
    pub header: RawMftHeader,
    /// Raw MFT bytes (decompressed if was compressed).
    pub data: Vec<u8>,
}

impl RawMftData {
    /// Returns the number of records.
    #[must_use]
    pub const fn record_count(&self) -> u64 {
        self.header.record_count
    }

    /// Returns the record size.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.header.record_size
    }

    /// Returns a slice of a single record by FRS.
    ///
    /// # Returns
    ///
    /// `None` if FRS is out of range.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn get_record(&self, frs: u64) -> Option<&[u8]> {
        let record_size = self.header.record_size as usize;
        let offset = frs as usize * record_size;
        self.data.get(offset..offset + record_size)
    }

    /// Returns an iterator over all records.
    #[allow(clippy::cast_possible_truncation)]
    pub fn iter_records(&self) -> impl Iterator<Item = (u64, &[u8])> {
        let record_size = self.header.record_size as usize;
        self.data
            .chunks_exact(record_size)
            .enumerate()
            .map(|(i, data)| (i as u64, data))
    }
}

/// Saves raw MFT bytes to a file.
///
/// This function writes the complete MFT data to a file that can be
/// loaded later for offline analysis.
///
/// # Arguments
///
/// * `path` - Output file path
/// * `data` - Raw MFT bytes (must be contiguous, all records concatenated)
/// * `record_size` - Size of each MFT record in bytes
/// * `options` - Save options (compression, etc.)
///
/// # Errors
///
/// Returns an error if writing fails.
#[allow(clippy::shadow_reuse)]
pub fn save_raw_mft<P: AsRef<Path>>(
    path: P,
    data: &[u8],
    record_size: u32,
    options: &SaveRawOptions,
) -> Result<RawMftHeader> {
    let path = path.as_ref();
    #[allow(clippy::cast_possible_truncation)]
    let original_size = data.len() as u64;
    let record_count = original_size / u64::from(record_size);

    // Prepare data (compress if requested)
    #[allow(unused_mut)]
    let mut flags = 0_u32;
    #[allow(unused_mut)]
    let mut compressed_size = 0_u64;

    #[cfg(feature = "zstd")]
    let write_data: Vec<u8> = if options.compress {
        #[allow(clippy::cast_possible_truncation)]
        let compressed = zstd::encode_all(data, options.compression_level)
            .map_err(|err| MftError::Io(std::io::Error::other(err)))?;
        flags |= FLAG_COMPRESSED;
        compressed_size = compressed.len() as u64;
        compressed
    } else {
        data.to_vec()
    };

    #[cfg(not(feature = "zstd"))]
    if options.compress {
        return Err(MftError::InvalidData(
            "zstd feature not enabled for compression".into(),
        ));
    }

    // Create header
    let header = RawMftHeader {
        version: VERSION,
        flags,
        record_size,
        record_count,
        original_size,
        compressed_size,
    };

    // Write file
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(&header.to_bytes())?;
    #[cfg(feature = "zstd")]
    writer.write_all(&write_data)?;
    #[cfg(not(feature = "zstd"))]
    writer.write_all(data)?;
    writer.flush()?;

    Ok(header)
}

/// Loads raw MFT bytes from a file.
///
/// # Arguments
///
/// * `path` - Input file path
/// * `options` - Load options
///
/// # Errors
///
/// Returns an error if reading fails or file format is invalid.
#[allow(clippy::shadow_reuse)]
pub fn load_raw_mft<P: AsRef<Path>>(path: P, options: &LoadRawOptions) -> Result<RawMftData> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    // Read header
    let mut header_buf = [0_u8; HEADER_SIZE];
    reader.read_exact(&mut header_buf)?;
    let header = RawMftHeader::from_bytes(&header_buf)?;

    // If header-only mode, return empty data
    if options.header_only {
        return Ok(RawMftData {
            header,
            data: Vec::new(),
        });
    }

    // Read data
    let mut compressed_data = Vec::new();
    reader.read_to_end(&mut compressed_data)?;

    // Decompress if needed
    let data = if header.is_compressed() {
        #[cfg(feature = "zstd")]
        {
            zstd::decode_all(&compressed_data[..])
                .map_err(|err| MftError::Io(std::io::Error::other(err)))?
        }
        #[cfg(not(feature = "zstd"))]
        {
            return Err(MftError::InvalidData(
                "zstd feature not enabled for decompression".into(),
            ));
        }
    } else {
        compressed_data
    };

    // Validate size
    let expected_size = header.record_count * u64::from(header.record_size);
    if data.len() as u64 != expected_size {
        return Err(MftError::InvalidData(format!(
            "Data size mismatch: expected {expected_size}, got {}",
            data.len()
        )));
    }

    Ok(RawMftData { header, data })
}

/// Loads only the header from a raw MFT file.
///
/// This is useful for inspecting file metadata without loading the full data.
///
/// # Errors
///
/// Returns an error if reading fails or file format is invalid.
pub fn load_raw_mft_header<P: AsRef<Path>>(path: P) -> Result<RawMftHeader> {
    let result = load_raw_mft(path, &LoadRawOptions { header_only: true })?;
    Ok(result.header)
}

// ============================================================================
// Streaming Raw MFT Writer (for high-performance saves)
// ============================================================================

/// A streaming writer for raw MFT data.
///
/// This writer enables high-performance saves by writing chunks as they are
/// read, rather than buffering the entire MFT in memory. This is critical
/// for large MFTs (10+ GB) where buffering would be prohibitively expensive.
///
/// # Usage
///
/// ```ignore
/// let mut writer = StreamingRawMftWriter::new(path, record_size, estimated_records, options)?;
/// for chunk in chunks {
///     let data = read_chunk(...);
///     writer.write_chunk(&data)?;
/// }
/// let header = writer.finish()?;
/// ```
pub struct StreamingRawMftWriter {
    /// The underlying file writer.
    writer: BufWriter<File>,
    /// Record size in bytes.
    record_size: u32,
    /// Total bytes written (for record count calculation).
    bytes_written: u64,
    /// Whether compression is enabled.
    compress: bool,
    /// Compression level (if compressing).
    #[allow(dead_code)]
    compression_level: i32,
    /// Zstd encoder (if compressing).
    #[cfg(feature = "zstd")]
    encoder: Option<zstd::stream::Encoder<'static, BufWriter<File>>>,
}

impl StreamingRawMftWriter {
    /// Creates a new streaming writer.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    /// * `record_size` - Size of each MFT record in bytes
    /// * `options` - Save options (compression, etc.)
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created.
    pub fn new<P: AsRef<Path>>(
        path: P,
        record_size: u32,
        options: &SaveRawOptions,
    ) -> Result<Self> {
        let output_file = File::create(path.as_ref())?;
        let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, output_file); // 8MB buffer

        // Write placeholder header (will be updated in finish())
        let placeholder_header = RawMftHeader {
            version: VERSION,
            flags: if options.compress { FLAG_COMPRESSED } else { 0 },
            record_size,
            record_count: 0,
            original_size: 0,
            compressed_size: 0,
        };
        writer.write_all(&placeholder_header.to_bytes())?;

        #[cfg(feature = "zstd")]
        if options.compress {
            // For compressed output, we need to use a zstd encoder
            // Flush the header first, then create encoder for the data portion
            writer.flush()?;
            let inner_file = writer
                .into_inner()
                .map_err(|err| MftError::Io(err.into_error()))?;
            let encoder = zstd::stream::Encoder::new(
                BufWriter::with_capacity(8 * 1024 * 1024, inner_file),
                options.compression_level,
            )
            .map_err(|err| MftError::Io(std::io::Error::other(err)))?;

            // Create a dummy writer - we use encoder instead for compressed output
            // We need a valid File handle, so create a temp file that we'll never use
            let temp_path = std::env::temp_dir().join(".uffs_streaming_dummy");
            let dummy_file = File::create(&temp_path)?;

            return Ok(Self {
                writer: BufWriter::new(dummy_file),
                record_size,
                bytes_written: 0,
                compress: true,
                compression_level: options.compression_level,
                encoder: Some(encoder),
            });
        }

        Ok(Self {
            writer,
            record_size,
            bytes_written: 0,
            compress: options.compress,
            compression_level: options.compression_level,
            #[cfg(feature = "zstd")]
            encoder: None,
        })
    }

    /// Writes a chunk of raw MFT data.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw MFT bytes to write
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    pub fn write_chunk(&mut self, data: &[u8]) -> Result<()> {
        #[cfg(feature = "zstd")]
        if let Some(ref mut encoder) = self.encoder {
            encoder
                .write_all(data)
                .map_err(|err| MftError::Io(std::io::Error::other(err)))?;
            self.bytes_written += data.len() as u64;
            return Ok(());
        }

        self.writer.write_all(data)?;
        self.bytes_written += data.len() as u64;
        Ok(())
    }

    /// Finishes writing and returns the final header.
    ///
    /// This updates the header with the actual record count and sizes,
    /// then flushes all buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if flushing or seeking fails.
    pub fn finish(mut self) -> Result<RawMftHeader> {
        use std::io::{Seek, SeekFrom};

        let original_size = self.bytes_written;
        let record_count = original_size / u64::from(self.record_size);

        #[cfg(feature = "zstd")]
        let compressed_size = if let Some(encoder) = self.encoder.take() {
            let mut zstd_writer = encoder
                .finish()
                .map_err(|err| MftError::Io(std::io::Error::other(err)))?;
            zstd_writer.flush()?;
            let zstd_file = zstd_writer
                .into_inner()
                .map_err(|err| MftError::Io(err.into_error()))?;
            let metadata = zstd_file.metadata()?;
            // Compressed size is file size minus header
            metadata.len().saturating_sub(HEADER_SIZE as u64)
        } else {
            0
        };

        #[cfg(not(feature = "zstd"))]
        let compressed_size = 0_u64;

        // Create final header
        let header = RawMftHeader {
            version: VERSION,
            flags: if self.compress { FLAG_COMPRESSED } else { 0 },
            record_size: self.record_size,
            record_count,
            original_size,
            compressed_size,
        };

        // For uncompressed, update the header at the beginning of the file
        if !self.compress {
            self.writer.flush()?;
            let mut output_file = self
                .writer
                .into_inner()
                .map_err(|err| MftError::Io(err.into_error()))?;

            // Seek to beginning and write final header
            output_file.seek(SeekFrom::Start(0))?;
            output_file.write_all(&header.to_bytes())?;
            output_file.flush()?;
        }

        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    #[test]
    fn test_header_roundtrip() -> TestResult {
        let header = RawMftHeader {
            version: VERSION,
            flags: FLAG_COMPRESSED,
            record_size: 1024,
            record_count: 1000,
            original_size: 1024 * 1000,
            compressed_size: 500_000,
        };

        let bytes = header.to_bytes();
        let parsed = RawMftHeader::from_bytes(&bytes)?;

        assert_eq!(parsed.version, header.version);
        assert_eq!(parsed.flags, header.flags);
        assert_eq!(parsed.record_size, header.record_size);
        assert_eq!(parsed.record_count, header.record_count);
        assert_eq!(parsed.original_size, header.original_size);
        assert_eq!(parsed.compressed_size, header.compressed_size);
        assert!(parsed.is_compressed());

        Ok(())
    }

    #[test]
    fn test_header_invalid_magic() {
        let mut bytes = [0_u8; HEADER_SIZE];
        bytes[0..8].copy_from_slice(b"INVALID!");

        let result = RawMftHeader::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn test_save_load_uncompressed() -> TestResult {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_mft_uncompressed.raw");

        // Create test data (4 records of 1024 bytes each)
        let record_size = 1024_u32;
        let mut data = vec![0_u8; 4 * record_size as usize];
        // Mark each record with its FRS
        for i in 0_u8..4 {
            data[usize::from(i) * record_size as usize] = i;
        }

        // Save uncompressed
        let options = SaveRawOptions {
            compress: false,
            compression_level: 3,
        };
        let header = save_raw_mft(&path, &data, record_size, &options)?;

        assert_eq!(header.record_count, 4);
        assert_eq!(header.record_size, record_size);
        assert!(!header.is_compressed());

        // Load and verify
        let loaded = load_raw_mft(&path, &LoadRawOptions::default())?;
        assert_eq!(loaded.data.len(), data.len());
        assert_eq!(loaded.data, data);

        // Test get_record
        for i in 0_u8..4 {
            let record = loaded.get_record(u64::from(i)).ok_or("Record not found")?;
            assert_eq!(record[0], i);
        }

        // Cleanup
        std::fs::remove_file(&path)?;

        Ok(())
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn test_save_load_compressed() -> TestResult {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_mft_compressed.raw");

        // Create test data with some repetition for good compression
        let record_size = 1024_usize;
        let record_count = 100_usize;
        let mut data = vec![0xAB_u8; record_count * record_size];
        // Mark each record with a unique byte pattern
        for idx in 0..record_count {
            if let Some(byte) = data.get_mut(idx * record_size) {
                *byte = u8::try_from(idx % 256).unwrap_or(0);
            }
        }

        // Save compressed
        #[allow(clippy::cast_possible_truncation)]
        let record_size_u32 = record_size as u32;
        let options = SaveRawOptions::default(); // compress: true
        let header = save_raw_mft(&path, &data, record_size_u32, &options)?;

        assert_eq!(header.record_count, 100);
        assert!(header.is_compressed());
        assert!(header.compressed_size < header.original_size);

        // Load and verify
        let loaded = load_raw_mft(&path, &LoadRawOptions::default())?;
        assert_eq!(loaded.data.len(), data.len());
        assert_eq!(loaded.data, data);

        // Cleanup
        std::fs::remove_file(&path)?;

        Ok(())
    }

    #[test]
    fn test_load_header_only() -> TestResult {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_mft_header_only.raw");

        let record_size = 1024_u32;
        let data = vec![0_u8; 10 * record_size as usize];

        let options = SaveRawOptions {
            compress: false,
            compression_level: 3,
        };
        save_raw_mft(&path, &data, record_size, &options)?;

        // Load header only
        let header = load_raw_mft_header(&path)?;
        assert_eq!(header.record_count, 10);
        assert_eq!(header.record_size, record_size);

        // Cleanup
        std::fs::remove_file(&path)?;

        Ok(())
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn test_iter_records() {
        let header = RawMftHeader {
            version: VERSION,
            flags: 0,
            record_size: 4,
            record_count: 3,
            original_size: 12,
            compressed_size: 0,
        };

        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let raw = RawMftData { header, data };

        let records: Vec<_> = raw.iter_records().collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0], (0, &[1, 2, 3, 4][..]));
        assert_eq!(records[1], (1, &[5, 6, 7, 8][..]));
        assert_eq!(records[2], (2, &[9, 10, 11, 12][..]));
    }
}
