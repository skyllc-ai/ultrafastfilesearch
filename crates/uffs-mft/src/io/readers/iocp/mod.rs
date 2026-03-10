//! IOCP-based reader helpers and implementations.

pub(super) use super::zero_copy::parse_buffer_zero_copy_inner;
use super::*;

mod multi_volume;
mod reader;
mod shared;

pub use multi_volume::{MultiVolumeIoOp, MultiVolumeIocpReader, VolumeState, prepare_volume_state};
pub use reader::IocpMftReader;
pub use shared::{IoCompletionPort, OverlappedRead};
