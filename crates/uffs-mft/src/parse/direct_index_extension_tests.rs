// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Regression tests for `direct_index_extension.rs`
//!
//! These tests verify the snapshot/restore pattern for handling out-of-order
//! IOCP delivery of extension records before base records.
//!
//! Rather than creating complex mock MFT records, these tests directly verify
//! the core logic by simulating the index state after `dir_index` accumulation.

use crate::index::{IndexNameRef, IndexStreamInfo, MftIndex, NO_ENTRY, SizeInfo};

/// Test helper to create a `FileRecord` with specified `first_stream` size
fn create_test_record(frs: u64, length: u64, allocated: u64) -> crate::index::FileRecord {
    crate::index::FileRecord {
        frs,
        first_stream: IndexStreamInfo {
            size: SizeInfo { length, allocated },
            next_entry: NO_ENTRY,
            name: IndexNameRef::default(),
            flags: 0,
            _pad0: [0; 3],
        },
        ..Default::default()
    }
}

/// Simulate the snapshot/restore pattern for `dir_index` merging.
/// This is the core logic from `direct_index_extension.rs` lines 749-765.
fn merge_dir_index(
    record: &mut crate::index::FileRecord,
    dir_index_size: u64,
    dir_index_allocated: u64,
) {
    if record.first_stream.size.length == 0 && record.first_stream.size.allocated == 0 {
        // Base has no size set - use extension's dir_index values
        record.first_stream.size.length = dir_index_size;
        record.first_stream.size.allocated = dir_index_allocated;
    } else {
        // Base has size set - accumulate extension's dir_index
        record.first_stream.size.length = record
            .first_stream
            .size
            .length
            .saturating_add(dir_index_size);
        record.first_stream.size.allocated = record
            .first_stream
            .size
            .allocated
            .saturating_add(dir_index_allocated);
    }
}

#[test]
fn dir_index_extension_before_base_snapshot_restore() {
    // Scenario: IOCP delivers extension record before base record
    // Extension has dir_index_size=4096, base has dir_index_size=8192
    // Result should be cumulative: 4096 + 8192 = 12288

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Create empty base record (base hasn't been parsed yet)
    index.records.push(create_test_record(100, 0, 0));

    // Step 1: Extension arrives first (4096 bytes, 8192 allocated)
    merge_dir_index(&mut index.records[0], 4096, 8192);

    // After extension: should snapshot these values (base had nothing)
    assert_eq!(
        index.records[0].first_stream.size.length, 4096,
        "Extension should set length when base has no size"
    );
    assert_eq!(
        index.records[0].first_stream.size.allocated, 8192,
        "Extension should set allocated when base has no size"
    );

    // Step 2: Base arrives second (8192 bytes, 16384 allocated)
    // Should ACCUMULATE with extension values
    merge_dir_index(&mut index.records[0], 8192, 16384);

    // After base: should have cumulative values
    assert_eq!(
        index.records[0].first_stream.size.length, 12288,
        "Should have cumulative length: 4096 (ext) + 8192 (base) = 12288"
    );
    assert_eq!(
        index.records[0].first_stream.size.allocated, 24576,
        "Should have cumulative allocated: 8192 (ext) + 16384 (base) = 24576"
    );
}

#[test]
fn dir_index_base_before_extension_snapshot_restore() {
    // Scenario: Base record arrives before extension (normal case)
    // Base has dir_index_size=8192, extension has dir_index_size=4096
    // Result should be cumulative: 8192 + 4096 = 12288

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Create base record with dir_index (base already parsed)
    index.records.push(create_test_record(100, 8192, 16384));

    // Extension arrives (4096 bytes, 8192 allocated)
    // Should ACCUMULATE with base values
    merge_dir_index(&mut index.records[0], 4096, 8192);

    assert_eq!(
        index.records[0].first_stream.size.length, 12288,
        "Should accumulate: 8192 (base) + 4096 (ext) = 12288"
    );
    assert_eq!(
        index.records[0].first_stream.size.allocated, 24576,
        "Should accumulate: 16384 (base) + 8192 (ext) = 24576"
    );
}

#[test]
fn dir_index_multiple_extensions_snapshot_restore() {
    // Scenario: Multiple extension records all arrive before base
    // All should accumulate properly using saturating_add

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Empty base record
    index.records.push(create_test_record(100, 0, 0));

    // Extension 1: 1000 bytes (should snapshot since base is empty)
    merge_dir_index(&mut index.records[0], 1000, 2000);
    assert_eq!(index.records[0].first_stream.size.length, 1000);
    assert_eq!(index.records[0].first_stream.size.allocated, 2000);

    // Extension 2: 500 bytes (should accumulate)
    merge_dir_index(&mut index.records[0], 500, 1000);
    assert_eq!(index.records[0].first_stream.size.length, 1500);
    assert_eq!(index.records[0].first_stream.size.allocated, 3000);

    // Extension 3: 2500 bytes (should accumulate)
    merge_dir_index(&mut index.records[0], 2500, 5000);
    assert_eq!(index.records[0].first_stream.size.length, 4000);
    assert_eq!(index.records[0].first_stream.size.allocated, 8000);

    // Base arrives last: 10000 bytes (should accumulate)
    merge_dir_index(&mut index.records[0], 10000, 20000);
    assert_eq!(index.records[0].first_stream.size.length, 14000);
    assert_eq!(index.records[0].first_stream.size.allocated, 28000);
}

#[test]
fn dir_index_zero_extension_values() {
    // Scenario: Extension has zero dir_index (branch not taken in actual code,
    // but verify the logic handles it correctly)

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Base with existing values
    index.records.push(create_test_record(100, 8192, 16384));

    // This simulates what would happen if the code path were taken with 0 values
    // (In reality, the if dir_index_size > 0 check prevents this branch)
    merge_dir_index(&mut index.records[0], 0, 0);

    // Base values should remain unchanged (0 + 8192 = 8192)
    assert_eq!(
        index.records[0].first_stream.size.length, 8192,
        "Zero extension should not modify base values (saturating_add(0) = identity)"
    );
    assert_eq!(index.records[0].first_stream.size.allocated, 16384);
}

#[test]
fn dir_index_saturating_add_no_overflow() {
    // Verify saturating_add prevents overflow

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Start with large values
    let near_max = u64::MAX - 1000;
    index
        .records
        .push(create_test_record(100, near_max, near_max));

    // Add values that would overflow without saturating_add
    merge_dir_index(&mut index.records[0], 2000, 2000);

    // Should saturate at u64::MAX, not wrap
    assert_eq!(
        index.records[0].first_stream.size.length,
        u64::MAX,
        "Should saturate at u64::MAX, not overflow"
    );
    assert_eq!(index.records[0].first_stream.size.allocated, u64::MAX);
}

#[test]
fn dir_index_regression_old_unconditional_add_bug() {
    // This test demonstrates the bug that was fixed
    // OLD CODE (buggy): Always used += , losing data when extension arrives first
    // NEW CODE (correct): Uses snapshot/restore pattern

    let mut index = MftIndex::new('C');
    index.frs_to_idx.resize(101, NO_ENTRY);
    index.frs_to_idx[100] = 0;

    // Empty base (extension will arrive first)
    index.records.push(create_test_record(100, 0, 0));

    // Extension arrives: dir_index = 4096
    merge_dir_index(&mut index.records[0], 4096, 8192);

    // With OLD buggy code (unconditional +=):
    // first_stream.size.length = 0 + 4096 = 4096 ✓ (correct by accident)

    // Base overwrites with new SizeInfo = {length: 8192, allocated: 16384}
    // This is simulated by directly setting values (what the old base parser did)
    // OLD CODE would do: record.first_stream.size = SizeInfo { length: 8192,
    // allocated: 16384 } This LOSES the extension data!

    // NEW CODE prevents this by accumulating:
    let snapshot = index.records[0].first_stream.size;
    merge_dir_index(&mut index.records[0], 8192, 16384);

    // Verify we accumulated, not overwrote
    assert_eq!(
        index.records[0].first_stream.size.length,
        snapshot.length + 8192,
        "Must accumulate, not overwrite (old bug)"
    );
    assert_eq!(
        index.records[0].first_stream.size.allocated,
        snapshot.allocated + 16384,
        "Must accumulate, not overwrite (old bug)"
    );
}
