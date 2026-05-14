// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Core type, basic index, and name-buffer tests for the split `index` module.

use core::mem::size_of;

use super::*;

#[test]
fn standard_info_flags() {
    let mut info = StandardInfo::default();
    assert!(!info.is_directory());

    info.set_directory(true);
    assert!(info.is_directory());

    info.set_directory(false);
    assert!(!info.is_directory());
}

#[test]
fn file_record_size() {
    // Verify compact size - should be reasonably compact (<= 240 bytes)
    // Version 4 added: sequence_number (2), namespace (1), reserved (1),
    // fn_created/modified/accessed/mft_changed (4 × 8 = 32) = 36 bytes extra
    // Version 5 added: lsn (8 bytes) for forensic correlation
    // Version 6 added: reparse_tag (4 bytes)
    // Version 7 added: base_frs (8 bytes) for forensic extension records
    // Version 8 added: total_stream_count (2 bytes, with padding = 4 bytes)
    //                  internal_streams_size (8 bytes)
    //                  internal_streams_allocated (8 bytes)
    //                  = 20 bytes extra for full tree-metrics accounting
    let size = size_of::<FileRecord>();
    assert!(size <= 240, "FileRecord too large: {size} bytes");
}

#[test]
fn index_basic_operations() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add a record
    let record = index.get_or_create(100);
    record.stdinfo.set_directory(true);

    // Find it
    let found = index.find(100);
    assert!(found.is_some());
    assert!(found.unwrap().is_directory());

    // Not found
    assert!(index.find(999).is_none());
}

#[test]
fn index_name_ref_size() {
    // Verify IndexNameRef is exactly 8 bytes (no padding)
    assert_eq!(size_of::<IndexNameRef>(), 8);
}

#[test]
fn index_name_ref_bit_packing() {
    // Test bit-packing correctness
    let name_ref = IndexNameRef::new(100, 255, true, 1234);

    assert_eq!(name_ref.offset, 100);
    assert_eq!(name_ref.length(), 255);
    assert!(name_ref.is_ascii());
    assert_eq!(name_ref.extension_id(), 1234);

    // Test max values
    let max_ref = IndexNameRef::new(u32::MAX, 1023, false, 65535);
    assert_eq!(max_ref.length(), 1023); // Max 10 bits
    assert!(!max_ref.is_ascii());
    assert_eq!(max_ref.extension_id(), 65535); // Max 16 bits
}

#[test]
fn names_buffer() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    let offset1 = index.add_name("test.txt");
    let offset2 = index.add_name("hello.rs");

    let info1 = IndexNameRef::new(offset1, 8, true, IndexNameRef::NO_EXTENSION);
    let info2 = IndexNameRef::new(offset2, 8, true, IndexNameRef::NO_EXTENSION);

    assert_eq!(index.get_name(info1), "test.txt");
    assert_eq!(index.get_name(info2), "hello.rs");
}

#[test]
fn cmp_ascii_case_insensitive_works() {
    use core::cmp::Ordering;

    // Equal strings (different case)
    assert_eq!(
        cmp_ascii_case_insensitive("hello", "HELLO"),
        Ordering::Equal
    );
    assert_eq!(cmp_ascii_case_insensitive("Test", "test"), Ordering::Equal);
    assert_eq!(cmp_ascii_case_insensitive("ABC", "abc"), Ordering::Equal);

    // Less than
    assert_eq!(cmp_ascii_case_insensitive("abc", "def"), Ordering::Less);
    assert_eq!(cmp_ascii_case_insensitive("file1", "file2"), Ordering::Less);
    assert_eq!(cmp_ascii_case_insensitive("AAA", "bbb"), Ordering::Less);

    // Greater than
    assert_eq!(cmp_ascii_case_insensitive("xyz", "abc"), Ordering::Greater);
    assert_eq!(
        cmp_ascii_case_insensitive("file2", "file1"),
        Ordering::Greater
    );
    assert_eq!(cmp_ascii_case_insensitive("ZZZ", "aaa"), Ordering::Greater);

    // Empty strings
    assert_eq!(cmp_ascii_case_insensitive("", ""), Ordering::Equal);
    assert_eq!(cmp_ascii_case_insensitive("", "a"), Ordering::Less);
    assert_eq!(cmp_ascii_case_insensitive("a", ""), Ordering::Greater);

    // Different lengths
    assert_eq!(
        cmp_ascii_case_insensitive("test", "testing"),
        Ordering::Less
    );
    assert_eq!(
        cmp_ascii_case_insensitive("testing", "test"),
        Ordering::Greater
    );
}
