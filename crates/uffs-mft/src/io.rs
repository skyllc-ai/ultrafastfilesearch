//! Low-level I/O operations for MFT reading.
//!
//! This module provides efficient disk I/O for reading MFT records:
//! - Aligned buffer management for direct I/O
//! - Sector-aligned reads
//! - Multi-sector fixup (Update Sequence Array)
//! - Fragmented MFT support via extent mapping
//! - Chunk planning and reader implementations tuned by drive type

#![cfg(windows)]

use std::cell::RefCell;
use std::mem::size_of;

use smallvec::SmallVec;
use tracing::{debug, info, trace, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

use crate::error::{MftError, Result};
pub use crate::ntfs::SECTOR_SIZE;
use crate::platform::{MftExtent, VolumeHandle};

mod aligned_buffer;
mod chunking;
mod extent_map;
mod fixup;
mod merger;
mod parser;
mod readers;

pub use aligned_buffer::AlignedBuffer;
pub use chunking::{ReadChunk, generate_precise_read_chunks, generate_read_chunks};
pub use extent_map::MftExtentMap;
pub use fixup::apply_fixup;
pub use merger::MftRecordMerger;
pub use parser::{
    ExtensionAttributes, ParseResult, ParsedColumns, ParsedRecord,
    add_missing_parent_placeholders_to_vec, create_placeholder_record, parse_record,
    parse_record_full, parse_record_to_fragment, parse_record_to_index, parse_record_zero_alloc,
};
pub use readers::{
    BatchMftReader, IoCompletionPort, IocpMftReader, MftRecordReader, MultiVolumeIoOp,
    MultiVolumeIocpReader, OverlappedRead, ParallelMftReader, PipelinedMftReader,
    PrefetchMftReader, ReadParseTiming, StreamingMftReader, VolumeState, prepare_volume_state,
};
