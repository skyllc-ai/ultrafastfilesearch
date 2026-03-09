//! Read mode types and drive-type-based selection helpers.

#[cfg(windows)]
use crate::platform::DriveType;

/// Read mode for MFT operations.
///
/// Different modes optimize for different drive types and workloads:
/// - `Parallel`: Best for SSDs - reads all chunks then parses in parallel
/// - `Streaming`: Best for HDDs - sequential reads with immediate parsing
/// - `Prefetch`: Best for HDDs - double-buffered prefetch for I/O overlap
/// - `Pipelined`: True I/O and CPU overlap with separate threads
/// - `PipelinedParallel`: Pipelined I/O with multi-core parallel parsing
/// - `Auto`: Automatically selects based on detected drive type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MftReadMode {
    /// Automatic mode selection based on drive type (default).
    /// - SSD → `Parallel`
    /// - HDD → `PipelinedParallel`
    /// - Unknown → `Parallel`
    #[default]
    Auto,
    /// Parallel mode: Read all chunks into memory, then parse in parallel.
    /// Best for SSDs where random I/O is fast.
    Parallel,
    /// Streaming mode: Sequential reads with immediate parsing.
    /// Lower memory usage, good for HDDs.
    Streaming,
    /// Prefetch mode: Double-buffered reads for I/O overlap.
    /// Good for HDDs - overlaps next read with current parse.
    Prefetch,
    /// Pipelined mode: True I/O and CPU overlap with separate threads.
    /// Best for HDDs - reader thread queues chunks while parser processes.
    /// Note: Parsing is single-threaded. Use `PipelinedParallel` for
    /// multi-core.
    Pipelined,
    /// Pipelined parallel mode: Pipelined I/O with multi-core parallel parsing.
    /// Best for HDDs with multi-core CPUs.
    PipelinedParallel,
    /// IOCP parallel mode: Windows I/O Completion Ports with multiple
    /// concurrent reads.
    IocpParallel,
    /// Bulk mode: queue reads first, then parse after collection.
    Bulk,
    /// Bulk IOCP mode: queues all reads to IOCP at once.
    BulkIocp,
    /// Sliding window IOCP mode with two reads in flight.
    SlidingIocp,
    /// Sliding window IOCP with inline parsing and direct index building.
    SlidingIocpInline,
}

impl MftReadMode {
    /// Returns the mode name as a string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Parallel => "parallel",
            Self::Streaming => "streaming",
            Self::Prefetch => "prefetch",
            Self::Pipelined => "pipelined",
            Self::PipelinedParallel => "pipelined-parallel",
            Self::IocpParallel => "iocp-parallel",
            Self::Bulk => "bulk",
            Self::BulkIocp => "bulk-iocp",
            Self::SlidingIocp => "sliding-iocp",
            Self::SlidingIocpInline => "sliding-iocp-inline",
        }
    }
}

impl core::fmt::Display for MftReadMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl core::str::FromStr for MftReadMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "parallel" => Ok(Self::Parallel),
            "streaming" => Ok(Self::Streaming),
            "prefetch" => Ok(Self::Prefetch),
            "pipelined" | "pipeline" => Ok(Self::Pipelined),
            "pipelined-parallel" | "pipelinedparallel" => Ok(Self::PipelinedParallel),
            "iocp-parallel" | "iocpparallel" | "iocp" => Ok(Self::IocpParallel),
            "bulk" => Ok(Self::Bulk),
            "bulk-iocp" | "bulkiocp" => Ok(Self::BulkIocp),
            "sliding-iocp" | "slidingiocp" | "sliding" => Ok(Self::SlidingIocp),
            "sliding-iocp-inline" | "slidingiocpinline" | "inline" => Ok(Self::SlidingIocpInline),
            _ => Err(format!(
                "Invalid read mode '{s}'. Valid options: auto, parallel, streaming, prefetch, pipelined, pipelined-parallel, iocp-parallel, bulk, bulk-iocp, sliding-iocp, sliding-iocp-inline"
            )),
        }
    }
}

/// Selects the effective read mode for the `DataFrame` path.
#[must_use]
#[cfg(windows)]
pub(super) const fn dataframe_effective_mode(
    mode: MftReadMode,
    drive_type: DriveType,
) -> MftReadMode {
    match mode {
        MftReadMode::Auto => match drive_type {
            DriveType::Nvme | DriveType::Ssd | DriveType::Hdd | DriveType::Unknown => {
                MftReadMode::SlidingIocp
            }
        },
        other => other,
    }
}

/// Selects the effective read mode for the lean-index path.
#[must_use]
#[cfg(windows)]
pub(super) const fn index_effective_mode(mode: MftReadMode, drive_type: DriveType) -> MftReadMode {
    match mode {
        MftReadMode::Auto => match drive_type {
            DriveType::Nvme | DriveType::Ssd | DriveType::Hdd | DriveType::Unknown => {
                MftReadMode::SlidingIocpInline
            }
        },
        other => other,
    }
}
