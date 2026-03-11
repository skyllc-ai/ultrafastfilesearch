//! Reader implementations and async I/O orchestration for MFT ingestion.

pub(super) use std::sync::Arc;
pub(super) use std::sync::atomic::{AtomicU64, Ordering};

pub(super) use rayon::prelude::*;

use super::*;

mod basic;
mod iocp;
mod parallel;
mod pipelined;
mod prefetch;
mod streaming;
mod zero_copy;

pub use basic::{BatchMftReader, MftRecordReader};
pub use iocp::{
    IoCompletionPort, IocpMftReader, MultiVolumeIoOp, MultiVolumeIocpReader, OverlappedRead,
    VolumeState, prepare_volume_state,
};
pub use parallel::{ParallelMftReader, ReadParseTiming};
pub use pipelined::PipelinedMftReader;
pub use prefetch::PrefetchMftReader;
pub use streaming::StreamingMftReader;
