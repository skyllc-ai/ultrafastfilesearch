//! Error types for MFT operations.

use thiserror::Error;

/// Result type for MFT operations.
pub type Result<T> = core::result::Result<T, MftError>;

/// Errors that can occur during MFT operations.
#[derive(Error, Debug)]
pub enum MftError {
    /// Failed to open volume for reading.
    #[error("Failed to open volume '{volume}': {source}")]
    VolumeOpen {
        /// The volume letter that failed to open.
        volume: char,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Volume is not NTFS formatted.
    #[error("Volume '{0}' is not NTFS formatted")]
    NotNtfs(char),

    /// Insufficient privileges to read MFT.
    #[error("Insufficient privileges to read MFT. Run as Administrator.")]
    InsufficientPrivileges,

    /// Failed to read boot sector.
    #[error("Failed to read boot sector: {0}")]
    BootSectorRead(String),

    /// Invalid boot sector data.
    #[error("Invalid boot sector: {0}")]
    InvalidBootSector(String),

    /// Failed to read MFT record.
    #[error("Failed to read MFT record {frs}: {reason}")]
    RecordRead {
        /// The File Reference Segment number.
        frs: u64,
        /// The reason for the failure.
        reason: String,
    },

    /// Invalid MFT record (bad magic number).
    #[error("Invalid MFT record at FRS {0}: bad magic number")]
    InvalidRecord(u64),

    /// Attribute parsing error.
    #[error("Failed to parse attribute at offset {offset}: {reason}")]
    AttributeParse {
        /// The byte offset where parsing failed.
        offset: u64,
        /// The reason for the parse failure.
        reason: String,
    },

    /// I/O error during disk operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Polars error during `DataFrame` operations.
    #[error("DataFrame error: {0}")]
    Polars(#[from] uffs_polars::PolarsError),

    /// Parquet file error.
    #[error("Parquet error: {0}")]
    Parquet(String),

    /// Invalid data format.
    #[error("Invalid data: {0}")]
    InvalidData(String),

    /// Failed to get retrieval pointers (MFT extents).
    #[error("Failed to get MFT extents: {0}")]
    RetrievalPointers(String),

    /// Windows API error.
    #[cfg(windows)]
    #[error("Windows API error: {0}")]
    Windows(#[from] windows::core::Error),

    /// Feature not available on this platform.
    #[error("MFT reading is only available on Windows")]
    PlatformNotSupported,

    /// Invalid input provided.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}
