// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`ColumnStorage<T>`] — exercises both the `Vec`
//! variant (Phase 2a) and the `Mmap` variant (Phase 2b).
//!
//! Test pairs are kept symmetric: every `Vec`-variant test has an
//! `Mmap`-variant counterpart.  All `Mmap`-variant tests use the same
//! [`make_mmap_for_test`] helper, which builds an `Arc<Mmap>` over an
//! anonymous mmap (`memmap2::MmapMut::map_anon`) — the safe path that
//! works under `uffs-core`'s `#![forbid(unsafe_code)]`.  See
//! `docs/refactor/memory-tiering-implementation-plan.md` §3 Phase 2b
//! for the broader rollout.

use alloc::sync::Arc;
use core::mem::align_of;

use memmap2::Mmap;

use super::{ColumnStorage, MmapRegionError};

// ─────────────────────────────────────────────────────────────────────
// Phase 2a — `Vec` variant tests.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn empty_default_has_zero_len_and_is_empty() {
    let column: ColumnStorage<u32> = ColumnStorage::default();
    assert_eq!(column.len(), 0);
    assert!(column.is_empty());
    assert_eq!(column.as_slice(), &[] as &[u32]);
}

#[test]
fn from_vec_round_trips_through_into_vec() {
    let original = vec![1_u32, 2, 3, 4, 5];
    let column = ColumnStorage::from_vec(original.clone());
    assert_eq!(column.len(), 5);
    assert_eq!(column.as_slice(), original.as_slice());
    let recovered = column.into_vec();
    assert_eq!(recovered, original);
}

#[test]
fn deref_lets_call_sites_use_slice_methods() {
    let column = ColumnStorage::from(vec![10_u32, 20, 30]);
    // Read-side operations dispatched via Deref.
    assert_eq!(column.first(), Some(&10));
    assert_eq!(column.last(), Some(&30));
    assert_eq!(column.iter().sum::<u32>(), 60);
    assert_eq!(column.get(1), Some(&20));
    let collected: Vec<u32> = (&column).into_iter().copied().collect();
    assert_eq!(collected, vec![10, 20, 30]);
}

#[test]
fn deref_mut_lets_call_sites_mutate_in_place() {
    let mut column = ColumnStorage::from(vec![1_u32, 2, 3]);
    // Slice-style mutation via DerefMut.
    if let Some(slot) = column.get_mut(1) {
        *slot = 99;
    }
    assert_eq!(column.as_slice(), &[1_u32, 99, 3]);
}

#[test]
fn as_mut_vec_supports_vec_specific_methods() {
    let mut column: ColumnStorage<u8> = ColumnStorage::default();
    column.as_mut_vec().push(7);
    column.as_mut_vec().extend_from_slice(&[8, 9, 10]);
    assert_eq!(column.as_slice(), &[7, 8, 9, 10]);
    column.as_mut_vec().shrink_to_fit();
    // shrink_to_fit may match capacity to len; either way, len stays.
    assert_eq!(column.len(), 4);
}

#[test]
fn capacity_tracks_underlying_vec() {
    let mut buf: Vec<u32> = Vec::with_capacity(16);
    buf.push(1);
    let column = ColumnStorage::from_vec(buf);
    assert!(
        column.capacity() >= 16,
        "capacity must reflect underlying Vec"
    );
    assert_eq!(column.len(), 1);
}

#[test]
fn clone_always_produces_a_vec_variant() {
    let original = ColumnStorage::from(vec![1_u32, 2, 3]);
    let mut copy = original.clone();
    assert_eq!(copy.as_slice(), original.as_slice());
    // Mutate the clone — original must remain unchanged.
    copy.as_mut_vec().push(4);
    assert_eq!(original.as_slice(), &[1_u32, 2, 3]);
    assert_eq!(copy.as_slice(), &[1_u32, 2, 3, 4]);
}

#[test]
fn debug_format_shows_inner_slice() {
    let column = ColumnStorage::from(vec![1_u8, 2, 3]);
    let printed = format!("{column:?}");
    assert!(printed.contains("ColumnStorage"), "got: {printed}");
    assert!(printed.contains('1'), "got: {printed}");
}

// ─────────────────────────────────────────────────────────────────────
// Phase 2b — `Mmap` variant tests.
// ─────────────────────────────────────────────────────────────────────

/// Build an `Arc<Mmap>` over an anonymous mmap pre-populated with
/// `bytes`.
///
/// Anonymous mmap (`MmapMut::map_anon`) is the safe path for tests:
/// no backing file means no file-truncation race, so the
/// `unsafe { Mmap::map(file) }` precondition that disqualifies
/// file-backed mmap construction in `#![forbid(unsafe_code)]`
/// crates does not apply.  For the read-side invariants this
/// helper exercises — `as_ref()` slice contents, page-aligned base
/// address, `cast_slice` round-trips — anonymous and file-backed
/// mmaps are observationally identical.  Production code
/// constructs file-backed `Arc<Mmap>` via
/// `uffs-security::runtime_dir`, where the unsafe block lives
/// alongside the rest of the FFI surface.
fn make_mmap_for_test(bytes: &[u8]) -> Arc<Mmap> {
    let mut writable = memmap2::MmapMut::map_anon(bytes.len()).expect("map_anon for test fixture");
    writable.copy_from_slice(bytes);
    let mmap = writable.make_read_only().expect("make_read_only");
    Arc::new(mmap)
}

#[test]
fn mmap_variant_round_trips_through_into_vec() {
    let original = vec![10_u32, 20, 30, 40, 50];
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let column: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    assert_eq!(column.len(), 5);
    assert!(!column.is_empty());
    assert_eq!(column.as_slice(), original.as_slice());
    let recovered = column.into_vec();
    assert_eq!(recovered, original);
}

#[test]
fn mmap_variant_deref_lets_call_sites_use_slice_methods() {
    let original = vec![1_u32, 2, 3];
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let column: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    // Read-side path — every call dispatches via Deref into
    // `&[T]`, which is fed by the mmap variant of `as_slice`.
    assert_eq!(column.first(), Some(&1));
    assert_eq!(column.last(), Some(&3));
    assert_eq!(column.iter().sum::<u32>(), 6);
    assert_eq!(column.get(1), Some(&2));
}

#[test]
fn as_mut_vec_promotes_mmap_to_heap() {
    let original = vec![100_u32, 200, 300];
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let mut column: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(Arc::clone(&mmap), 0, original.len())
            .expect("valid region");
    assert!(matches!(column, ColumnStorage::Mmap { .. }));
    // First mutation triggers the promotion.
    column.as_mut_vec().push(400);
    assert!(matches!(column, ColumnStorage::Vec(_)));
    assert_eq!(column.as_slice(), &[100_u32, 200, 300, 400]);
    // The mmap is still alive (we hold an external reference) and
    // is now decoupled from the column — mutating the column did
    // not corrupt the underlying mmap bytes.
    let mmap_view = mmap.get(..bytes.len()).expect("mmap covers fixture range");
    assert_eq!(
        bytemuck::cast_slice::<u8, u32>(mmap_view),
        original.as_slice(),
    );
}

#[test]
fn as_mut_slice_promotes_and_writes_in_place() {
    let original = vec![7_u8, 8, 9, 10];
    let mmap = make_mmap_for_test(&original);
    let mut column: ColumnStorage<u8> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    if let Some(slot) = column.as_mut_slice().get_mut(2) {
        *slot = 99;
    }
    assert_eq!(column.as_slice(), &[7_u8, 8, 99, 10]);
    assert!(matches!(column, ColumnStorage::Vec(_)));
}

#[test]
fn mmap_variant_len_capacity_and_is_empty() {
    let original = vec![1_u32, 2, 3, 4];
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let column: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    assert_eq!(column.len(), 4);
    assert!(!column.is_empty());
    // Mmap variant: capacity == len (pages have no slack).
    assert_eq!(column.capacity(), 4);
}

#[test]
fn from_mmap_region_rejects_out_of_bounds() {
    let bytes = vec![0_u8; 16];
    let mmap = make_mmap_for_test(&bytes);
    // 5 u32s = 20 bytes; mmap is 16.
    let err = ColumnStorage::<u32>::from_mmap_region(mmap, 0, 5)
        .expect_err("OOB region must be rejected");
    match err {
        MmapRegionError::OutOfBounds {
            start,
            end,
            mmap_len,
        } => {
            assert_eq!(start, 0);
            assert_eq!(end, 20);
            assert_eq!(mmap_len, 16);
        }
        other @ (MmapRegionError::Overflow | MmapRegionError::Misaligned { .. }) => {
            panic!("expected OutOfBounds, got {other:?}")
        }
    }
}

#[test]
fn from_mmap_region_rejects_misalignment() {
    // Build a buffer big enough that we have a misaligned region
    // available.  A 1-byte offset against `u32` forces alignment 4
    // → actual_offset 1 regardless of the page base, since
    // anonymous-mmap base addresses are page-aligned (≥ 4096).
    let bytes = vec![0_u8; 32];
    let mmap = make_mmap_for_test(&bytes);
    let err = ColumnStorage::<u32>::from_mmap_region(mmap, 1, 4)
        .expect_err("misaligned region must be rejected");
    match err {
        MmapRegionError::Misaligned {
            required,
            actual_offset,
        } => {
            assert_eq!(required, align_of::<u32>());
            assert!(
                (1..required).contains(&actual_offset),
                "actual_offset {actual_offset} should be in 1..{required}"
            );
        }
        other @ (MmapRegionError::Overflow | MmapRegionError::OutOfBounds { .. }) => {
            panic!("expected Misaligned, got {other:?}")
        }
    }
}

#[test]
fn cloning_an_mmap_variant_produces_a_vec_variant() {
    let original = vec![5_u32, 10, 15];
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let column: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    assert!(matches!(column, ColumnStorage::Mmap { .. }));
    let cloned = column.clone();
    // Clone always allocates — invariant required by the Clone impl
    // doc-comment so promote-on-mutation logic stays single-owner.
    assert!(matches!(cloned, ColumnStorage::Vec(_)));
    assert_eq!(cloned.as_slice(), original.as_slice());
    // Original must remain mmap-backed and unchanged — cloning is
    // pure (no side effects on the source).
    assert!(matches!(column, ColumnStorage::Mmap { .. }));
    assert_eq!(column.as_slice(), original.as_slice());
}

#[test]
fn debug_format_distinguishes_variants() {
    let original = vec![42_u32];
    let heap = ColumnStorage::from_vec(original.clone());
    let bytes = bytemuck::cast_slice::<u32, u8>(&original);
    let mmap = make_mmap_for_test(bytes);
    let mmap_col: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(mmap, 0, original.len()).expect("valid region");
    let heap_dbg = format!("{heap:?}");
    let mmap_dbg = format!("{mmap_col:?}");
    assert!(heap_dbg.contains("variant: \"Vec\""), "got: {heap_dbg}");
    assert!(mmap_dbg.contains("variant: \"Mmap\""), "got: {mmap_dbg}");
    // Both render their data section.
    assert!(heap_dbg.contains("42"), "got: {heap_dbg}");
    assert!(mmap_dbg.contains("42"), "got: {mmap_dbg}");
}

#[test]
fn multiple_columns_can_share_one_arc_mmap() {
    // Production callers (`compact_mmap::load_from_runtime`) build
    // *several* columns over disjoint byte ranges of the same
    // tempfile.  Verify the `Arc<Mmap>` is reference-counted: both
    // columns can read independently, and dropping one does not
    // invalidate the other.
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(bytemuck::cast_slice(&[1_u32, 2, 3, 4]));
    bytes.extend_from_slice(bytemuck::cast_slice(&[10_u32, 20, 30, 40]));
    let mmap = make_mmap_for_test(&bytes);
    let col_a: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(Arc::clone(&mmap), 0, 4).expect("valid region a");
    let col_b: ColumnStorage<u32> =
        ColumnStorage::from_mmap_region(Arc::clone(&mmap), 16, 4).expect("valid region b");
    assert_eq!(col_a.as_slice(), &[1_u32, 2, 3, 4]);
    assert_eq!(col_b.as_slice(), &[10_u32, 20, 30, 40]);
    drop(col_a);
    // col_b's reads still work after col_a is gone.
    assert_eq!(col_b.as_slice(), &[10_u32, 20, 30, 40]);
}
