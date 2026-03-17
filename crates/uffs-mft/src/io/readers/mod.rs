//! Reader implementations and async I/O orchestration for MFT ingestion.

// Windows-specific readers (require HANDLE and Windows APIs)
#[cfg(windows)]
mod basic;
#[cfg(windows)]
mod iocp;
#[cfg(windows)]
mod pipelined;
#[cfg(windows)]
mod prefetch;
#[cfg(windows)]
mod streaming;
#[cfg(windows)]
mod zero_copy;

// Parallel reader available on all platforms (contains ChaosMftReader)
pub mod parallel;

#[cfg(windows)]
pub use basic::{BatchMftReader, MftRecordReader};
#[cfg(windows)]
pub use iocp::{
    IoCompletionPort, IocpMftReader, MultiVolumeIoOp, MultiVolumeIocpReader, OverlappedRead,
    VolumeState, prepare_volume_state,
};
#[cfg(windows)]
pub use parallel::ParallelMftReader;
pub use parallel::ReadParseTiming;
#[cfg(windows)]
pub use pipelined::PipelinedMftReader;
#[cfg(windows)]
pub use prefetch::PrefetchMftReader;
#[cfg(windows)]
pub use streaming::StreamingMftReader;
