// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! IOCP-based reader helpers and implementations.

pub(super) use super::zero_copy::parse_buffer_zero_copy_inner;
#[expect(
    clippy::wildcard_imports,
    reason = "parent module's `pub(super) use` prelude \
              (HANDLE, MftError, ReadFile, rayon::prelude::*, tracing \
              macros, etc.) is designed to be consumed by submodules; \
              re-enumerating ~15 items here would duplicate the prelude \
              across every sibling reader file"
)]
use super::*;

mod multi_volume;
mod reader;
mod shared;

pub use multi_volume::{MultiVolumeIoOp, MultiVolumeIocpReader, VolumeState, prepare_volume_state};
pub use reader::IocpMftReader;
pub use shared::{IoCompletionPort, OverlappedRead};
