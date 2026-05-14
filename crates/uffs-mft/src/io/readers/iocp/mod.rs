// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! IOCP-based reader helpers and implementations.
//!
//! The local [`prelude`] re-exports `super::prelude` (the readers-wide
//! prelude) plus iocp-internal items (`IoCompletionPort`, `OverlappedRead`)
//! that iocp's own children need.  Children import via
//! `use super::prelude::*;`, exempt from `clippy::wildcard_imports` because
//! the module is named `prelude`.

/// Re-exports the readers-wide prelude plus iocp-internal items
/// (`IoCompletionPort`, `OverlappedRead`) that iocp's own children need.
/// The module name `prelude` is exempt from `clippy::wildcard_imports`.
mod prelude {
    pub(super) use super::super::prelude::*;
    pub(super) use super::shared::{IoCompletionPort, OverlappedRead, set_overlapped_offset};
}

mod multi_volume;
mod reader;
mod shared;

// Public IOCP reader API.  MultiVolumeIocpReader / VolumeState /
// prepare_volume_state are consumed by the uffs-mft bin (benchmark_index)
// via external-style paths; IocpMftReader was Phase 2.5-demoted from pub
// (commit 1529cb162) — restored here to preserve its public contract.
pub use multi_volume::{MultiVolumeIoOp, MultiVolumeIocpReader, VolumeState, prepare_volume_state};
pub use reader::IocpMftReader;
pub(crate) use shared::{IoCompletionPort, OverlappedRead, set_overlapped_offset};
