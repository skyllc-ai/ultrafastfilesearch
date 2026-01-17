//! MFT Reader implementation.
//!
//! This module provides the main entry point for reading NTFS MFT data.

use core::time::Duration;
use std::path::Path;
#[cfg(windows)]
use std::time::Instant;

use uffs_polars::{DataFrame, ParquetReader, ParquetWriter, SerReader};

use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::platform::VolumeHandle;

/// Progress information during MFT reading.
#[derive(Debug, Clone)]
pub struct MftProgress {
    /// Number of records read so far.
    pub records_read: u64,
    /// Total number of records (if known).
    pub total_records: Option<u64>,
    /// Bytes read from disk.
    pub bytes_read: u64,
    /// Time elapsed since start.
    pub elapsed: Duration,
}

impl MftProgress {
    /// Returns the percentage complete (0.0 to 100.0), if total is known.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn percentage(&self) -> Option<f64> {
        self.total_records
            .map(|total| (self.records_read as f64 / total as f64) * 100.0_f64)
    }

    /// Returns the read speed in MB/s.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn speed_mbps(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            (self.bytes_read as f64 / 1_048_576.0) / secs
        } else {
            0.0
        }
    }
}

/// MFT Reader for direct NTFS Master File Table access.
///
/// This struct provides high-performance MFT reading by bypassing
/// Windows file enumeration APIs and reading the MFT directly.
///
/// # Platform Support
///
/// MFT reading is only available on Windows. On other platforms,
/// methods will return `MftError::PlatformNotSupported`.
///
/// # Privileges
///
/// Reading the MFT requires Administrator privileges. The process
/// must be elevated or have `SE_BACKUP_PRIVILEGE`.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::MftReader;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MftReader::open('C').await?;
///     let df = reader.read_all().await?;
///     println!("Found {} files", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct MftReader {
    /// The volume letter (e.g., 'C').
    volume: char,
    /// The volume handle (Windows only).
    #[cfg(windows)]
    handle: VolumeHandle,
}

impl MftReader {
    /// Open a volume for MFT reading.
    ///
    /// # Arguments
    ///
    /// * `volume` - The drive letter (e.g., 'C', 'D')
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The volume cannot be opened
    /// - The volume is not NTFS formatted
    /// - Insufficient privileges (not running as Administrator)
    ///
    /// # Platform
    ///
    /// This function is only available on Windows.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn open(volume: char) -> Result<Self> {
        // Open the volume handle (validates NTFS and privileges)
        let handle = VolumeHandle::open(volume)?;

        Ok(Self {
            volume: volume.to_ascii_uppercase(),
            handle,
        })
    }

    /// Open a volume for MFT reading (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn open(_volume: char) -> Result<Self> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read the entire MFT and return as a `DataFrame`.
    ///
    /// This method reads all MFT records and constructs a Polars `DataFrame`
    /// with the standard schema (frs, `parent_frs`, name, size, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_mft_internal(None::<fn(MftProgress)>)
    }

    /// Read the entire MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT with progress callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called periodically with progress updates
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_internal(Some(callback))
    }

    /// Read MFT with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal MFT reading implementation.
    ///
    /// This implementation uses the high-performance parallel reader with:
    /// 1. Extent-aware reading for fragmented MFTs
    /// 2. Bitmap-based cluster skipping (like C++ implementation)
    /// 3. Parallel record processing using Rayon
    /// 4. Batch I/O for reduced syscall overhead
    #[cfg(windows)]
    fn read_mft_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(MftProgress),
    {
        use crate::io::{MftExtentMap, ParallelMftReader};

        let start_time = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            // Fallback to single contiguous extent
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        // Create extent map
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        let total_records = extent_map.total_records();

        // Try to get the MFT bitmap for optimization
        let bitmap = self.handle.get_mft_bitmap().ok();

        // Report initial progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: 0,
                total_records: Some(total_records),
                bytes_read: 0,
                elapsed: start_time.elapsed(),
            });
        }

        // Use the high-performance parallel reader
        let parallel_reader = ParallelMftReader::new(extent_map, bitmap);
        let handle = self.handle.raw_handle();

        // Read all records in parallel with extension merging for full C++ parity
        let parsed_records = parallel_reader.read_all_parallel_with_merge(handle, true)?;

        // Report final progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: total_records,
                total_records: Some(total_records),
                bytes_read: total_records * u64::from(record_size),
                elapsed: start_time.elapsed(),
            });
        }

        // Convert parsed records to DataFrame columns (full C++ parity schema)
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        // Extended flags
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(parsed.name_count());
            stream_count_vec.push(parsed.stream_count());
            // Extended flags
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
        }

        // Build DataFrame with full schema
        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
        )
    }

    /// Builds a `DataFrame` from the collected vectors (legacy 8-column
    /// schema).
    #[cfg(windows)]
    #[allow(clippy::too_many_arguments, dead_code)]
    fn build_dataframe(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        flags_vec: Vec<u16>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, Series, TimeUnit};

        let columns = vec![
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            Series::new("size".into(), size_vec).into_column(),
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` with full C++ parity schema (23 columns).
    #[cfg(windows)]
    #[allow(clippy::too_many_arguments)]
    fn build_dataframe_full(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        allocated_size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        mft_changed_vec: Vec<i64>,
        is_directory_vec: Vec<bool>,
        name_count_vec: Vec<u16>,
        stream_count_vec: Vec<u16>,
        is_readonly_vec: Vec<bool>,
        is_hidden_vec: Vec<bool>,
        is_system_vec: Vec<bool>,
        is_archive_vec: Vec<bool>,
        is_compressed_vec: Vec<bool>,
        is_encrypted_vec: Vec<bool>,
        is_sparse_vec: Vec<bool>,
        is_reparse_vec: Vec<bool>,
        is_offline_vec: Vec<bool>,
        is_not_indexed_vec: Vec<bool>,
        is_temporary_vec: Vec<bool>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, Series, TimeUnit};

        let columns = vec![
            // Core identifiers
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            // Size information
            Series::new("size".into(), size_vec).into_column(),
            Series::new("allocated_size".into(), allocated_size_vec).into_column(),
            // Timestamps (4 total, matching C++)
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), mft_changed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            // Type and counts
            Series::new("is_directory".into(), is_directory_vec).into_column(),
            Series::new("name_count".into(), name_count_vec).into_column(),
            Series::new("stream_count".into(), stream_count_vec).into_column(),
            // Extended attribute flags (matching C++ StandardInfo)
            Series::new("is_readonly".into(), is_readonly_vec).into_column(),
            Series::new("is_hidden".into(), is_hidden_vec).into_column(),
            Series::new("is_system".into(), is_system_vec).into_column(),
            Series::new("is_archive".into(), is_archive_vec).into_column(),
            Series::new("is_compressed".into(), is_compressed_vec).into_column(),
            Series::new("is_encrypted".into(), is_encrypted_vec).into_column(),
            Series::new("is_sparse".into(), is_sparse_vec).into_column(),
            Series::new("is_reparse".into(), is_reparse_vec).into_column(),
            Series::new("is_offline".into(), is_offline_vec).into_column(),
            Series::new("is_not_indexed".into(), is_not_indexed_vec).into_column(),
            Series::new("is_temporary".into(), is_temporary_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Save a `DataFrame` to Parquet format.
    ///
    /// Parquet provides excellent compression and fast loading times.
    ///
    /// # Arguments
    ///
    /// * `df` - The `DataFrame` to save
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save_parquet<P: AsRef<Path>>(df: &mut DataFrame, path: P) -> Result<()> {
        let file = std::fs::File::create(path.as_ref())?;
        ParquetWriter::new(file)
            .finish(df)
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(())
    }

    /// Load a `DataFrame` from Parquet format.
    ///
    /// # Arguments
    ///
    /// * `path` - Input file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is invalid.
    pub fn load_parquet<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        let file = std::fs::File::open(path.as_ref())?;
        let df = ParquetReader::new(file)
            .finish()
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(df)
    }

    /// Get the volume letter this reader is attached to.
    #[must_use]
    pub const fn volume(&self) -> char {
        self.volume
    }

    /// Create an empty `DataFrame` with the MFT schema.
    #[allow(dead_code)]
    fn create_empty_dataframe() -> Result<DataFrame> {
        use uffs_polars::{Column, DataType, TimeUnit};

        let schema_columns = vec![
            Column::new_empty("frs".into(), &DataType::UInt64),
            Column::new_empty("parent_frs".into(), &DataType::UInt64),
            Column::new_empty("name".into(), &DataType::String),
            Column::new_empty("size".into(), &DataType::UInt64),
            Column::new_empty(
                "created".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "modified".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "accessed".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty("flags".into(), &DataType::UInt16),
        ];

        // Use new_infer_height to infer height from columns (Polars 0.52+ API)
        DataFrame::new_infer_height(schema_columns).map_err(MftError::from)
    }

    // ========================================================================
    // RAW MFT Persistence
    // ========================================================================

    /// Read the entire MFT as raw bytes.
    ///
    /// This reads all MFT records as contiguous raw bytes, handling fragmented
    /// MFTs by reassembling extents in order. The result can be saved with
    /// [`save_raw_mft`](crate::raw::save_raw_mft) for offline analysis.
    ///
    /// # Returns
    ///
    /// A tuple of (raw bytes, record size).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        self.read_raw_internal()
    }

    /// Read raw MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal raw MFT reading implementation.
    #[cfg(windows)]
    fn read_raw_internal(&self) -> Result<(Vec<u8>, u32)> {
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        use crate::io::{AlignedBuffer, MftExtentMap, SECTOR_SIZE, generate_read_chunks};

        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        // Create extent map
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();

        // Allocate output buffer
        let total_size = total_records as usize * record_size as usize;
        let mut output = vec![0u8; total_size];

        // Generate read chunks (without bitmap - we want ALL records)
        let chunks = generate_read_chunks(&extent_map, None, 1024 * 1024);

        // Read buffer (aligned for direct I/O)
        let chunk_size = 1024 * 1024usize;
        let aligned_size = ((chunk_size + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let mut buffer = AlignedBuffer::new(aligned_size);

        let handle = self.handle.raw_handle();

        // Read each chunk
        for chunk in chunks {
            let read_size = chunk.record_count as usize * record_size as usize;
            let aligned_read = ((read_size + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;

            // Seek to chunk position
            let mut new_pos = 0i64;
            unsafe {
                SetFilePointerEx(
                    handle,
                    chunk.disk_offset as i64,
                    Some(&mut new_pos),
                    FILE_BEGIN,
                )?;
            }

            // Read chunk
            let mut bytes_read = 0u32;
            unsafe {
                ReadFile(
                    handle,
                    Some(&mut buffer.as_mut_slice()[..aligned_read]),
                    Some(&mut bytes_read),
                    None,
                )?;
            }

            // Copy to output at correct position
            let output_offset = chunk.start_frs as usize * record_size as usize;
            let copy_size = read_size.min(bytes_read as usize);
            output[output_offset..output_offset + copy_size]
                .copy_from_slice(&buffer.as_slice()[..copy_size]);
        }

        Ok((output, record_size))
    }

    /// Read raw MFT and save to file.
    ///
    /// This is a convenience method that reads the MFT and saves it in one
    /// step.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    /// * `options` - Save options (compression, etc.)
    ///
    /// # Errors
    ///
    /// Returns an error if reading or saving fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        let (data, record_size) = self.read_raw_internal()?;
        crate::raw::save_raw_mft(path, &data, record_size, options)
    }

    /// Save raw MFT to file (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        _path: P,
        _options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        Err(MftError::PlatformNotSupported)
    }

    /// Load raw MFT from file and parse to DataFrame.
    ///
    /// This loads a previously saved raw MFT file and parses it into a
    /// DataFrame.
    ///
    /// # Arguments
    ///
    /// * `path` - Input file path
    ///
    /// # Errors
    ///
    /// Returns an error if loading or parsing fails.
    ///
    /// # Platform
    ///
    /// Currently only available on Windows due to NTFS parsing dependencies.
    #[cfg(windows)]
    pub fn load_raw_to_dataframe<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        use crate::io::{MftRecordMerger, apply_fixup, parse_record_full};

        let raw = crate::raw::load_raw_mft(path, &crate::raw::LoadRawOptions::default())?;

        // Parse all records
        let mut merger = MftRecordMerger::with_capacity(raw.header.record_count as usize);

        for (frs, record_data) in raw.iter_records() {
            let mut record_buf = record_data.to_vec();

            // Apply fixup
            if !apply_fixup(&mut record_buf) {
                continue;
            }

            // Parse record
            let result = parse_record_full(&record_buf, frs);
            merger.add_result(result);
        }

        // Merge extensions and get final records
        let parsed_records = merger.finalize();

        // Convert to DataFrame (same as read_mft_internal)
        Self::parsed_records_to_dataframe(parsed_records)
    }

    /// Load raw MFT from file (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn load_raw_to_dataframe<P: AsRef<Path>>(_path: P) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Convert parsed records to DataFrame.
    #[cfg(windows)]
    fn parsed_records_to_dataframe(
        parsed_records: Vec<crate::io::ParsedRecord>,
    ) -> Result<DataFrame> {
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(parsed.name_count());
            stream_count_vec.push(parsed.stream_count());
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
        }

        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
        )
    }
}

// ============================================================================
// Multi-Drive MFT Reader
// ============================================================================

/// Result from reading a single drive.
#[derive(Debug)]
pub struct DriveReadResult {
    /// The drive letter.
    pub drive: char,
    /// The `DataFrame` (if successful).
    pub dataframe: Option<DataFrame>,
    /// The error (if failed).
    pub error: Option<MftError>,
}

/// Reads MFTs from multiple drives concurrently.
///
/// This struct orchestrates parallel reading of MFTs from multiple NTFS
/// volumes, merging the results into a single `DataFrame` with a `drive` column
/// to distinguish the source of each record.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::MultiDriveMftReader;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MultiDriveMftReader::new(vec!['C', 'D', 'E']);
///     let df = reader.read_all().await?;
///     println!("Found {} files across all drives", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct MultiDriveMftReader {
    /// The drive letters to read from.
    drives: Vec<char>,
}

impl MultiDriveMftReader {
    /// Creates a new multi-drive reader.
    ///
    /// # Arguments
    ///
    /// * `drives` - List of drive letters to read (e.g., `vec!['C', 'D', 'E']`)
    #[must_use]
    pub fn new(drives: Vec<char>) -> Self {
        Self {
            drives: drives
                .into_iter()
                .map(|ch| ch.to_ascii_uppercase())
                .collect(),
        }
    }

    /// Returns the list of drives this reader will process.
    #[must_use]
    pub fn drives(&self) -> &[char] {
        &self.drives
    }

    /// Read MFTs from all drives concurrently.
    ///
    /// Returns a merged DataFrame with a `drive` column (e.g., "C:", "D:").
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_all_internal(None::<fn(char, MftProgress)>).await
    }

    /// Read MFTs from all drives (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with per-drive progress callbacks.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called with progress updates for each drive
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_internal(Some(callback)).await
    }

    /// Read MFTs with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for concurrent drive reading.
    #[cfg(windows)]
    async fn read_all_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        use std::sync::Arc;

        use tokio::task::JoinSet;
        use uffs_polars::{IntoLazy, col, lit};

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        // Wrap callback in Arc for sharing across tasks
        let callback = callback.map(Arc::new);

        // Spawn a task for each drive
        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            let cb = callback.clone();

            join_set.spawn(async move {
                let result = Self::read_single_drive(drive, cb).await;
                DriveReadResult {
                    drive,
                    dataframe: result.as_ref().ok().cloned(),
                    error: result.err(),
                }
            });
        }

        // Collect results
        let mut dataframes: Vec<DataFrame> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => {
                    if let Some(df) = drive_result.dataframe {
                        // Add "drive" column
                        let drive_str = format!("{}:", drive_result.drive);
                        let df_with_drive = df
                            .lazy()
                            .with_column(lit(drive_str).alias("drive"))
                            .collect()
                            .map_err(MftError::from)?;
                        dataframes.push(df_with_drive);
                    } else if let Some(err) = drive_result.error {
                        errors.push((drive_result.drive, err));
                    }
                }
                Err(join_err) => {
                    // Task panicked or was cancelled
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }
        }

        // If no DataFrames were collected, return the first error
        if dataframes.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        // Concatenate all DataFrames using vstack
        let mut result = dataframes.remove(0);
        for df in dataframes {
            result = result.vstack(&df).map_err(MftError::from)?;
        }

        // Reorder columns to put "drive" first
        let columns: Vec<_> = std::iter::once("drive")
            .chain(
                result
                    .get_column_names()
                    .into_iter()
                    .filter(|&c| c != "drive"),
            )
            .map(|s| col(s))
            .collect();

        result
            .lazy()
            .select(columns)
            .collect()
            .map_err(MftError::from)
    }

    /// Read a single drive with optional progress callback.
    #[cfg(windows)]
    async fn read_single_drive<F>(drive: char, callback: Option<Arc<F>>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        let reader = MftReader::open(drive).await?;

        if let Some(cb) = callback {
            reader
                .read_with_progress(move |progress| {
                    cb(drive, progress);
                })
                .await
        } else {
            reader.read_all().await
        }
    }

    /// Read all drives and return individual results (for detailed error
    /// handling).
    ///
    /// Unlike `read_all()`, this returns results for each drive separately,
    /// allowing the caller to handle partial failures.
    ///
    /// # Errors
    ///
    /// Returns an error only if the operation itself fails (not individual
    /// drives).
    #[cfg(windows)]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        use std::sync::Arc;

        use tokio::task::JoinSet;

        if self.drives.is_empty() {
            return Ok(Vec::new());
        }

        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            join_set.spawn(async move {
                let result = Self::read_single_drive::<fn(char, MftProgress)>(drive, None).await;
                DriveReadResult {
                    drive,
                    dataframe: result.as_ref().ok().cloned(),
                    error: result.err(),
                }
            });
        }

        let mut results = Vec::with_capacity(self.drives.len());
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => results.push(drive_result),
                Err(join_err) => {
                    results.push(DriveReadResult {
                        drive: '?',
                        dataframe: None,
                        error: Some(MftError::InvalidInput(format!("Task failed: {join_err}"))),
                    });
                }
            }
        }

        Ok(results)
    }

    /// Read all drives detailed (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        Err(MftError::PlatformNotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(windows)]
    async fn test_open_valid_volume() {
        let result = MftReader::open('C').await;
        // This will fail without admin privileges, but should not panic
        assert!(result.is_ok() || matches!(result, Err(MftError::InsufficientPrivileges)));
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_platform_not_supported() {
        let result = MftReader::open('C').await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }

    #[test]
    fn test_progress_percentage() {
        let progress = MftProgress {
            records_read: 50,
            total_records: Some(100),
            bytes_read: 1024,
            elapsed: Duration::from_secs(1),
        };
        assert_eq!(progress.percentage(), Some(50.0_f64));
    }

    #[test]
    fn test_progress_speed() {
        let progress = MftProgress {
            records_read: 1000,
            total_records: None,
            bytes_read: 10 * 1024 * 1024, // 10 MB
            elapsed: Duration::from_secs(2),
        };
        assert!((progress.speed_mbps() - 5.0_f64).abs() < 0.01_f64);
    }

    #[test]
    fn test_multi_drive_reader_new() {
        let reader = MultiDriveMftReader::new(vec!['c', 'd', 'e']);
        assert_eq!(reader.drives(), &['C', 'D', 'E']);
    }

    #[test]
    fn test_multi_drive_reader_empty() {
        let reader = MultiDriveMftReader::new(vec![]);
        assert!(reader.drives().is_empty());
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_multi_drive_platform_not_supported() {
        let reader = MultiDriveMftReader::new(vec!['C', 'D']);
        let result = reader.read_all().await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }
}
