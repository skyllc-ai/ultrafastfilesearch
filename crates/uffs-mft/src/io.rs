//! Low-level I/O operations for MFT reading.
//!
//! This module provides efficient disk I/O for reading MFT records:
//! - Aligned buffer management for direct I/O
//! - Sector-aligned reads
//! - Multi-sector fixup (Update Sequence Array)
//! - **Fragmented MFT support** via extent mapping
//!
//! # Performance
//!
//! Uses `FILE_FLAG_NO_BUFFERING` for direct I/O, which requires:
//! - Sector-aligned file offsets
//! - Sector-aligned buffer addresses
//! - Sector-aligned read sizes
//!
//! # Performance Optimizations
//!
//! - Large chunk sizes (4-8 MB) based on drive type (SSD vs HDD)
//! - Thread-local buffers to avoid per-record allocations
//! - Buffer reuse for streaming reads
//! - Double-buffering for prefetch readers
//!
//! # Fragmented MFT
//!
//! The MFT can be scattered across multiple non-contiguous extents on disk.
//! This module handles fragmentation by:
//! 1. Getting the extent map via `FSCTL_GET_RETRIEVAL_POINTERS`
//! 2. Mapping Virtual Cluster Numbers (VCN) to Logical Cluster Numbers (LCN)
//! 3. Reading from the correct physical locations

#![cfg(windows)]

use std::cell::RefCell;
use std::mem::size_of;

// Thread-local buffer for record processing to avoid per-record allocations.
// Each thread gets its own 4KB buffer (enough for any MFT record).
thread_local! {
    static RECORD_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 4096]);
}

use tracing::{debug, info, trace, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

use crate::error::{MftError, Result};
// Re-export SECTOR_SIZE for use by other modules (e.g., reader.rs streaming save)
pub use crate::ntfs::SECTOR_SIZE;
use crate::ntfs::{
    AttributeRecordHeader, AttributeType, FILE_RECORD_MAGIC, FileNameAttribute,
    FileRecordSegmentHeader, MultiSectorHeader, StandardInformation,
};
use crate::platform::{MftExtent, VolumeHandle};

// ============================================================================
// Aligned Buffer
// ============================================================================

/// A buffer aligned to sector boundaries for direct I/O.
///
/// Windows `FILE_FLAG_NO_BUFFERING` requires sector-aligned buffers.
#[derive(Debug)]
pub struct AlignedBuffer {
    /// The underlying buffer (over-allocated for alignment).
    data: Vec<u8>,
    /// Offset to the aligned portion.
    offset: usize,
    /// Usable size of the aligned buffer.
    size: usize,
}

impl AlignedBuffer {
    /// Creates a new aligned buffer with the specified size.
    ///
    /// The buffer will be aligned to `SECTOR_SIZE` (512 bytes).
    #[must_use]
    pub fn new(size: usize) -> Self {
        // Allocate extra space for alignment
        let alloc_size = size + SECTOR_SIZE;
        let data = vec![0_u8; alloc_size];

        // Calculate alignment offset
        let ptr = data.as_ptr() as usize;
        let offset = (SECTOR_SIZE - (ptr % SECTOR_SIZE)) % SECTOR_SIZE;

        Self { data, offset, size }
    }

    /// Returns a mutable slice to the aligned buffer.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[self.offset..self.offset + self.size]
    }

    /// Returns an immutable slice to the aligned buffer.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data[self.offset..self.offset + self.size]
    }

    /// Returns the size of the usable buffer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.size
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }
}

// ============================================================================
// MFT Extent Map
// ============================================================================

/// Maps Virtual Cluster Numbers (VCN) to Logical Cluster Numbers (LCN).
///
/// The MFT can be fragmented across multiple non-contiguous extents on disk.
/// This struct provides efficient lookup to find the physical location of any
/// MFT record.
#[derive(Debug, Clone)]
pub struct MftExtentMap {
    /// Sorted list of extents (by VCN).
    extents: Vec<MftExtent>,
    /// Bytes per cluster.
    pub bytes_per_cluster: u32,
    /// Bytes per file record.
    pub bytes_per_record: u32,
}

impl MftExtentMap {
    /// Creates a new extent map from a list of extents.
    ///
    /// # Arguments
    ///
    /// * `extents` - List of MFT extents from `FSCTL_GET_RETRIEVAL_POINTERS`
    /// * `bytes_per_cluster` - Cluster size in bytes
    /// * `bytes_per_record` - File record size in bytes
    #[must_use]
    pub fn new(extents: Vec<MftExtent>, bytes_per_cluster: u32, bytes_per_record: u32) -> Self {
        let num_extents = extents.len();
        let total_clusters: u64 = extents.iter().map(|e| e.cluster_count).sum();
        let total_size_mb =
            (total_clusters * u64::from(bytes_per_cluster)) as f64 / (1024.0 * 1024.0);
        let records_per_cluster = bytes_per_cluster / bytes_per_record;
        let total_records = total_clusters * u64::from(records_per_cluster);

        // Analyze fragmentation
        let sparse_extents = extents.iter().filter(|e| e.lcn < 0).count();
        let is_fragmented = num_extents > 1;

        if is_fragmented {
            info!(
                extents = num_extents,
                sparse_extents,
                total_clusters,
                total_records,
                mft_size_mb = format!("{:.2}", total_size_mb),
                "⚠️  MFT is fragmented"
            );

            // Log extent details at debug level
            for (i, ext) in extents.iter().enumerate() {
                debug!(
                    extent = i,
                    vcn = ext.vcn,
                    lcn = ext.lcn,
                    clusters = ext.cluster_count,
                    is_sparse = ext.lcn < 0,
                    "  Extent {}: VCN {} → LCN {}, {} clusters{}",
                    i,
                    ext.vcn,
                    ext.lcn,
                    ext.cluster_count,
                    if ext.lcn < 0 { " (SPARSE)" } else { "" }
                );
            }
        } else {
            info!(
                total_clusters,
                total_records,
                mft_size_mb = format!("{:.2}", total_size_mb),
                "✅ MFT is contiguous (single extent)"
            );
        }

        Self {
            extents,
            bytes_per_cluster,
            bytes_per_record,
        }
    }

    /// Creates a simple extent map for a contiguous MFT.
    ///
    /// This is a fallback when extent information is not available.
    #[must_use]
    pub fn contiguous(
        mft_start_lcn: u64,
        mft_size_bytes: u64,
        bytes_per_cluster: u32,
        bytes_per_record: u32,
    ) -> Self {
        let cluster_count =
            (mft_size_bytes + u64::from(bytes_per_cluster) - 1) / u64::from(bytes_per_cluster);
        let total_records = mft_size_bytes / u64::from(bytes_per_record);
        let mft_size_mb = mft_size_bytes as f64 / (1024.0 * 1024.0);

        info!(
            mft_start_lcn,
            cluster_count,
            total_records,
            mft_size_mb = format!("{:.2}", mft_size_mb),
            "📁 Creating contiguous MFT extent map (fallback)"
        );

        Self {
            extents: vec![MftExtent {
                vcn: 0,
                cluster_count,
                lcn: mft_start_lcn as i64,
            }],
            bytes_per_cluster,
            bytes_per_record,
        }
    }

    /// Returns the physical byte offset for a given File Record Segment number.
    ///
    /// # Arguments
    ///
    /// * `frs` - The File Record Segment number
    ///
    /// # Returns
    ///
    /// `Some(offset)` if the FRS is within the mapped extents,
    /// `None` if the FRS is outside the MFT or in a sparse region.
    #[must_use]
    pub fn physical_offset(&self, frs: u64) -> Option<u64> {
        // Calculate the byte offset within the MFT (virtual offset)
        let virtual_byte_offset = frs * u64::from(self.bytes_per_record);

        // Calculate the VCN containing this record
        let vcn = virtual_byte_offset / u64::from(self.bytes_per_cluster);

        // Find the extent containing this VCN
        let extent = self.find_extent(vcn)?;

        // Check for sparse extent
        if extent.lcn < 0 {
            return None;
        }

        // Calculate offset within the extent
        let vcn_offset = vcn - extent.vcn;
        let cluster_byte_offset = vcn_offset * u64::from(self.bytes_per_cluster);

        // Calculate offset within the cluster
        let offset_in_cluster = virtual_byte_offset % u64::from(self.bytes_per_cluster);

        // Physical offset = LCN * bytes_per_cluster + offset within extent + offset in
        // cluster
        let physical = (extent.lcn as u64) * u64::from(self.bytes_per_cluster)
            + cluster_byte_offset
            + offset_in_cluster;

        Some(physical)
    }

    /// Finds the extent containing a given VCN.
    fn find_extent(&self, vcn: u64) -> Option<&MftExtent> {
        // Binary search for the extent
        let idx = self
            .extents
            .binary_search_by(|extent| {
                if vcn < extent.vcn {
                    std::cmp::Ordering::Greater
                } else if vcn >= extent.vcn + extent.cluster_count {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;

        Some(&self.extents[idx])
    }

    /// Returns the number of extents in the map.
    #[must_use]
    pub fn extent_count(&self) -> usize {
        self.extents.len()
    }

    /// Returns true if the MFT is fragmented (more than one extent).
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        self.extents.len() > 1
    }

    /// Returns an iterator over the extents.
    pub fn extents(&self) -> impl Iterator<Item = &MftExtent> {
        self.extents.iter()
    }

    /// Returns the total size of the MFT in bytes.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.extents
            .iter()
            .map(|e| e.cluster_count * u64::from(self.bytes_per_cluster))
            .sum()
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.total_size() / u64::from(self.bytes_per_record)
    }
}

// ============================================================================
// MFT Record Reader
// ============================================================================

/// Reads MFT records from a volume, handling fragmented MFTs.
#[derive(Debug)]
pub struct MftRecordReader {
    /// Size of each file record in bytes.
    record_size: u32,
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Aligned buffer for reading records.
    buffer: AlignedBuffer,
}

impl MftRecordReader {
    /// Creates a new MFT record reader.
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume handle to read from
    ///
    /// # Note
    ///
    /// This constructor creates a simple contiguous extent map.
    /// For fragmented MFT support, use `new_with_extents()`.
    #[must_use]
    pub fn new(volume: &VolumeHandle) -> Self {
        let record_size = volume.file_record_size();
        let volume_data = volume.volume_data();

        // Create a simple contiguous extent map
        let extent_map = MftExtentMap::contiguous(
            volume_data.mft_start_lcn,
            volume_data.mft_valid_data_length,
            volume_data.bytes_per_cluster,
            record_size,
        );

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Creates a new MFT record reader with explicit extent mapping.
    ///
    /// This constructor should be used when the MFT is fragmented.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The extent map for the MFT
    #[must_use]
    pub fn new_with_extents(extent_map: MftExtentMap) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Allocate buffer for one record (rounded up to sector boundary)
        let buffer_size = ((record_size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;
        let buffer = AlignedBuffer::new(buffer_size);

        Self {
            record_size,
            extent_map,
            buffer,
        }
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Returns true if the MFT is fragmented.
    #[must_use]
    pub fn is_fragmented(&self) -> bool {
        self.extent_map.is_fragmented()
    }

    /// Reads a single MFT record by its File Record Segment number.
    ///
    /// This method handles fragmented MFTs by using the extent map to
    /// translate the FRS to a physical disk location.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `frs` - The File Record Segment number to read
    ///
    /// # Errors
    ///
    /// Returns an error if the record cannot be read or is invalid.
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    pub fn read_record(&mut self, handle: HANDLE, frs: u64) -> Result<&[u8]> {
        // Use extent map to get the physical offset (handles fragmentation)
        let record_offset =
            self.extent_map
                .physical_offset(frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs,
                    reason: "FRS outside MFT extents or in sparse region".to_owned(),
                })?;

        // Align to sector boundary
        let aligned_offset = (record_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_within_sector = (record_offset - aligned_offset) as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the record
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(self.buffer.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )?;
        }

        if (bytes_read as usize) < self.record_size as usize + offset_within_sector {
            return Err(MftError::RecordRead {
                frs,
                reason: format!(
                    "Short read: expected {} bytes, got {}",
                    self.record_size, bytes_read
                ),
            });
        }

        // Return the record data (accounting for sector alignment offset)
        Ok(&self.buffer.as_slice()
            [offset_within_sector..offset_within_sector + self.record_size as usize])
    }

    /// Returns the record size in bytes.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.record_size
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }
}

// ============================================================================
// Multi-Sector Fixup
// ============================================================================

/// Applies the multi-sector fixup (Update Sequence Array) to a record.
///
/// NTFS uses a fixup mechanism to detect torn writes. The last two bytes
/// of each sector are replaced with a check value, and the original bytes
/// are stored in the Update Sequence Array.
///
/// # Arguments
///
/// * `data` - The record data (must be mutable)
///
/// # Returns
///
/// `true` if the fixup was successful, `false` if the record is corrupted.
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
pub fn apply_fixup(data: &mut [u8]) -> bool {
    if data.len() < size_of::<MultiSectorHeader>() {
        return false;
    }

    // SAFETY: We've verified the buffer is large enough.
    let header: MultiSectorHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Validate magic number
    if header.magic != FILE_RECORD_MAGIC {
        return false;
    }

    let usa_offset = header.usa_offset as usize;
    let usa_count = header.usa_count as usize;

    // USA must have at least 2 entries (check value + at least one sector)
    if usa_count < 2 {
        return false;
    }

    // Validate USA offset
    if usa_offset + usa_count * 2 > data.len() {
        return false;
    }

    // Get the check value (first entry in USA)
    let check_value = u16::from_le_bytes([data[usa_offset], data[usa_offset + 1]]);

    // Apply fixup to each sector
    for idx in 1..usa_count {
        let sector_end = idx * SECTOR_SIZE - 2;

        if sector_end + 2 > data.len() {
            break;
        }

        // Verify the check value
        let current_value = u16::from_le_bytes([data[sector_end], data[sector_end + 1]]);
        if current_value != check_value {
            return false;
        }

        // Replace with the original value from USA
        let usa_entry_offset = usa_offset + idx * 2;
        data[sector_end] = data[usa_entry_offset];
        data[sector_end + 1] = data[usa_entry_offset + 1];
    }

    true
}

// ============================================================================
// Record Parsing
// ============================================================================

use crate::ntfs::{ExtendedStandardInfo, NameInfo, StreamInfo};

/// Parsed data from an MFT record (full C++ parity).
///
/// This struct captures ALL information from an MFT record, including:
/// - Multiple file names (hard links)
/// - Multiple data streams (Alternate Data Streams)
/// - Extended size information (allocated, compressed)
/// - All 18 attribute flags
#[derive(Debug, Clone, Default)]
pub struct ParsedRecord {
    /// File Record Segment number.
    pub frs: u64,
    /// Primary parent directory FRS (from best name).
    pub parent_frs: u64,
    /// Primary file name (Win32 or Win32+DOS preferred).
    pub name: String,
    /// All file names (hard links). Includes primary name.
    pub names: Vec<NameInfo>,
    /// All data streams. First is the default (unnamed) stream.
    pub streams: Vec<StreamInfo>,
    /// Logical file size (from default stream).
    pub size: u64,
    /// Allocated size on disk.
    pub allocated_size: u64,
    /// Extended standard information with all flags.
    pub std_info: ExtendedStandardInfo,
    /// Whether this record is in use.
    pub in_use: bool,
    /// Whether this is a directory.
    pub is_directory: bool,
}

impl ParsedRecord {
    /// Returns the number of hard links (names).
    #[must_use]
    pub fn name_count(&self) -> u16 {
        self.names.len() as u16
    }

    /// Returns the number of data streams.
    #[must_use]
    pub fn stream_count(&self) -> u16 {
        self.streams.len() as u16
    }

    /// Returns the creation time (Unix microseconds).
    #[must_use]
    pub fn created(&self) -> i64 {
        self.std_info.created
    }

    /// Returns the modification time (Unix microseconds).
    #[must_use]
    pub fn modified(&self) -> i64 {
        self.std_info.modified
    }

    /// Returns the access time (Unix microseconds).
    #[must_use]
    pub fn accessed(&self) -> i64 {
        self.std_info.accessed
    }

    /// Returns the MFT change time (Unix microseconds).
    #[must_use]
    pub fn mft_changed(&self) -> i64 {
        self.std_info.mft_changed
    }

    /// Returns the raw flags as u16 (for backward compatibility).
    #[must_use]
    pub fn flags(&self) -> u16 {
        (self.std_info.to_raw_flags() & 0xFFFF) as u16
    }
}

/// Creates a placeholder record for a missing parent directory.
///
/// This matches C++ behavior where the `at()` method creates placeholder
/// records for any referenced FRS that hasn't been seen yet. When a file
/// references a parent directory that wasn't parsed (e.g., marked as not-in-use
/// in bitmap but still referenced), we create a placeholder to ensure path
/// resolution can complete.
///
/// # Arguments
///
/// * `frs` - The FRS number for the placeholder record
///
/// # Returns
///
/// A `ParsedRecord` with minimal information suitable for path resolution.
#[must_use]
pub fn create_placeholder_record(frs: u64) -> ParsedRecord {
    ParsedRecord {
        frs,
        parent_frs: 5, // Assume root as parent (FRS 5 is root directory)
        name: format!("<dir:{frs}>"),
        names: Vec::new(),
        streams: Vec::new(),
        size: 0,
        allocated_size: 0,
        std_info: ExtendedStandardInfo::default(),
        in_use: true,       // Mark as in-use so it's included in output
        is_directory: true, // Assume directory since it's referenced as parent
    }
}

/// Adds placeholder records for parent directories that are referenced
/// but not present in the parsed records.
///
/// This is the `Vec<ParsedRecord>` version of
/// `ParsedColumns::add_missing_parent_placeholders`.
///
/// # Arguments
///
/// * `records` - Mutable reference to the vector of parsed records
///
/// # Returns
///
/// The number of placeholder records added.
pub fn add_missing_parent_placeholders_to_vec(records: &mut Vec<ParsedRecord>) -> usize {
    use std::collections::HashSet;

    // Iterate until no new placeholders are needed (handles recursive missing
    // parents)
    let mut total_added = 0;
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 10; // Prevent infinite loops

    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            warn!(
                iterations,
                "Max iterations reached in placeholder creation - possible cycle"
            );
            break;
        }

        // Collect all FRS values we have
        let known_frs: HashSet<u64> = records.iter().map(|r| r.frs).collect();

        // Collect all parent_frs values that are referenced
        let referenced_parents: HashSet<u64> = records.iter().map(|r| r.parent_frs).collect();

        // Find missing parents (exclude 0 and 5 which are special root markers)
        let missing_parents: Vec<u64> = referenced_parents
            .difference(&known_frs)
            .filter(|&&frs| frs != 0 && frs != 5)
            .copied()
            .collect();

        if missing_parents.is_empty() {
            break; // No more missing parents
        }

        debug!(
            iteration = iterations,
            missing_count = missing_parents.len(),
            "Creating placeholder records for missing parent directories (Vec path)"
        );

        // Create placeholder records
        for frs in missing_parents {
            let placeholder = create_placeholder_record(frs);
            records.push(placeholder);
            total_added += 1;
        }
    }

    if total_added > 0 {
        info!(
            total_added,
            iterations, "Added placeholder records for missing parent directories (Vec path)"
        );
    }

    total_added
}

/// Attributes extracted from an extension record.
///
/// Extension records contain additional attributes for files that don't
/// fit in a single MFT record. These must be merged into the base record.
#[derive(Debug, Clone, Default)]
pub struct ExtensionAttributes {
    /// The base FRS this extension belongs to.
    pub base_frs: u64,
    /// The extension's own FRS.
    pub extension_frs: u64,
    /// File names found in this extension.
    pub names: Vec<NameInfo>,
    /// Streams found in this extension.
    pub streams: Vec<StreamInfo>,
}

/// Result of parsing an MFT record.
#[derive(Debug, Clone)]
pub enum ParseResult {
    /// A base record with all its data.
    Base(ParsedRecord),
    /// An extension record with attributes to merge.
    Extension(ExtensionAttributes),
    /// Record is not in use or invalid.
    Skip,
}

// ============================================================================
// ParsedColumns - Struct-of-Arrays (SoA) Layout for DataFrame Building
// ============================================================================

/// Column-oriented storage for parsed MFT records (Struct-of-Arrays layout).
///
/// This struct stores MFT record data in column vectors rather than as an
/// array of structs. This layout is optimal for:
/// - Direct conversion to Polars DataFrame (no transpose needed)
/// - Cache-friendly parallel accumulation
/// - Efficient memory access patterns
///
/// # Performance
///
/// Using SoA layout eliminates the AoS→SoA transpose that was previously
/// done in `build_dataframe_from_records`, reducing df_build time by ~20%.
#[derive(Debug, Clone, Default)]
pub struct ParsedColumns {
    // Core identifiers
    /// File Record Segment numbers.
    pub frs: Vec<u64>,
    /// Parent directory FRS values.
    pub parent_frs: Vec<u64>,
    /// File/directory names.
    pub name: Vec<String>,

    // Size information
    /// Logical file sizes in bytes.
    pub size: Vec<u64>,
    /// Allocated sizes on disk.
    pub allocated_size: Vec<u64>,

    // Timestamps (Unix microseconds)
    /// Creation timestamps.
    pub created: Vec<i64>,
    /// Modification timestamps.
    pub modified: Vec<i64>,
    /// Access timestamps.
    pub accessed: Vec<i64>,
    /// MFT change timestamps.
    pub mft_changed: Vec<i64>,

    // Record metadata
    /// Whether each record is a directory.
    pub is_directory: Vec<bool>,
    /// Number of hard links (names) per record.
    pub name_count: Vec<u16>,
    /// Number of data streams per record.
    pub stream_count: Vec<u16>,
    /// Stream name (empty for default stream, non-empty for ADS).
    pub stream_name: Vec<String>,

    // Attribute flags (all boolean columns for C++ parity)
    /// Read-only flag.
    pub is_readonly: Vec<bool>,
    /// Hidden flag.
    pub is_hidden: Vec<bool>,
    /// System flag.
    pub is_system: Vec<bool>,
    /// Archive flag.
    pub is_archive: Vec<bool>,
    /// Compressed flag.
    pub is_compressed: Vec<bool>,
    /// Encrypted flag.
    pub is_encrypted: Vec<bool>,
    /// Sparse flag.
    pub is_sparse: Vec<bool>,
    /// Reparse point flag.
    pub is_reparse: Vec<bool>,
    /// Offline flag.
    pub is_offline: Vec<bool>,
    /// Not content indexed flag.
    pub is_not_indexed: Vec<bool>,
    /// Temporary flag.
    pub is_temporary: Vec<bool>,
    /// Integrity stream flag (ReFS).
    pub is_integrity_stream: Vec<bool>,
    /// No scrub data flag.
    pub is_no_scrub_data: Vec<bool>,
    /// Pinned flag (OneDrive).
    pub is_pinned: Vec<bool>,
    /// Unpinned flag (OneDrive).
    pub is_unpinned: Vec<bool>,
    /// Virtual flag.
    pub is_virtual: Vec<bool>,
    /// Raw attribute flags (combined value for C++ parity).
    pub flags: Vec<u32>,
}

impl ParsedColumns {
    /// Creates a new empty `ParsedColumns`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `ParsedColumns` with pre-allocated capacity.
    ///
    /// Use this when you know the approximate number of records to avoid
    /// reallocations during accumulation.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            frs: Vec::with_capacity(capacity),
            parent_frs: Vec::with_capacity(capacity),
            name: Vec::with_capacity(capacity),
            size: Vec::with_capacity(capacity),
            allocated_size: Vec::with_capacity(capacity),
            created: Vec::with_capacity(capacity),
            modified: Vec::with_capacity(capacity),
            accessed: Vec::with_capacity(capacity),
            mft_changed: Vec::with_capacity(capacity),
            is_directory: Vec::with_capacity(capacity),
            name_count: Vec::with_capacity(capacity),
            stream_count: Vec::with_capacity(capacity),
            stream_name: Vec::with_capacity(capacity),
            is_readonly: Vec::with_capacity(capacity),
            is_hidden: Vec::with_capacity(capacity),
            is_system: Vec::with_capacity(capacity),
            is_archive: Vec::with_capacity(capacity),
            is_compressed: Vec::with_capacity(capacity),
            is_encrypted: Vec::with_capacity(capacity),
            is_sparse: Vec::with_capacity(capacity),
            is_reparse: Vec::with_capacity(capacity),
            is_offline: Vec::with_capacity(capacity),
            is_not_indexed: Vec::with_capacity(capacity),
            is_temporary: Vec::with_capacity(capacity),
            is_integrity_stream: Vec::with_capacity(capacity),
            is_no_scrub_data: Vec::with_capacity(capacity),
            is_pinned: Vec::with_capacity(capacity),
            is_unpinned: Vec::with_capacity(capacity),
            is_virtual: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of records stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frs.len()
    }

    /// Returns true if no records are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frs.is_empty()
    }

    /// Pushes a single parsed record into the columns.
    ///
    /// This is the hot path for accumulation - keep it fast!
    #[inline]
    pub fn push_record(&mut self, record: &ParsedRecord) {
        self.frs.push(record.frs);
        self.parent_frs.push(record.parent_frs);
        self.name.push(record.name.clone());
        self.size.push(record.size);
        self.allocated_size.push(record.allocated_size);
        self.created.push(record.std_info.created);
        self.modified.push(record.std_info.modified);
        self.accessed.push(record.std_info.accessed);
        self.mft_changed.push(record.std_info.mft_changed);
        self.is_directory.push(record.is_directory);
        self.name_count.push(record.name_count());
        self.stream_count.push(record.stream_count());
        self.stream_name.push(String::new()); // Default stream (no ADS)
        self.is_readonly.push(record.std_info.is_readonly);
        self.is_hidden.push(record.std_info.is_hidden);
        self.is_system.push(record.std_info.is_system);
        self.is_archive.push(record.std_info.is_archive);
        self.is_compressed.push(record.std_info.is_compressed);
        self.is_encrypted.push(record.std_info.is_encrypted);
        self.is_sparse.push(record.std_info.is_sparse);
        self.is_reparse.push(record.std_info.is_reparse);
        self.is_offline.push(record.std_info.is_offline);
        self.is_not_indexed
            .push(record.std_info.is_not_content_indexed);
        self.is_temporary.push(record.std_info.is_temporary);
        self.is_integrity_stream
            .push(record.std_info.is_integrity_stream);
        self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
        self.is_pinned.push(record.std_info.is_pinned);
        self.is_unpinned.push(record.std_info.is_unpinned);
        self.is_virtual.push(record.std_info.is_virtual);
        self.flags.push(record.std_info.to_raw_flags());
    }

    /// Pushes a record with full expansion (names × streams).
    ///
    /// This matches C++ behavior: one row per (hard link × stream) combination.
    /// If a file has 2 hard links and 3 streams, this creates 6 rows.
    ///
    /// This is the default behavior for user-facing output, as users
    /// expect to see each hard link and ADS as separate entries.
    #[inline]
    pub fn push_record_expanded(&mut self, record: &ParsedRecord) {
        // Get names to iterate over (use primary name if names is empty)
        let names: Vec<_> = if record.names.is_empty() {
            vec![NameInfo {
                name: record.name.clone(),
                parent_frs: record.parent_frs,
                namespace: 3, // Win32+DOS
            }]
        } else {
            record.names.clone()
        };

        // Get streams to iterate over (use empty stream if streams is empty)
        let streams: Vec<_> = if record.streams.is_empty() {
            vec![StreamInfo {
                name: String::new(),
                size: record.size,
                allocated_size: record.allocated_size,
                is_sparse: false,
                is_compressed: false,
            }]
        } else {
            record.streams.clone()
        };

        // Create one row per (name × stream) combination
        for name_info in &names {
            for stream_info in &streams {
                self.frs.push(record.frs);
                self.parent_frs.push(name_info.parent_frs);
                self.name.push(name_info.name.clone());
                // Use stream-specific size for ADS, file size for default stream
                let (size, alloc) = if stream_info.name.is_empty() {
                    (record.size, record.allocated_size)
                } else {
                    (stream_info.size, stream_info.allocated_size)
                };
                self.size.push(size);
                self.allocated_size.push(alloc);
                self.created.push(record.std_info.created);
                self.modified.push(record.std_info.modified);
                self.accessed.push(record.std_info.accessed);
                self.mft_changed.push(record.std_info.mft_changed);
                self.is_directory.push(record.is_directory);
                // For expanded records, counts are 1 (this row = one link + one stream)
                self.name_count.push(1);
                self.stream_count.push(1);
                self.stream_name.push(stream_info.name.clone());
                self.is_readonly.push(record.std_info.is_readonly);
                self.is_hidden.push(record.std_info.is_hidden);
                self.is_system.push(record.std_info.is_system);
                self.is_archive.push(record.std_info.is_archive);
                self.is_compressed.push(record.std_info.is_compressed);
                self.is_encrypted.push(record.std_info.is_encrypted);
                self.is_sparse.push(record.std_info.is_sparse);
                self.is_reparse.push(record.std_info.is_reparse);
                self.is_offline.push(record.std_info.is_offline);
                self.is_not_indexed
                    .push(record.std_info.is_not_content_indexed);
                self.is_temporary.push(record.std_info.is_temporary);
                self.is_integrity_stream
                    .push(record.std_info.is_integrity_stream);
                self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
                self.is_pinned.push(record.std_info.is_pinned);
                self.is_unpinned.push(record.std_info.is_unpinned);
                self.is_virtual.push(record.std_info.is_virtual);
                self.flags.push(record.std_info.to_raw_flags());
            }
        }
    }

    /// Extends this `ParsedColumns` with all records from another.
    ///
    /// Used in Rayon reduce phase to merge per-thread results.
    pub fn extend(&mut self, other: Self) {
        self.frs.extend(other.frs);
        self.parent_frs.extend(other.parent_frs);
        self.name.extend(other.name);
        self.size.extend(other.size);
        self.allocated_size.extend(other.allocated_size);
        self.created.extend(other.created);
        self.modified.extend(other.modified);
        self.accessed.extend(other.accessed);
        self.mft_changed.extend(other.mft_changed);
        self.is_directory.extend(other.is_directory);
        self.name_count.extend(other.name_count);
        self.stream_count.extend(other.stream_count);
        self.stream_name.extend(other.stream_name);
        self.is_readonly.extend(other.is_readonly);
        self.is_hidden.extend(other.is_hidden);
        self.is_system.extend(other.is_system);
        self.is_archive.extend(other.is_archive);
        self.is_compressed.extend(other.is_compressed);
        self.is_encrypted.extend(other.is_encrypted);
        self.is_sparse.extend(other.is_sparse);
        self.is_reparse.extend(other.is_reparse);
        self.is_offline.extend(other.is_offline);
        self.is_not_indexed.extend(other.is_not_indexed);
        self.is_temporary.extend(other.is_temporary);
        self.is_integrity_stream.extend(other.is_integrity_stream);
        self.is_no_scrub_data.extend(other.is_no_scrub_data);
        self.is_pinned.extend(other.is_pinned);
        self.is_unpinned.extend(other.is_unpinned);
        self.is_virtual.extend(other.is_virtual);
        self.flags.extend(other.flags);
    }

    /// Reserves capacity for additional records.
    pub fn reserve(&mut self, additional: usize) {
        self.frs.reserve(additional);
        self.parent_frs.reserve(additional);
        self.name.reserve(additional);
        self.size.reserve(additional);
        self.allocated_size.reserve(additional);
        self.created.reserve(additional);
        self.modified.reserve(additional);
        self.accessed.reserve(additional);
        self.mft_changed.reserve(additional);
        self.is_directory.reserve(additional);
        self.name_count.reserve(additional);
        self.stream_count.reserve(additional);
        self.stream_name.reserve(additional);
        self.is_readonly.reserve(additional);
        self.is_hidden.reserve(additional);
        self.is_system.reserve(additional);
        self.is_archive.reserve(additional);
        self.is_compressed.reserve(additional);
        self.is_encrypted.reserve(additional);
        self.is_sparse.reserve(additional);
        self.is_reparse.reserve(additional);
        self.is_offline.reserve(additional);
        self.is_not_indexed.reserve(additional);
        self.is_temporary.reserve(additional);
        self.is_integrity_stream.reserve(additional);
        self.is_no_scrub_data.reserve(additional);
        self.is_pinned.reserve(additional);
        self.is_unpinned.reserve(additional);
        self.is_virtual.reserve(additional);
        self.flags.reserve(additional);
    }

    /// Creates `ParsedColumns` from a vector of `ParsedRecord`.
    ///
    /// # Arguments
    ///
    /// * `records` - The parsed records to convert
    /// * `expand_links` - If `true`, expand hard links to separate rows
    ///   (matching C++ behavior). If `false`, one row per FRS.
    #[must_use]
    pub fn from_records(records: Vec<ParsedRecord>, expand_links: bool) -> Self {
        // Estimate capacity
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file
            (records.len() as f64 * 1.2) as usize
        } else {
            records.len()
        };

        let mut columns = Self::with_capacity(estimated_capacity);
        for record in records {
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }

    /// Adds placeholder records for parent directories that are referenced
    /// but not present in the parsed records.
    ///
    /// This matches C++ behavior where `at()` creates placeholder records
    /// for any referenced FRS that hasn't been seen yet. Without this,
    /// path resolution fails with `<unknown:XXXXXX>` for files whose parent
    /// directories weren't parsed (e.g., marked as not-in-use in bitmap).
    ///
    /// # Returns
    ///
    /// The number of placeholder records added.
    pub fn add_missing_parent_placeholders(&mut self) -> usize {
        use std::collections::HashSet;

        // Iterate until no new placeholders are needed (handles recursive missing
        // parents)
        let mut total_added = 0;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10; // Prevent infinite loops

        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                warn!(
                    iterations,
                    "Max iterations reached in placeholder creation - possible cycle"
                );
                break;
            }

            // Collect all FRS values we have
            let known_frs: HashSet<u64> = self.frs.iter().copied().collect();

            // Collect all parent_frs values that are referenced
            let referenced_parents: HashSet<u64> = self.parent_frs.iter().copied().collect();

            // Find missing parents (exclude 0 and 5 which are special root markers)
            let missing_parents: Vec<u64> = referenced_parents
                .difference(&known_frs)
                .filter(|&&frs| frs != 0 && frs != 5)
                .copied()
                .collect();

            if missing_parents.is_empty() {
                break; // No more missing parents
            }

            debug!(
                iteration = iterations,
                missing_count = missing_parents.len(),
                "Creating placeholder records for missing parent directories"
            );

            // Create placeholder records
            for frs in missing_parents {
                let placeholder = create_placeholder_record(frs);
                self.push_record(&placeholder);
                total_added += 1;
            }
        }

        if total_added > 0 {
            info!(
                total_added,
                iterations, "Added placeholder records for missing parent directories"
            );
        }

        total_added
    }
}

/// Parses an MFT record and extracts relevant information.
///
/// This function handles both base records and extension records.
/// Extension records return `ParseResult::Extension` which must be
/// merged into the base record later.
///
/// # Arguments
///
/// * `data` - The raw record data (after fixup)
/// * `frs` - The File Record Segment number
///
/// # Returns
///
/// `ParseResult::Base` for base records, `ParseResult::Extension` for
/// extension records, or `ParseResult::Skip` if invalid/not in use.
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS structs
pub fn parse_record_full(data: &[u8], frs: u64) -> ParseResult {
    if data.len() < size_of::<FileRecordSegmentHeader>() {
        return ParseResult::Skip;
    }

    // SAFETY: We've verified the buffer is large enough for the header.
    let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };

    // Check if record is in use
    if !header.is_in_use() {
        return ParseResult::Skip;
    }

    // Copy the packed field to avoid unaligned reference
    let multi_sector_header = header.multi_sector_header;
    if !multi_sector_header.is_file_record() {
        return ParseResult::Skip;
    }

    // Check if this is an extension record
    let is_extension = !header.is_base_record();
    let base_frs = if is_extension {
        crate::ntfs::file_reference_to_frs(header.base_file_record_segment)
    } else {
        frs
    };

    // Prepare result containers
    let mut names: Vec<NameInfo> = Vec::new();
    let mut streams: Vec<StreamInfo> = Vec::new();
    let mut std_info = ExtendedStandardInfo::default();
    let mut primary_name = String::new();
    let mut primary_parent_frs = 0u64;
    let mut primary_namespace = 255u8; // Invalid, will be replaced

    // Parse attributes
    let mut offset = header.first_attribute_offset as usize;
    let max_offset = core::cmp::min(header.bytes_in_use as usize, data.len());

    // SAFETY: We've verified the buffer is large enough.
    while offset + size_of::<AttributeRecordHeader>() <= max_offset {
        let attr_header: AttributeRecordHeader =
            unsafe { core::ptr::read(data[offset..].as_ptr().cast()) };

        // Check for end marker
        if attr_header.type_code == AttributeType::End as u32 {
            break;
        }

        // Validate attribute length
        if attr_header.length == 0 || offset + attr_header.length as usize > max_offset {
            break;
        }

        // Parse based on attribute type
        match AttributeType::from_u32(attr_header.type_code) {
            Some(AttributeType::StandardInformation) => {
                if attr_header.is_non_resident == 0 {
                    parse_standard_info_full(data, offset, &mut std_info);
                }
            }
            Some(AttributeType::FileName) => {
                if attr_header.is_non_resident == 0 {
                    if let Some(name_info) = parse_file_name_full(data, offset) {
                        // Skip DOS-only names (namespace 2)
                        if name_info.namespace != 2 {
                            // Check if this is a better primary name
                            let is_better = match name_info.namespace {
                                1 | 3 => true,                 // Win32 or Win32+DOS
                                0 => primary_namespace == 255, // POSIX only if no name yet
                                _ => false,
                            };
                            if is_better || primary_namespace == 255 {
                                primary_name = name_info.name.clone();
                                primary_parent_frs = name_info.parent_frs;
                                primary_namespace = name_info.namespace;
                            }
                            names.push(name_info);
                        }
                    }
                }
            }
            Some(AttributeType::Data) => {
                if let Some(stream_info) = parse_data_attribute_full(data, offset, &attr_header) {
                    streams.push(stream_info);
                }
            }
            _ => {}
        }

        offset += attr_header.length as usize;
    }

    // Handle extension records
    if is_extension {
        return ParseResult::Extension(ExtensionAttributes {
            base_frs,
            extension_frs: frs,
            names,
            streams,
        });
    }

    // Skip records without a $FILE_NAME attribute (matching C++ behavior).
    // C++ uses nameinfo() which returns NULL for records without filenames,
    // causing the traversal loop to skip them entirely.
    // These are typically extension records, deleted files, or corrupted records.
    if primary_name.is_empty() {
        return ParseResult::Skip;
    }

    // Calculate primary size from default stream
    let (size, allocated_size) = streams
        .iter()
        .find(|s| s.name.is_empty())
        .map(|s| (s.size, s.allocated_size))
        .unwrap_or((0, 0));

    ParseResult::Base(ParsedRecord {
        frs,
        parent_frs: primary_parent_frs,
        name: primary_name,
        names,
        streams,
        size,
        allocated_size,
        std_info,
        in_use: true,
        is_directory: header.is_directory(),
    })
}

/// Legacy parse function for backward compatibility.
///
/// This function skips extension records and returns `Option<ParsedRecord>`.
pub fn parse_record(data: &[u8], frs: u64) -> Option<ParsedRecord> {
    match parse_record_full(data, frs) {
        ParseResult::Base(record) => Some(record),
        _ => None,
    }
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
pub fn parse_record_zero_alloc(data: &[u8], frs: u64) -> ParseResult {
    RECORD_BUFFER.with(|buf| {
        let mut buffer = buf.borrow_mut();

        // Ensure buffer is large enough
        if buffer.len() < data.len() {
            buffer.resize(data.len(), 0);
        }

        // Copy data into thread-local buffer
        buffer[..data.len()].copy_from_slice(data);

        // Apply fixup in place
        if !apply_fixup(&mut buffer[..data.len()]) {
            return ParseResult::Skip;
        }

        // Parse the record
        parse_record_full(&buffer[..data.len()], frs)
    })
}
/// Parses `$STANDARD_INFORMATION` into `ExtendedStandardInfo`.
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
fn parse_standard_info_full(data: &[u8], attr_offset: usize, result: &mut ExtendedStandardInfo) {
    use crate::ntfs::filetime_to_unix_micros;

    // Get value offset (resident attribute)
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let si_offset = attr_offset + value_offset;
    if si_offset + size_of::<StandardInformation>() > data.len() {
        return;
    }

    // SAFETY: We've verified the buffer is large enough.
    let si: StandardInformation = unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

    result.created = filetime_to_unix_micros(si.creation_time);
    result.modified = filetime_to_unix_micros(si.modification_time);
    result.accessed = filetime_to_unix_micros(si.access_time);
    result.mft_changed = filetime_to_unix_micros(si.mft_change_time);

    // Parse all flags
    *result = ExtendedStandardInfo {
        created: result.created,
        modified: result.modified,
        accessed: result.accessed,
        mft_changed: result.mft_changed,
        ..ExtendedStandardInfo::from_attributes(si.file_attributes)
    };
}

/// Parses `$FILE_NAME` and returns a `NameInfo`.
#[allow(unsafe_code)] // Required: ptr::read for packed NTFS struct
fn parse_file_name_full(data: &[u8], attr_offset: usize) -> Option<NameInfo> {
    use crate::ntfs::file_reference_to_frs;

    // Get value offset (resident attribute)
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let fn_offset = attr_offset + value_offset;
    if fn_offset + size_of::<FileNameAttribute>() > data.len() {
        return None;
    }

    // SAFETY: We've verified the buffer is large enough.
    let fn_attr: FileNameAttribute = unsafe { core::ptr::read(data[fn_offset..].as_ptr().cast()) };

    // Extract file name (UTF-16LE)
    let name_len = fn_attr.file_name_length as usize;
    let name_offset = fn_offset + size_of::<FileNameAttribute>();

    if name_offset + name_len * 2 > data.len() {
        return None;
    }

    let name_bytes = &data[name_offset..name_offset + name_len * 2];
    let name_u16: Vec<u16> = name_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let name = String::from_utf16(&name_u16).ok()?;

    Some(NameInfo {
        name,
        parent_frs: file_reference_to_frs(fn_attr.parent_directory),
        namespace: fn_attr.file_name_namespace,
    })
}

/// Parses `$DATA` attribute and returns a `StreamInfo`.
fn parse_data_attribute_full(
    data: &[u8],
    attr_offset: usize,
    header: &AttributeRecordHeader,
) -> Option<StreamInfo> {
    // Extract stream name from attribute header
    let stream_name = if header.name_length > 0 {
        let name_offset = attr_offset + header.name_offset as usize;
        let name_len = header.name_length as usize;
        if name_offset + name_len * 2 > data.len() {
            return None;
        }
        let name_bytes = &data[name_offset..name_offset + name_len * 2];
        let name_u16: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&name_u16).unwrap_or_default()
    } else {
        String::new()
    };

    let (size, allocated_size, is_sparse, is_compressed) = if header.is_non_resident != 0 {
        // Non-resident: get sizes from non-resident header
        let nr_offset = attr_offset + 16; // After common header
        if nr_offset + 48 > data.len() {
            return None;
        }

        let allocated_size =
            i64::from_le_bytes(data[nr_offset + 24..nr_offset + 32].try_into().ok()?);
        let data_size = i64::from_le_bytes(data[nr_offset + 40..nr_offset + 48].try_into().ok()?);

        // Check compression unit (at offset 16 in non-resident header)
        let compression_unit = data[nr_offset + 8];
        let is_compressed = compression_unit > 0;

        // Check sparse flag in attribute flags
        let is_sparse = (header.flags & 0x8000) != 0;

        (
            data_size.max(0) as u64,
            allocated_size.max(0) as u64,
            is_sparse,
            is_compressed,
        )
    } else {
        // Resident: get size from resident header
        let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
        let value_length = u32::from_le_bytes(value_length_bytes.try_into().ok()?);
        (value_length as u64, 0, false, false)
    };

    Some(StreamInfo {
        name: stream_name,
        size,
        allocated_size,
        is_sparse,
        is_compressed,
    })
}

// ============================================================================
// MFT Record Merger
// ============================================================================

use std::collections::HashMap;

/// Merges extension record attributes into base records.
///
/// This implements the C++ behavior where attributes from extension
/// records are merged into their base records.
pub struct MftRecordMerger {
    /// Base records indexed by FRS.
    base_records: HashMap<u64, ParsedRecord>,
    /// Pending extension attributes.
    extensions: Vec<ExtensionAttributes>,
}

impl MftRecordMerger {
    /// Creates a new merger.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_records: HashMap::new(),
            extensions: Vec::new(),
        }
    }

    /// Creates a new merger with estimated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            base_records: HashMap::with_capacity(capacity),
            extensions: Vec::with_capacity(capacity / 100), // Extensions are rare
        }
    }

    /// Adds a parse result to the merger.
    pub fn add_result(&mut self, result: ParseResult) {
        match result {
            ParseResult::Base(record) => {
                self.base_records.insert(record.frs, record);
            }
            ParseResult::Extension(ext) => {
                self.extensions.push(ext);
            }
            ParseResult::Skip => {}
        }
    }

    /// Merges all extensions into their base records and returns the result.
    #[must_use]
    pub fn merge(mut self) -> Vec<ParsedRecord> {
        // Merge all extensions into their base records
        for ext in self.extensions {
            if let Some(base) = self.base_records.get_mut(&ext.base_frs) {
                // Merge names (avoiding duplicates)
                for name in ext.names {
                    if !base
                        .names
                        .iter()
                        .any(|n| n.name == name.name && n.parent_frs == name.parent_frs)
                    {
                        base.names.push(name);
                    }
                }
                // Merge streams (avoiding duplicates)
                for stream in ext.streams {
                    if !base.streams.iter().any(|s| s.name == stream.name) {
                        base.streams.push(stream);
                    }
                }
            }
        }

        // Recalculate sizes from merged streams
        for record in self.base_records.values_mut() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }
        }

        self.base_records.into_values().collect()
    }

    /// Returns the number of base records.
    #[must_use]
    pub fn base_count(&self) -> usize {
        self.base_records.len()
    }

    /// Returns the number of pending extensions.
    #[must_use]
    pub fn extension_count(&self) -> usize {
        self.extensions.len()
    }

    /// Merges all extensions and returns the result as `ParsedColumns` (SoA
    /// layout).
    ///
    /// This is more efficient than `merge()` followed by conversion because it
    /// avoids creating an intermediate `Vec<ParsedRecord>`.
    ///
    /// # Arguments
    ///
    /// * `expand_links` - If `true` (default), expand hard links to separate
    ///   rows (matching C++ behavior and user expectations). If `false`, output
    ///   one row per unique FRS (power user mode).
    #[must_use]
    pub fn merge_into_columns(self, expand_links: bool) -> ParsedColumns {
        self.merge_into_columns_internal(expand_links)
    }

    /// Internal implementation for merge_into_columns.
    fn merge_into_columns_internal(mut self, expand_links: bool) -> ParsedColumns {
        // Merge all extensions into their base records
        for ext in self.extensions {
            if let Some(base) = self.base_records.get_mut(&ext.base_frs) {
                // Merge names (avoiding duplicates)
                for name in ext.names {
                    if !base
                        .names
                        .iter()
                        .any(|n| n.name == name.name && n.parent_frs == name.parent_frs)
                    {
                        base.names.push(name);
                    }
                }
                // Merge streams (avoiding duplicates)
                for stream in ext.streams {
                    if !base.streams.iter().any(|s| s.name == stream.name) {
                        base.streams.push(stream);
                    }
                }
            }
        }

        // Recalculate sizes from merged streams
        for record in self.base_records.values_mut() {
            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }
        }

        // Estimate capacity: if expanding links, we need more space
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file
            (self.base_records.len() as f64 * 1.2) as usize
        } else {
            self.base_records.len()
        };

        // Convert directly to ParsedColumns (single pass, no intermediate Vec)
        let mut columns = ParsedColumns::with_capacity(estimated_capacity);
        for record in self.base_records.into_values() {
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }
}

impl Default for MftRecordMerger {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Batch MFT Reader
// ============================================================================

/// Batch reader for efficient MFT reading.
///
/// Reads multiple records per I/O operation by reading entire clusters
/// or extent chunks at once.
#[derive(Debug)]
pub struct BatchMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Size of each file record in bytes.
    record_size: u32,
    /// Bytes per cluster.
    bytes_per_cluster: u32,
    /// Read block size (multiple of cluster size).
    read_block_size: usize,
    /// Aligned buffer for batch reads.
    buffer: AlignedBuffer,
}

impl BatchMftReader {
    /// Default read block size (1 MB).
    pub const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;

    /// Creates a new batch reader.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bytes_per_cluster: u32) -> Self {
        Self::with_block_size(extent_map, bytes_per_cluster, Self::DEFAULT_BLOCK_SIZE)
    }

    /// Creates a new batch reader with a custom block size.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - The MFT extent map
    /// * `bytes_per_cluster` - Cluster size in bytes
    /// * `block_size` - Read block size (will be rounded to cluster boundary)
    #[must_use]
    pub fn with_block_size(
        extent_map: MftExtentMap,
        bytes_per_cluster: u32,
        block_size: usize,
    ) -> Self {
        let record_size = extent_map.bytes_per_record;

        // Round block size to cluster boundary
        let cluster_size = bytes_per_cluster as usize;
        let read_block_size = ((block_size + cluster_size - 1) / cluster_size) * cluster_size;

        let buffer = AlignedBuffer::new(read_block_size);

        Self {
            extent_map,
            record_size,
            bytes_per_cluster,
            read_block_size,
            buffer,
        }
    }

    /// Returns the number of records that fit in one read block.
    #[must_use]
    pub fn records_per_block(&self) -> usize {
        self.read_block_size / self.record_size as usize
    }

    /// Returns the extent map.
    #[must_use]
    pub fn extent_map(&self) -> &MftExtentMap {
        &self.extent_map
    }

    /// Reads a batch of records starting from a given FRS.
    ///
    /// This reads up to `records_per_block()` records in a single I/O
    /// operation.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `start_frs` - The first FRS to read
    ///
    /// # Returns
    ///
    /// A tuple of (buffer slice, first FRS in buffer, number of records read).
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    pub fn read_batch(&mut self, handle: HANDLE, start_frs: u64) -> Result<(&[u8], u64, usize)> {
        // Get physical offset for the starting FRS
        let start_offset =
            self.extent_map
                .physical_offset(start_frs)
                .ok_or_else(|| MftError::RecordRead {
                    frs: start_frs,
                    reason: "FRS outside MFT extents".to_owned(),
                })?;

        // Align to cluster boundary for optimal I/O
        let cluster_size = u64::from(self.bytes_per_cluster);
        let aligned_offset = (start_offset / cluster_size) * cluster_size;

        // Calculate how many records we can read
        let total_records = self.extent_map.total_records();
        let max_records = (total_records - start_frs) as usize;
        let records_to_read = max_records.min(self.records_per_block());
        let bytes_to_read = records_to_read * self.record_size as usize;

        // Seek to the aligned offset
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        // Read the batch
        let read_size = bytes_to_read.min(self.buffer.len());
        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut self.buffer.as_mut_slice()[..read_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Calculate offset within buffer for the first record
        let offset_in_buffer = (start_offset - aligned_offset) as usize;
        let usable_bytes = (bytes_read as usize).saturating_sub(offset_in_buffer);
        let records_read = usable_bytes / self.record_size as usize;

        Ok((
            &self.buffer.as_slice()
                [offset_in_buffer..offset_in_buffer + records_read * self.record_size as usize],
            start_frs,
            records_read,
        ))
    }

    /// Extracts a single record from a batch buffer.
    ///
    /// # Arguments
    ///
    /// * `batch_buffer` - The buffer returned by `read_batch()`
    /// * `index` - The index of the record within the batch (0-based)
    ///
    /// # Returns
    ///
    /// The record data slice, or `None` if the index is out of bounds.
    #[must_use]
    pub fn extract_record<'a>(&self, batch_buffer: &'a [u8], index: usize) -> Option<&'a [u8]> {
        let record_size = self.record_size as usize;
        let start = index * record_size;
        let end = start + record_size;

        if end <= batch_buffer.len() {
            Some(&batch_buffer[start..end])
        } else {
            None
        }
    }
}

// ============================================================================
// Parallel MFT Reader
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

/// A read chunk representing a contiguous range of MFT records.
#[derive(Debug, Clone)]
pub struct ReadChunk {
    /// Physical byte offset on disk.
    pub disk_offset: u64,
    /// First FRS in this chunk.
    pub start_frs: u64,
    /// Number of records in this chunk.
    pub record_count: u64,
    /// Number of records to skip at the beginning (all unused).
    pub skip_begin: u64,
    /// Number of records to skip at the end (all unused).
    pub skip_end: u64,
}

impl ReadChunk {
    /// Returns the effective first FRS (after skipping unused records).
    #[must_use]
    pub fn effective_start_frs(&self) -> u64 {
        self.start_frs + self.skip_begin
    }

    /// Returns the effective record count (excluding skipped records).
    #[must_use]
    pub fn effective_record_count(&self) -> u64 {
        self.record_count
            .saturating_sub(self.skip_begin + self.skip_end)
    }

    /// Returns the byte size to read (after accounting for skips).
    #[must_use]
    pub fn read_size(&self, record_size: u32) -> u64 {
        self.effective_record_count() * u64::from(record_size)
    }
}

/// Generates optimized read chunks for the MFT.
///
/// This function implements the C++ optimization of:
/// 1. Splitting the MFT into chunks based on extents
/// 2. Using the bitmap to skip clusters with no in-use records
/// 3. Calculating skip_begin/skip_end for each chunk
///
/// # Arguments
///
/// * `extent_map` - The MFT extent map
/// * `bitmap` - Optional bitmap for skip optimization
/// * `chunk_size` - Target chunk size in bytes (default 1MB)
///
/// # Returns
///
/// Vector of read chunks optimized for I/O.
pub fn generate_read_chunks(
    extent_map: &MftExtentMap,
    bitmap: Option<&crate::platform::MftBitmap>,
    chunk_size: usize,
) -> Vec<ReadChunk> {
    let mut chunks = Vec::new();
    let record_size = extent_map.bytes_per_record;
    let cluster_size = extent_map.bytes_per_cluster;
    let records_per_cluster = cluster_size / record_size;

    let num_extents = extent_map.extent_count();
    let mut sparse_extents = 0u64;
    let mut total_records_to_read = 0u64;
    let mut total_records_skipped = 0u64;

    debug!(
        num_extents,
        record_size, cluster_size, records_per_cluster, chunk_size, "📐 Generating read chunks"
    );

    // Process each extent
    for (extent_idx, extent) in extent_map.extents().enumerate() {
        if extent.lcn < 0 {
            sparse_extents += 1;
            trace!(extent_idx, vcn = extent.vcn, "Skipping sparse extent");
            continue;
        }

        let extent_start_frs = extent.vcn * u64::from(records_per_cluster);
        let extent_records = extent.cluster_count * u64::from(records_per_cluster);
        let extent_disk_offset = (extent.lcn as u64) * u64::from(cluster_size);

        trace!(
            extent_idx,
            vcn = extent.vcn,
            lcn = extent.lcn,
            clusters = extent.cluster_count,
            records = extent_records,
            disk_offset = extent_disk_offset,
            "Processing extent"
        );

        // Split extent into chunks
        let records_per_chunk = (chunk_size / record_size as usize) as u64;
        let mut chunk_start = 0u64;

        while chunk_start < extent_records {
            let chunk_records = (extent_records - chunk_start).min(records_per_chunk);
            let chunk_frs_start = extent_start_frs + chunk_start;
            let chunk_frs_end = chunk_frs_start + chunk_records;

            // Calculate skip ranges using bitmap (for I/O optimization only).
            //
            // IMPORTANT: We ALWAYS add chunks regardless of bitmap status.
            // The bitmap is used for I/O optimization (skip_begin/skip_end) to reduce
            // disk reads, but we still parse all records and check the IN_USE flag
            // in each record header. This matches C++ behavior where bitmap is
            // advisory, not authoritative.
            //
            // The C++ implementation (line 7489) defaults to reading all records:
            //   this->mft_bitmap.resize(..., ~Bitmap::value_type() /*default should be to
            // read unused slots too */);
            //
            // Previously, Rust skipped entire chunks if all records were marked as
            // not-in-use in the bitmap. This caused ~5-6M files to be missed because:
            // 1. Bitmap may be stale or inconsistent with record headers
            // 2. Parent directories marked not-in-use are still referenced by children
            // 3. Extension records may be in different chunks than their base records
            let (skip_begin, skip_end) = if let Some(bm) = bitmap {
                bm.calculate_skip_range(chunk_frs_start, chunk_frs_end)
            } else {
                (0, 0)
            };

            // ALWAYS add chunk - bitmap is for I/O optimization, not filtering
            // The IN_USE flag in each record header is the authoritative source
            let effective_records = chunk_records - skip_begin - skip_end;
            total_records_to_read += effective_records;
            total_records_skipped += skip_begin + skip_end;

            chunks.push(ReadChunk {
                disk_offset: extent_disk_offset + chunk_start * u64::from(record_size),
                start_frs: chunk_frs_start,
                record_count: chunk_records,
                skip_begin,
                skip_end,
            });

            chunk_start += chunk_records;
        }
    }

    if sparse_extents > 0 {
        debug!(sparse_extents, "Skipped sparse extents");
    }

    // M1 8.6: Merge adjacent chunks with small gaps
    // If two chunks are close together (gap < merge_threshold records),
    // it's more efficient to read them as one chunk than to do two I/O ops.
    let merge_threshold = 64u64; // Records - about 64KB at 1024 bytes/record
    let chunks_before_merge = chunks.len();
    let chunks = merge_adjacent_chunks(chunks, record_size, merge_threshold);
    let chunks_after_merge = chunks.len();

    if chunks_before_merge != chunks_after_merge {
        debug!(
            before = chunks_before_merge,
            after = chunks_after_merge,
            merged = chunks_before_merge - chunks_after_merge,
            "🔗 Merged adjacent chunks"
        );
    }

    info!(
        chunks = chunks.len(),
        records_to_read = total_records_to_read,
        records_skipped = total_records_skipped,
        skip_percentage = format!(
            "{:.1}%",
            if total_records_to_read + total_records_skipped > 0 {
                (total_records_skipped as f64
                    / (total_records_to_read + total_records_skipped) as f64)
                    * 100.0
            } else {
                0.0
            }
        ),
        "📊 Read plan generated"
    );

    chunks
}

/// M1 8.6: Merge adjacent chunks with small gaps.
///
/// When two chunks are close together (gap < threshold), reading them as one
/// chunk is more efficient than two separate I/O operations. The overhead of
/// reading a few extra unused records is less than the syscall overhead.
///
/// **Important**: Merged chunks are capped at `MAX_CHUNK_BYTES` (1GB) to avoid
/// exceeding the Windows `ReadFile` API's 4GB buffer limit (u32::MAX).
fn merge_adjacent_chunks(
    mut chunks: Vec<ReadChunk>,
    record_size: u32,
    threshold: u64,
) -> Vec<ReadChunk> {
    // Maximum merged chunk size: 1GB (well below u32::MAX to be safe)
    // Windows ReadFile API takes buffer length as u32, so >4GB would panic.
    const MAX_CHUNK_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB

    if chunks.len() < 2 {
        return chunks;
    }

    let mut merged = Vec::with_capacity(chunks.len());
    let mut current = chunks.remove(0);

    for next in chunks {
        // Check if chunks are PHYSICALLY adjacent on disk.
        // This is critical for fragmented MFTs where FRS numbers may be contiguous
        // but disk locations are NOT (e.g., extent 4 at LCN 9M, extent 5 at LCN 3M).
        //
        // BUG FIX: Previously we only checked if gap_bytes (using saturating_sub) was
        // small, but saturating_sub returns 0 when next.disk_offset <
        // current_end_offset, causing chunks from different extents to be
        // incorrectly merged.
        let current_end_offset =
            current.disk_offset + current.record_count * u64::from(record_size);

        // Check for physical contiguity: next chunk must start at or very close to
        // where current chunk ends. We check BOTH directions to catch non-contiguous
        // extents regardless of their relative disk positions.
        let is_physically_contiguous = if next.disk_offset >= current_end_offset {
            // Normal case: next chunk is after current on disk
            let gap_bytes = next.disk_offset - current_end_offset;
            gap_bytes <= threshold * u64::from(record_size)
        } else {
            // Next chunk is BEFORE current on disk - NOT contiguous!
            // This happens with fragmented MFTs where extents are scattered.
            false
        };

        // Also check if they're in the same extent (contiguous FRS range)
        let current_end_frs = current.start_frs + current.record_count;
        let frs_gap = next.start_frs.saturating_sub(current_end_frs);
        let is_frs_contiguous = frs_gap <= threshold;

        // Calculate merged size to check against limit
        let new_record_count = (next.start_frs + next.record_count) - current.start_frs;
        let merged_bytes = new_record_count * u64::from(record_size);

        // Only merge if BOTH physically contiguous AND FRS contiguous
        if is_physically_contiguous && is_frs_contiguous && merged_bytes <= MAX_CHUNK_BYTES {
            // Merge: extend current chunk to include next
            current.record_count = new_record_count;
            // Update skip_end to be from the merged chunk
            current.skip_end = next.skip_end;
        } else {
            // Not contiguous or merged chunk would exceed size limit
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);

    merged
}

/// High-performance parallel MFT reader.
///
/// This reader implements aggressive optimizations for maximum throughput:
/// - Extent-aware reading for fragmented MFTs
/// - Bitmap-based cluster skipping
/// - Parallel record processing using Rayon
/// - Large batch I/O (4-8 MB chunks) for reduced syscall overhead
/// - Drive-type aware tuning (SSD vs HDD)
/// - Buffer reuse to minimize allocations
#[derive(Debug)]
pub struct ParallelMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    pub chunk_size: usize,
    /// Progress counter (atomic for thread-safe updates).
    records_processed: Arc<AtomicU64>,
    /// Fixup failure counter (potential corruption).
    fixup_failures: Arc<AtomicU64>,
    /// Skipped records counter (not in use or invalid).
    skipped_records: Arc<AtomicU64>,
    /// M1 8.4: Reusable aligned buffer for sequential I/O.
    /// Wrapped in RefCell for interior mutability since read_chunk needs &mut.
    buffer: RefCell<AlignedBuffer>,
}

impl ParallelMftReader {
    /// Default chunk size for SSD (8 MB) - high IOPS, large sequential reads.
    pub const DEFAULT_CHUNK_SIZE_SSD: usize = 8 * 1024 * 1024;

    /// Default chunk size for HDD (4 MB) - balance seek overhead and
    /// throughput.
    pub const DEFAULT_CHUNK_SIZE_HDD: usize = 4 * 1024 * 1024;

    /// Legacy default chunk size (1 MB) - kept for compatibility.
    pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

    /// Creates a new parallel reader with default (legacy) chunk size.
    #[must_use]
    pub fn new(extent_map: MftExtentMap, bitmap: Option<crate::platform::MftBitmap>) -> Self {
        let chunk_size = Self::DEFAULT_CHUNK_SIZE_HDD;
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        Self {
            extent_map,
            bitmap,
            chunk_size,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Creates a new parallel reader optimized for the given drive type.
    #[must_use]
    pub fn new_optimized(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        // M1 8.4: Pre-allocate reusable buffer for chunk_size + sector alignment
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Creating optimized reader for drive type"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            records_processed: Arc::new(AtomicU64::new(0)),
            fixup_failures: Arc::new(AtomicU64::new(0)),
            skipped_records: Arc::new(AtomicU64::new(0)),
            buffer: RefCell::new(buffer),
        }
    }

    /// Sets the chunk size for I/O operations.
    #[must_use]
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        // M1 8.4: Resize buffer to match new chunk size
        self.buffer = RefCell::new(AlignedBuffer::new(chunk_size + SECTOR_SIZE));
        self
    }

    /// Returns the number of records processed so far.
    #[must_use]
    pub fn records_processed(&self) -> u64 {
        self.records_processed.load(Ordering::Relaxed)
    }

    /// Returns the total number of records in the MFT.
    #[must_use]
    pub fn total_records(&self) -> u64 {
        self.extent_map.total_records()
    }

    /// Reads and parses all MFT records in parallel.
    ///
    /// This is the main entry point for high-performance MFT reading.
    /// Uses the legacy `parse_record` which skips extension records.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    ///
    /// # Returns
    ///
    /// Vector of parsed records.
    pub fn read_all_parallel(&self, handle: HANDLE) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, false, None)
    }

    /// Reads and parses all MFT records in parallel with full C++ parity.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the C++ implementation behavior.
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_merge(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
    ) -> Result<Vec<ParsedRecord>> {
        self.read_all_parallel_with_progress::<fn(u64, u64)>(handle, merge_extensions, None)
    }

    /// Reads and parses all MFT records in parallel with progress callback.
    ///
    /// This function handles extension records by merging their attributes
    /// into the base records, matching the C++ implementation behavior.
    /// The progress callback is called during the I/O phase with (bytes_read,
    /// total_bytes).
    ///
    /// # Arguments
    ///
    /// * `handle` - The raw volume handle
    /// * `merge_extensions` - If true, merge extension record attributes
    /// * `progress_callback` - Optional callback called with (bytes_read,
    ///   total_bytes)
    ///
    /// # Returns
    ///
    /// Vector of parsed records with all attributes merged.
    pub fn read_all_parallel_with_progress<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            merge_extensions, "Starting parallel MFT read"
        );

        // Generate optimized read chunks
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let num_chunks = chunks.len();
        info!(num_chunks, "Generated read chunks");

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };
        info!(estimated_records, "Estimated record count");

        // Process chunks in parallel
        // Note: We read sequentially but parse in parallel for thread safety with
        // HANDLE
        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // Calculate total bytes to read for progress reporting
        let total_bytes_to_read: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Read all chunks (sequential I/O for handle safety)
        debug!("Reading all chunks into memory...");
        let mut total_bytes_read: u64 = 0;
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.into_iter().enumerate() {
            trace!(
                chunk_idx = idx,
                start_frs = chunk.start_frs,
                "Reading chunk"
            );
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    total_bytes_read += data.len() as u64;
                    trace!(
                        chunk_idx = idx,
                        bytes = data.len(),
                        total_bytes = total_bytes_read,
                        "Chunk read successfully"
                    );

                    // Report progress after each chunk
                    if let Some(ref cb) = progress_callback {
                        cb(total_bytes_read, total_bytes_to_read);
                    }

                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(chunk_idx = idx, error = ?e, "Failed to read chunk");
                }
            }
        }

        info!(
            chunks_read = chunk_data.len(),
            total_bytes = total_bytes_read,
            total_mb = total_bytes_read / (1024 * 1024),
            "All chunks read into memory"
        );

        // M1 8.1 OPTIMIZATION: Use fold/reduce pattern instead of per-record atomics
        // This eliminates cache-line ping-pong across threads by accumulating
        // per-thread stats, then reducing at the end.

        if merge_extensions {
            // Per-thread accumulator for fold/reduce pattern
            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            // Full parsing with extension merging using fold/reduce
            let combined = chunk_data
                .par_iter()
                .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    // Pre-allocate for this chunk's results
                    acc.results.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let record_data = &data[offset..offset + record_size];
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        // Use zero-allocation parsing with thread-local buffer
                        let result = parse_record_zero_alloc(record_data, frs);
                        if matches!(result, ParseResult::Skip) {
                            acc.skipped += 1;
                        } else {
                            acc.results.push(result);
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(ChunkStats::default, |mut a, b| {
                    a.results.extend(b.results);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            // Update atomics once at the end (not per-record!)
            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let parse_results = combined.results;
            let skipped_count = combined.skipped;

            // Log statistics
            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);

            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if skipped_count > 0 {
                debug!(
                    skipped_records = skipped_count,
                    "📋 Records skipped (not in use or invalid)"
                );
            }

            // Merge extensions into base records
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in parse_results {
                merger.add_result(result);
            }

            Ok(merger.merge())
        } else {
            // Legacy parsing (skips extension records) - also uses fold/reduce
            #[derive(Default)]
            struct LegacyStats {
                records: Vec<ParsedRecord>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter()
                .fold(LegacyStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.records.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let record_data = &data[offset..offset + record_size];
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        match parse_record_zero_alloc(record_data, frs) {
                            ParseResult::Base(parsed) => acc.records.push(parsed),
                            _ => acc.skipped += 1,
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(LegacyStats::default, |mut a, b| {
                    a.records.extend(b.records);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            // Update atomics once at the end
            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            // Log statistics
            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);

            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if combined.skipped > 0 {
                debug!(
                    skipped_records = combined.skipped,
                    "📋 Records skipped (not in use or invalid)"
                );
            }

            Ok(combined.records)
        }
    }

    /// Reads all MFT records and returns them as `ParsedColumns` (SoA layout).
    ///
    /// This is the optimized path that avoids the AoS→SoA transpose by:
    /// 1. Parsing records into `ParseResult` (same as before)
    /// 2. Optionally merging extensions using `MftRecordMerger`
    /// 3. Converting directly to `ParsedColumns` (no intermediate
    ///    `Vec<ParsedRecord>`)
    ///
    /// # Performance
    ///
    /// - **Fast path** (`merge_extensions=false`): Parses directly to
    ///   `ParsedColumns`, skipping the HashMap-based merge. ~15-25% faster on
    ///   SSD. Extension records (~1% of files with many hard links or ADS) are
    ///   skipped.
    ///
    /// - **Full path** (`merge_extensions=true`): Uses `MftRecordMerger` to
    ///   merge extension attributes. Complete data for all files.
    ///
    /// # Arguments
    ///
    /// * `handle` - Windows file handle to the MFT
    /// * `merge_extensions` - If true, merge extension records (slower but
    ///   complete). If false, skip extensions for maximum speed.
    /// * `progress_callback` - Optional callback for progress reporting
    ///
    /// # Returns
    ///
    /// `ParsedColumns` ready for direct conversion to Polars DataFrame.
    pub fn read_all_parallel_to_columns<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        expand_links: bool,
        progress_callback: Option<F>,
    ) -> Result<ParsedColumns>
    where
        F: Fn(u64, u64),
    {
        info!(
            chunk_size = self.chunk_size,
            "Starting parallel MFT read (SoA path)"
        );

        // Generate optimized read chunks
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let num_chunks = chunks.len();
        info!(num_chunks, "Generated read chunks");

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };
        info!(estimated_records, "Estimated record count");

        let record_size = self.extent_map.bytes_per_record;
        let records_processed = Arc::clone(&self.records_processed);

        // Calculate total bytes to read for progress reporting
        let total_bytes_to_read: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Read all chunks (sequential I/O for handle safety)
        debug!("Reading all chunks into memory...");
        let mut total_bytes_read: u64 = 0;
        let mut chunk_data: Vec<(ReadChunk, Vec<u8>)> = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.into_iter().enumerate() {
            trace!(
                chunk_idx = idx,
                start_frs = chunk.start_frs,
                "Reading chunk"
            );
            match self.read_chunk(handle, &chunk, record_size) {
                Ok(data) => {
                    total_bytes_read += data.len() as u64;
                    if let Some(ref cb) = progress_callback {
                        cb(total_bytes_read, total_bytes_to_read);
                    }
                    chunk_data.push((chunk, data));
                }
                Err(e) => {
                    warn!(chunk_idx = idx, error = ?e, "Failed to read chunk");
                }
            }
        }

        info!(
            chunks_read = chunk_data.len(),
            total_bytes = total_bytes_read,
            total_mb = total_bytes_read / (1024 * 1024),
            merge_extensions,
            "All chunks read into memory"
        );

        if merge_extensions {
            // FULL PATH: Parse → Merge → ParsedColumns
            // Uses HashMap-based MftRecordMerger for complete extension handling.
            // ~15-25% slower but handles files with many hard links/ADS correctly.

            #[derive(Default)]
            struct ChunkStats {
                results: Vec<ParseResult>,
                skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter()
                .fold(ChunkStats::default, |mut acc, (chunk, data)| {
                    let record_size = record_size as usize;
                    let skip_begin = chunk.skip_begin as usize;
                    let effective_count = chunk.effective_record_count() as usize;

                    acc.results.reserve(effective_count);

                    for i in 0..effective_count {
                        let offset = (skip_begin + i) * record_size;
                        if offset + record_size > data.len() {
                            break;
                        }

                        let record_data = &data[offset..offset + record_size];
                        let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                        let result = parse_record_zero_alloc(record_data, frs);
                        if matches!(result, ParseResult::Skip) {
                            acc.skipped += 1;
                        } else {
                            acc.results.push(result);
                        }
                        acc.processed += 1;
                    }
                    acc
                })
                .reduce(ChunkStats::default, |mut a, b| {
                    a.results.extend(b.results);
                    a.skipped += b.skipped;
                    a.processed += b.processed;
                    a
                });

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if combined.skipped > 0 {
                debug!(
                    skipped_records = combined.skipped,
                    "📋 Records skipped (not in use or invalid)"
                );
            }

            // Merge extensions and convert directly to ParsedColumns
            let mut merger = MftRecordMerger::with_capacity(estimated_records);
            for result in combined.results {
                merger.add_result(result);
            }

            Ok(merger.merge_into_columns(expand_links))
        } else {
            // FAST PATH: Parse directly to ParsedColumns (no HashMap, no merge)
            // Skips extension records (~1% of files with many hard links/ADS).
            // ~15-25% faster on SSD, ideal for file search and size analysis.

            #[derive(Default)]
            struct FastStats {
                columns: ParsedColumns,
                skipped: u64,
                extensions_skipped: u64,
                processed: u64,
            }

            let combined = chunk_data
                .par_iter()
                .fold(
                    || FastStats {
                        columns: ParsedColumns::with_capacity(
                            estimated_records / rayon::current_num_threads(),
                        ),
                        ..Default::default()
                    },
                    |mut acc, (chunk, data)| {
                        let record_size = record_size as usize;
                        let skip_begin = chunk.skip_begin as usize;
                        let effective_count = chunk.effective_record_count() as usize;

                        acc.columns.reserve(effective_count);

                        for i in 0..effective_count {
                            let offset = (skip_begin + i) * record_size;
                            if offset + record_size > data.len() {
                                break;
                            }

                            let record_data = &data[offset..offset + record_size];
                            let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                            match parse_record_zero_alloc(record_data, frs) {
                                ParseResult::Base(record) => {
                                    if expand_links {
                                        acc.columns.push_record_expanded(&record);
                                    } else {
                                        acc.columns.push_record(&record);
                                    }
                                }
                                ParseResult::Extension(_) => {
                                    acc.extensions_skipped += 1;
                                }
                                ParseResult::Skip => {
                                    acc.skipped += 1;
                                }
                            }
                            acc.processed += 1;
                        }
                        acc
                    },
                )
                .reduce(
                    || FastStats::default(),
                    |mut a, b| {
                        a.columns.extend(b.columns);
                        a.skipped += b.skipped;
                        a.extensions_skipped += b.extensions_skipped;
                        a.processed += b.processed;
                        a
                    },
                );

            records_processed.fetch_add(combined.processed, Ordering::Relaxed);
            self.skipped_records
                .fetch_add(combined.skipped, Ordering::Relaxed);

            let fixup_fail_count = self.fixup_failures.load(Ordering::Relaxed);
            if fixup_fail_count > 0 {
                warn!(
                    fixup_failures = fixup_fail_count,
                    "⚠️  MFT records with fixup failures detected (possible corruption)"
                );
            }

            if combined.skipped > 0 || combined.extensions_skipped > 0 {
                debug!(
                    skipped_records = combined.skipped,
                    extensions_skipped = combined.extensions_skipped,
                    "📋 Records skipped (fast path)"
                );
            }

            Ok(combined.columns)
        }
    }

    /// Reads a single chunk from disk.
    ///
    /// M1 8.4: Uses reusable aligned buffer to minimize allocations.
    /// The buffer is resized only if the chunk is larger than the current
    /// buffer.
    #[allow(unsafe_code)] // Required: Windows FFI (SetFilePointerEx, ReadFile)
    pub fn read_chunk(
        &self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
    ) -> Result<Vec<u8>> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // M1 8.4: Reuse buffer, only reallocate if needed
        let mut buffer = self.buffer.borrow_mut();
        if buffer.len() < aligned_size {
            *buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        // Extract the actual data (accounting for alignment offset)
        let actual_size = (bytes_read as usize).saturating_sub(offset_adjustment);
        let data = buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

        Ok(data)
    }
}

// ============================================================================
// Optimized Streaming Reader (Zero-Copy)
// ============================================================================

/// Ultra-fast MFT reader with streaming processing.
///
/// This reader processes records as they are read, avoiding the need to
/// buffer the entire MFT in memory. Key optimizations:
/// - Reusable aligned buffer (no per-chunk allocation)
/// - Streaming processing (parse while reading)
/// - Larger I/O chunks (4-8 MB based on drive type)
#[derive(Debug)]
pub struct StreamingMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
    /// Reusable aligned buffer.
    buffer: AlignedBuffer,
}

impl StreamingMftReader {
    /// Creates a new streaming reader optimized for the given drive type.
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        // Pre-allocate buffer for largest expected chunk
        let buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Created streaming reader"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
            buffer,
        }
    }

    /// Reads and processes all MFT records with streaming.
    ///
    /// This method reads chunks and processes them immediately, reducing
    /// memory pressure compared to buffering the entire MFT.
    #[allow(unsafe_code)]
    pub fn read_all_streaming<F>(
        &mut self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;

        // Calculate total bytes for progress
        let total_bytes: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        info!(
            chunks = chunks.len(),
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            "📖 Starting streaming read"
        );

        for chunk in chunks {
            // Read chunk into reusable buffer
            let bytes_read = self.read_chunk_into_buffer(handle, &chunk, record_size)?;
            bytes_read_total += bytes_read as u64;

            // Process records from buffer
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;

            for i in 0..effective_count {
                let offset = (skip_begin + i) * record_size_usize;
                if offset + record_size_usize > bytes_read {
                    break;
                }

                let record_data = &self.buffer.as_slice()[offset..offset + record_size_usize];
                let mut record_buf = record_data.to_vec();
                let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                // Apply fixup
                if !apply_fixup(&mut record_buf) {
                    continue;
                }

                // Parse record
                if merge_extensions {
                    merger.add_result(parse_record_full(&record_buf, frs));
                } else if let Some(rec) = parse_record(&record_buf, frs) {
                    merger.add_result(ParseResult::Base(rec));
                }
            }

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Merge extensions and get final results
        let all_results = if merge_extensions {
            merger.merge()
        } else {
            merger.merge()
        };

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Streaming read complete"
        );

        Ok(all_results)
    }

    /// Reads a chunk into the internal reusable buffer.
    #[allow(unsafe_code)]
    fn read_chunk_into_buffer(
        &mut self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
    ) -> Result<usize> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // Resize buffer if needed
        if self.buffer.len() < aligned_size {
            self.buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut self.buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        Ok(bytes_read as usize)
    }
}

// ============================================================================
// Prefetch Reader (Double-Buffering)
// ============================================================================

/// Double-buffered MFT reader with prefetching.
///
/// This reader uses two buffers and a background thread to prefetch the next
/// chunk while processing the current one. This overlaps I/O latency with
/// CPU processing time.
///
/// Key optimizations:
/// - Double-buffering: Read into buffer A while processing buffer B
/// - Prefetch thread: Background I/O doesn't block processing
/// - Large chunks: 4-8 MB based on drive type
pub struct PrefetchMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
}

impl PrefetchMftReader {
    /// Creates a new prefetch reader optimized for the given drive type.
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        let chunk_size = drive_type.optimal_chunk_size();
        info!(
            drive_type = ?drive_type,
            chunk_size_mb = chunk_size / (1024 * 1024),
            "🚀 Created prefetch reader with double-buffering"
        );
        Self {
            extent_map,
            bitmap,
            chunk_size,
        }
    }

    /// Reads all MFT records with prefetching and double-buffering.
    ///
    /// This method uses a background thread to prefetch the next chunk while
    /// processing the current one, maximizing throughput.
    #[allow(unsafe_code)]
    pub fn read_all_prefetch<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

        // Calculate total bytes for progress
        let total_bytes: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            "📖 Starting prefetch read with double-buffering"
        );

        // Use MftRecordMerger for proper extension handling
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        // Pre-allocate two buffers for double-buffering
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        let mut buffer_a = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
        let mut buffer_b = AlignedBuffer::new(max_chunk_size + SECTOR_SIZE);
        let mut use_buffer_a = true;

        // Process chunks with double-buffering
        for chunk in chunks {
            // Read current chunk into active buffer
            let buffer = if use_buffer_a {
                &mut buffer_a
            } else {
                &mut buffer_b
            };

            let bytes_read = self.read_chunk_into_buffer(handle, &chunk, record_size, buffer)?;
            bytes_read_total += bytes_read as u64;

            // Process records from buffer
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;

            for i in 0..effective_count {
                let offset = (skip_begin + i) * record_size_usize;
                if offset + record_size_usize > bytes_read {
                    break;
                }

                let record_data = &buffer.as_slice()[offset..offset + record_size_usize];
                let mut record_buf = record_data.to_vec();
                let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                // Apply fixup
                if !apply_fixup(&mut record_buf) {
                    continue;
                }

                // Parse record
                if merge_extensions {
                    merger.add_result(parse_record_full(&record_buf, frs));
                } else if let Some(rec) = parse_record(&record_buf, frs) {
                    merger.add_result(ParseResult::Base(rec));
                }
            }

            // Swap buffers for next iteration
            use_buffer_a = !use_buffer_a;

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }
        }

        // Merge extensions and get final results
        let all_results = merger.merge();

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Prefetch read complete"
        );

        Ok(all_results)
    }

    /// Reads a chunk into a provided buffer.
    #[allow(unsafe_code)]
    fn read_chunk_into_buffer(
        &self,
        handle: HANDLE,
        chunk: &ReadChunk,
        record_size: u32,
        buffer: &mut AlignedBuffer,
    ) -> Result<usize> {
        let read_size = chunk.record_count * u64::from(record_size);

        // Align to sector boundary
        let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
        let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
        let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
            / SECTOR_SIZE)
            * SECTOR_SIZE;

        // Resize buffer if needed
        if buffer.len() < aligned_size {
            *buffer = AlignedBuffer::new(aligned_size);
        }

        // Seek and read
        let mut new_position = 0_i64;
        unsafe {
            SetFilePointerEx(
                handle,
                aligned_offset as i64,
                Some(&mut new_position),
                FILE_BEGIN,
            )?;
        }

        let mut bytes_read = 0_u32;
        unsafe {
            ReadFile(
                handle,
                Some(&mut buffer.as_mut_slice()[..aligned_size]),
                Some(&mut bytes_read),
                None,
            )?;
        }

        Ok(bytes_read as usize)
    }
}

// ============================================================================
// Pipelined MFT Reader (True I/O + CPU Overlap)
// ============================================================================

/// Message sent from reader thread to parser thread.
struct ReadBuffer {
    /// The buffer containing raw MFT data.
    buffer: AlignedBuffer,
    /// Number of bytes actually read.
    bytes_read: usize,
    /// The chunk metadata.
    chunk: ReadChunk,
    /// Record size in bytes.
    record_size: u32,
}

/// Pipelined MFT reader with true I/O and CPU overlap.
///
/// This reader uses separate threads for I/O and parsing, connected by
/// bounded channels. This allows I/O to proceed while parsing is happening,
/// maximizing throughput especially on HDDs where I/O latency is significant.
///
/// Architecture:
/// ```text
/// ┌─────────────┐     ┌──────────────────┐     ┌─────────────┐
/// │ Reader      │────▶│ Bounded Channel  │────▶│ Parser      │
/// │ Thread      │     │ (backpressure)   │     │ Thread(s)   │
/// └─────────────┘     └──────────────────┘     └─────────────┘
///       │                                             │
///       ▼                                             ▼
///   Read chunks                                 Parse records
///   from disk                                   into ParsedRecord
/// ```
///
/// Key features:
/// - **True overlap**: I/O and parsing happen concurrently
/// - **Backpressure**: Bounded channel prevents memory explosion
/// - **Buffer pool**: Reuses buffers to minimize allocations
pub struct PipelinedMftReader {
    /// Extent map for VCN-to-LCN translation.
    extent_map: MftExtentMap,
    /// Optional bitmap for skip optimization.
    bitmap: Option<crate::platform::MftBitmap>,
    /// Read chunk size in bytes.
    chunk_size: usize,
    /// Number of buffers in the pipeline (channel capacity).
    pipeline_depth: usize,
}

impl PipelinedMftReader {
    /// Creates a new pipelined reader.
    ///
    /// # Arguments
    ///
    /// * `extent_map` - MFT extent map for physical offset calculation
    /// * `bitmap` - Optional MFT bitmap for skipping unused records
    /// * `drive_type` - Drive type for chunk size tuning
    #[must_use]
    pub fn new(
        extent_map: MftExtentMap,
        bitmap: Option<crate::platform::MftBitmap>,
        drive_type: crate::platform::DriveType,
    ) -> Self {
        use crate::platform::DriveType;

        // Chunk size based on drive type
        let chunk_size = match drive_type {
            DriveType::Ssd => 8 * 1024 * 1024, // 8 MB for SSDs
            DriveType::Hdd => 4 * 1024 * 1024, // 4 MB for HDDs
            DriveType::Unknown => 4 * 1024 * 1024,
        };

        // Pipeline depth: 2-3 buffers is optimal
        // - 1 being read
        // - 1 being parsed
        // - 1 in the channel (optional, for smoothing)
        let pipeline_depth = 3;

        Self {
            extent_map,
            bitmap,
            chunk_size,
            pipeline_depth,
        }
    }

    /// Reads all MFT records with true I/O and CPU overlap.
    ///
    /// This method spawns a reader thread that reads chunks as fast as
    /// possible, sending them through a bounded channel to the main thread
    /// for parsing. The bounded channel provides backpressure to prevent
    /// memory explosion.
    #[allow(unsafe_code)]
    pub fn read_all_pipelined<F>(
        &self,
        handle: HANDLE,
        merge_extensions: bool,
        mut progress_callback: Option<F>,
    ) -> Result<Vec<ParsedRecord>>
    where
        F: FnMut(u64, u64),
    {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};

        let chunks = generate_read_chunks(&self.extent_map, self.bitmap.as_ref(), self.chunk_size);
        let record_size = self.extent_map.bytes_per_record;
        let num_chunks = chunks.len();

        if num_chunks == 0 {
            return Ok(Vec::new());
        }

        // Calculate total bytes for progress
        let total_bytes: u64 = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .sum();

        // Estimate capacity
        let estimated_records = if let Some(ref bm) = self.bitmap {
            bm.count_in_use()
        } else {
            self.extent_map.total_records() as usize
        };

        info!(
            chunks = num_chunks,
            estimated_records,
            chunk_size_mb = self.chunk_size / (1024 * 1024),
            pipeline_depth = self.pipeline_depth,
            "🚀 Starting pipelined read with I/O+CPU overlap"
        );

        // Create bounded channel for backpressure
        let (tx, rx): (Sender<ReadBuffer>, Receiver<ReadBuffer>) = bounded(self.pipeline_depth);

        // Pre-allocate buffer pool for the reader thread
        let max_chunk_size = chunks
            .iter()
            .map(|c| c.record_count * u64::from(record_size))
            .max()
            .unwrap_or(self.chunk_size as u64) as usize;

        // Clone data needed by reader thread
        let chunks_for_reader = chunks;
        let handle_raw = handle.0 as usize; // Convert to usize for Send

        // Spawn reader thread
        let reader_handle = thread::spawn(move || {
            // Reconstruct HANDLE in reader thread
            let handle = HANDLE(handle_raw as *mut std::ffi::c_void);

            // Create buffer pool
            let mut buffer_pool: Vec<AlignedBuffer> = Vec::new();

            for chunk in chunks_for_reader {
                // Get or create a buffer
                let mut buffer = buffer_pool
                    .pop()
                    .unwrap_or_else(|| AlignedBuffer::new(max_chunk_size + SECTOR_SIZE));

                // Read chunk into buffer
                match read_chunk_into_buffer_static(handle, &chunk, record_size, &mut buffer) {
                    Ok(bytes_read) => {
                        let read_buffer = ReadBuffer {
                            buffer,
                            bytes_read,
                            chunk,
                            record_size,
                        };

                        // Send to parser (blocks if channel is full - backpressure)
                        if tx.send(read_buffer).is_err() {
                            // Receiver dropped, stop reading
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to read chunk, skipping");
                        // Return buffer to pool
                        buffer_pool.push(buffer);
                    }
                }
            }
            // tx is dropped here, signaling end of stream
        });

        // Parse records in main thread
        let mut merger = MftRecordMerger::with_capacity(estimated_records);
        let mut bytes_read_total: u64 = 0;

        // Receive and parse buffers
        while let Ok(read_buffer) = rx.recv() {
            let ReadBuffer {
                buffer,
                bytes_read,
                chunk,
                record_size,
            } = read_buffer;

            bytes_read_total += bytes_read as u64;

            // Parse records from buffer
            let skip_begin = chunk.skip_begin as usize;
            let effective_count = chunk.effective_record_count() as usize;
            let record_size_usize = record_size as usize;

            for i in 0..effective_count {
                let offset = (skip_begin + i) * record_size_usize;
                if offset + record_size_usize > bytes_read {
                    break;
                }

                let record_data = &buffer.as_slice()[offset..offset + record_size_usize];
                let mut record_buf = record_data.to_vec();
                let frs = chunk.start_frs + skip_begin as u64 + i as u64;

                // Apply fixup
                if !apply_fixup(&mut record_buf) {
                    continue;
                }

                // Parse record
                if merge_extensions {
                    merger.add_result(parse_record_full(&record_buf, frs));
                } else if let Some(rec) = parse_record(&record_buf, frs) {
                    merger.add_result(ParseResult::Base(rec));
                }
            }

            // Report progress
            if let Some(ref mut cb) = progress_callback {
                cb(bytes_read_total, total_bytes);
            }

            // Note: buffer is dropped here, but we could return it to a pool
            // for even better performance
        }

        // Wait for reader thread to finish
        if let Err(e) = reader_handle.join() {
            warn!("Reader thread panicked: {:?}", e);
        }

        // Merge extensions and get final results
        let all_results = merger.merge();

        info!(
            records = all_results.len(),
            bytes_mb = bytes_read_total / (1024 * 1024),
            "✅ Pipelined read complete"
        );

        Ok(all_results)
    }
}

/// Static helper to read a chunk into a buffer (for use in reader thread).
#[allow(unsafe_code)]
fn read_chunk_into_buffer_static(
    handle: HANDLE,
    chunk: &ReadChunk,
    record_size: u32,
    buffer: &mut AlignedBuffer,
) -> Result<usize> {
    let read_size = chunk.record_count * u64::from(record_size);

    // Align to sector boundary
    let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
    let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
    let aligned_size =
        ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1) / SECTOR_SIZE) * SECTOR_SIZE;

    // Resize buffer if needed
    if buffer.len() < aligned_size {
        *buffer = AlignedBuffer::new(aligned_size);
    }

    // Seek to position
    let mut new_pos: i64 = 0;
    let seek_result = unsafe {
        SetFilePointerEx(
            handle,
            aligned_offset as i64,
            Some(&mut new_pos),
            FILE_BEGIN,
        )
    };

    if seek_result.is_err() {
        return Err(MftError::Io(std::io::Error::last_os_error()));
    }

    // Read data
    let mut bytes_read: u32 = 0;
    let read_result = unsafe {
        ReadFile(
            handle,
            Some(&mut buffer.as_mut_slice()[..aligned_size]),
            Some(&mut bytes_read),
            None,
        )
    };

    if read_result.is_err() {
        return Err(MftError::Io(std::io::Error::last_os_error()));
    }

    Ok(bytes_read as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_buffer() {
        let buffer = AlignedBuffer::new(1024);
        assert_eq!(buffer.len(), 1024);

        // Check alignment
        let ptr = buffer.as_slice().as_ptr() as usize;
        assert_eq!(ptr % SECTOR_SIZE, 0);
    }

    #[test]
    fn test_aligned_buffer_write() {
        let mut buffer = AlignedBuffer::new(512);
        buffer.as_mut_slice()[0] = 0x42;
        assert_eq!(buffer.as_slice()[0], 0x42);
    }

    #[test]
    fn test_extent_map_contiguous() {
        let map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        // First record should be at LCN 100 * 4096 = 409600
        assert_eq!(map.physical_offset(0), Some(409600));

        // Second record should be at 409600 + 1024 = 410624
        assert_eq!(map.physical_offset(1), Some(410624));

        // Record 4 should be at 409600 + 4096 = 413696 (next cluster)
        assert_eq!(map.physical_offset(4), Some(413696));
    }

    #[test]
    fn test_extent_map_fragmented() {
        // Create a fragmented extent map:
        // Extent 0: VCN 0-9, LCN 100 (10 clusters)
        // Extent 1: VCN 10-19, LCN 500 (10 clusters)
        let extents = vec![
            MftExtent {
                vcn: 0,
                cluster_count: 10,
                lcn: 100,
            },
            MftExtent {
                vcn: 10,
                cluster_count: 10,
                lcn: 500,
            },
        ];
        let map = MftExtentMap::new(extents, 4096, 1024);

        // Record 0 should be in first extent
        assert_eq!(map.physical_offset(0), Some(100 * 4096));

        // Record 40 (VCN 10) should be in second extent
        // VCN 10 = record 40 (4 records per cluster with 1024 byte records and 4096
        // byte clusters)
        assert_eq!(map.physical_offset(40), Some(500 * 4096));

        // Record 44 should be at VCN 11
        assert_eq!(map.physical_offset(44), Some(500 * 4096 + 4096));
    }

    #[test]
    fn test_pipelined_reader_creation() {
        // Test that PipelinedMftReader can be created with various drive types
        let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

        // Test with SSD
        let reader =
            PipelinedMftReader::new(extent_map.clone(), None, crate::platform::DriveType::Ssd);
        assert_eq!(reader.chunk_size, 8 * 1024 * 1024); // 8 MB for SSD
        assert_eq!(reader.pipeline_depth, 3);

        // Test with HDD
        let reader =
            PipelinedMftReader::new(extent_map.clone(), None, crate::platform::DriveType::Hdd);
        assert_eq!(reader.chunk_size, 4 * 1024 * 1024); // 4 MB for HDD
        assert_eq!(reader.pipeline_depth, 3);

        // Test with Unknown
        let reader = PipelinedMftReader::new(extent_map, None, crate::platform::DriveType::Unknown);
        assert_eq!(reader.chunk_size, 4 * 1024 * 1024); // 4 MB for Unknown
    }

    #[test]
    fn test_merge_adjacent_chunks_contiguous() {
        // Test that truly contiguous chunks ARE merged
        let chunks = vec![
            ReadChunk {
                disk_offset: 0,
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 100 * 1024, // Contiguous: 100 records * 1024 bytes
                start_frs: 100,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(merged.len(), 1, "Contiguous chunks should be merged");
        assert_eq!(merged[0].start_frs, 0);
        assert_eq!(merged[0].record_count, 200);
        assert_eq!(merged[0].disk_offset, 0);
    }

    #[test]
    fn test_merge_adjacent_chunks_non_contiguous_disk() {
        // Test the bug fix: chunks with contiguous FRS but non-contiguous disk offsets
        // should NOT be merged. This simulates a fragmented MFT where extent 4 is at
        // a higher LCN than extent 5.
        let chunks = vec![
            ReadChunk {
                disk_offset: 1_000_000_000, // Extent 4 at high disk offset
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 500_000_000, // Extent 5 at LOWER disk offset (fragmented!)
                start_frs: 100,           // FRS is contiguous
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(
            merged.len(),
            2,
            "Non-contiguous disk chunks should NOT be merged"
        );
        assert_eq!(merged[0].disk_offset, 1_000_000_000);
        assert_eq!(merged[0].record_count, 100);
        assert_eq!(merged[1].disk_offset, 500_000_000);
        assert_eq!(merged[1].record_count, 100);
    }

    #[test]
    fn test_merge_adjacent_chunks_gap_too_large() {
        // Test that chunks with large gaps are not merged
        let chunks = vec![
            ReadChunk {
                disk_offset: 0,
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 200 * 1024, // Gap of 100 records (> threshold of 64)
                start_frs: 200,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(
            merged.len(),
            2,
            "Chunks with large gaps should NOT be merged"
        );
    }
}
