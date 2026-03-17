//! Low-level I/O operations for MFT reading.
//!
//! This module provides efficient disk I/O for reading MFT records:
//! - Aligned buffer management for direct I/O
//! - Sector-aligned reads
//! - Multi-sector fixup (Update Sequence Array)
//! - Fragmented MFT support via extent mapping
//! - Chunk planning and reader implementations tuned by drive type
//!
//! Available on all platforms for offline MFT processing (chaos mode, testing).
//! Live MFT access via HANDLE is Windows-only and gated per-function.

// Tracing is needed for all submodules (chunking, merger, parser, etc.)
use tracing::{debug, info, trace, warn};

#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

#[cfg(windows)]
use crate::error::{MftError, Result};
pub use crate::ntfs::SECTOR_SIZE;
// MftExtent is needed in test mode for extent_map
use crate::platform::MftExtent;
#[cfg(windows)]
use crate::platform::VolumeHandle;

mod aligned_buffer;
mod chunking;
mod extent_map;
mod fixup;
mod merger;
mod parser;
// readers module available on all platforms (contains ChaosMftReader for offline MFT)
pub mod readers;

// Chaos test harness - works with offline MFT files on any platform
#[cfg(test)]
#[path = "io/readers/parallel/tests_chaos.rs"]
mod tests_chaos;

pub use aligned_buffer::AlignedBuffer;
pub use chunking::{ReadChunk, generate_precise_read_chunks, generate_read_chunks};
pub use extent_map::MftExtentMap;
pub use fixup::apply_fixup;
pub use merger::MftRecordMerger;
pub use parser::{
    ExtensionAttributes, ParseResult, ParsedColumns, ParsedRecord,
    add_missing_parent_placeholders_to_vec, create_placeholder_record, parse_record,
    parse_record_full, parse_record_to_index, parse_record_zero_alloc,
};
#[expect(deprecated, reason = "re-exporting deprecated API for backward compatibility")]
pub use parser::parse_record_to_fragment;
// Export Windows-specific readers (require HANDLE)
#[cfg(windows)]
pub use readers::{
    BatchMftReader, IoCompletionPort, IocpMftReader, MftRecordReader, MultiVolumeIoOp,
    MultiVolumeIocpReader, OverlappedRead, ParallelMftReader, PipelinedMftReader,
    PrefetchMftReader, ReadParseTiming, StreamingMftReader, VolumeState, prepare_volume_state,
};
