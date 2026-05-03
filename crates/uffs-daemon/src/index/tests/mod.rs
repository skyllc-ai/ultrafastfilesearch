// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Integration tests for `IndexManager` — split into thematic
//! submodules so each file stays under the 800 LOC policy while
//! keeping shared drive-builder fixtures co-located here.
//!
//! Submodules:
//!
//! * [`aggregate`] / [`aggregate_drilldown`] — `run_aggregations` handler tests
//!   covering presets, terms+sample drilldown, pagination, query-cache
//!   invalidation, and auto-concurrency.
//! * [`manager`] — search RPC, status (RSS / mimalloc), drive-letter inference,
//!   and the live-marker check.
//! * [`registry`] — `ShardRegistry` add/replace/remove, the legal transition
//!   graph, and `demote_letter` / `promote_letter` unit tests.
//! * [`ensure_warm`] — `ensure_warm_for_dispatch` happy-path, missing-cache,
//!   panicking-loader, parallel re-promote, and bloom-aware promote-side gating
//!   (Phase 4 task 4.11).
//! * [`idle_demote`] — `demote_idle_shards` TTL-driven cascade, round-trip
//!   query stats, and `shard.transition` event emission.
//! * [`lifecycle_hooks`] — Phase 5 task 5.8 / 5.9 / 5.10 `WorkingSetTrim` +
//!   `Prefetch` + `PressureSignal` injection tests, plus the `drives` RPC
//!   tier-marker enumeration.
//! * [`tracing_capture`] — shared `tracing::Subscriber` scaffold (`EventLog` /
//!   `CapturedEvent`) used by [`idle_demote`]'s `shard.transition`
//!   observability-contract tests.

#![expect(
    clippy::std_instead_of_alloc,
    reason = "test fixtures — `std::sync::Arc` matches the rest of the daemon's \
              test fixtures, no need to switch to `alloc::sync::Arc` for tests"
)]

use std::sync::Arc;

use uffs_client::protocol::AggregateSpecWire;
use uffs_core::compact::build_compact_index;
use uffs_core::search::backend::DriveIndex;
use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

use super::IndexManager;
use super::aggregation::AggregationRequest;

mod aggregate;
mod aggregate_drilldown;
mod body_loader_fakes;
mod ensure_warm;
mod idle_demote;
mod lifecycle_hooks;
mod manager;
mod registry;
mod tracing_capture;

/// Build a synthetic drive with root + 1 dir + 5 files of varied
/// sizes/extensions.
fn build_test_drive() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('C');

    // Root directory
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    // Subdirectory "Projects"
    let dir_name = "Projects";
    let dir_off = idx.add_name(dir_name);
    let dir_ext = idx.intern_extension(dir_name);
    let dir = idx.get_or_create(100);
    dir.stdinfo.set_directory(true);
    dir.stdinfo.flags = 0x10; // directory flag
    dir.first_name.name =
        IndexNameRef::new(dir_off, uffs_mft::len_to_u16(dir_name.len()), true, dir_ext);
    dir.first_name.parent_frs = ROOT_FRS;

    // Files with different extensions and sizes
    let files: &[(&str, u64, u64, u64)] = &[
        ("readme.md", 101, 500, 512),
        ("main.rs", 102, 2000, 4096),
        ("lib.rs", 103, 3000, 4096),
        ("config.toml", 104, 100, 512),
        ("data.bin", 105, 10_000, 16_384),
    ];

    for &(name, frs, size, allocated) in files {
        let off = idx.add_name(name);
        let ext = idx.intern_extension(name);
        let rec = idx.get_or_create(frs);
        rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
        rec.first_name.parent_frs = 100; // under Projects
        rec.first_stream.size = SizeInfo {
            length: size,
            allocated,
        };
        rec.stdinfo.flags = 0x20; // archive
        rec.stdinfo.modified = 1_000_000;
    }

    let (drive, _, _) = build_compact_index('C', &idx);
    drive
}

fn test_index() -> DriveIndex {
    DriveIndex {
        drives: vec![Arc::new(build_test_drive())],
    }
}

fn spec(kind: &str) -> AggregateSpecWire {
    AggregateSpecWire {
        kind: kind.to_owned(),
        ..AggregateSpecWire::default()
    }
}

/// Build a synthetic drive with letter `'D'` for multi-drive tests.
///
/// Same shape as [`build_test_drive`] but a different letter so a
/// 2-drive registry is unambiguous.
fn build_test_drive_d() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('D');
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    let file = "alpha.txt";
    let off = idx.add_name(file);
    let ext = idx.intern_extension(file);
    let rec = idx.get_or_create(200);
    rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(file.len()), true, ext);
    rec.first_name.parent_frs = ROOT_FRS;
    rec.first_stream.size = SizeInfo {
        length: 42,
        allocated: 512,
    };
    rec.stdinfo.flags = 0x20;
    rec.stdinfo.modified = 1_000_000;

    let (drive, _, _) = build_compact_index('D', &idx);
    drive
}

/// Build a synthetic drive with letter `'E'` — third drive for the
/// Phase 3 Commit E virtual-time tests (plan tasks 3.7 + 3.8) that
/// need to verify "queries on C only → D and E both demote, C
/// stays Warm" and "advance past parked TTL → all three Cold".
fn build_test_drive_e() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('E');
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    let file = "beta.bin";
    let off = idx.add_name(file);
    let ext = idx.intern_extension(file);
    let rec = idx.get_or_create(300);
    rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(file.len()), true, ext);
    rec.first_name.parent_frs = ROOT_FRS;
    rec.first_stream.size = SizeInfo {
        length: 84,
        allocated: 1024,
    };
    rec.stdinfo.flags = 0x20;
    rec.stdinfo.modified = 1_000_000;

    let (drive, _, _) = build_compact_index('E', &idx);
    drive
}

/// A `BodyLoader` that always returns `Some(self.body.clone())` —
/// used to verify the success path of `ensure_warm_for_dispatch`
/// without touching the platform cache directory.
struct FixedBodyLoader {
    body: Arc<uffs_core::compact::DriveCompactIndex>,
}

impl crate::cache::body_loader::BodyLoader for FixedBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        Some(Arc::clone(&self.body))
    }
}
