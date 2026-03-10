use super::*;
use crate::ntfs::{ExtendedStandardInfo, FILE_RECORD_MAGIC, NameInfo};

/// Create a minimal valid MFT record header for testing.
/// This creates a 1024-byte record with proper fixup values.
fn create_test_record(frs: u64, in_use: bool, is_dir: bool) -> Vec<u8> {
    let mut data = vec![0_u8; 1024];

    // Magic: "FILE"
    data[0..4].copy_from_slice(&FILE_RECORD_MAGIC.to_le_bytes());

    // USA offset (0x30 is typical)
    data[4..6].copy_from_slice(&0x30_u16.to_le_bytes());

    // USA count (3 for 1024-byte record: check + 2 sectors)
    data[6..8].copy_from_slice(&3_u16.to_le_bytes());

    // LSN (Log Sequence Number)
    data[8..16].copy_from_slice(&12345_u64.to_le_bytes());

    // Sequence number
    data[16..18].copy_from_slice(&1_u16.to_le_bytes());

    // Hard link count
    data[18..20].copy_from_slice(&1_u16.to_le_bytes());

    // First attribute offset (after header, 0x38 typical)
    data[20..22].copy_from_slice(&0x38_u16.to_le_bytes());

    // Flags: 0x01 = in use, 0x02 = directory
    let flags: u16 = u16::from(in_use) | (u16::from(is_dir) << 1);
    data[22..24].copy_from_slice(&flags.to_le_bytes());

    // Used size of record
    data[24..28].copy_from_slice(&0x100_u32.to_le_bytes());

    // Allocated size of record
    data[28..32].copy_from_slice(&0x400_u32.to_le_bytes());

    // Base record reference (0 for base records)
    data[32..40].copy_from_slice(&0_u64.to_le_bytes());

    // Next attribute ID
    data[40..42].copy_from_slice(&1_u16.to_le_bytes());

    // FRS number (at offset 44 in modern NTFS) - truncate to u32 for test data
    #[expect(
        clippy::cast_possible_truncation,
        reason = "test FRS values are small constants"
    )]
    let frs_u32 = frs as u32;
    data[44..48].copy_from_slice(&frs_u32.to_le_bytes());

    // USA: check value at offset 0x30
    let check_value: u16 = 0xABCD;
    data[0x30..0x32].copy_from_slice(&check_value.to_le_bytes());

    // USA entry 1 (original bytes from sector 1 end)
    data[0x32..0x34].copy_from_slice(&0x1234_u16.to_le_bytes());

    // USA entry 2 (original bytes from sector 2 end)
    data[0x34..0x36].copy_from_slice(&0x5678_u16.to_le_bytes());

    // Place check value at sector boundaries (will be replaced by fixup)
    data[510..512].copy_from_slice(&check_value.to_le_bytes());
    data[1022..1024].copy_from_slice(&check_value.to_le_bytes());

    // End marker attribute (type 0xFFFFFFFF)
    data[0x38..0x3C].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());

    data
}

#[test]
fn test_apply_fixup_valid_record() {
    let mut data = create_test_record(5, true, false);

    // Before fixup, sector ends have check value
    assert_eq!(&data[510..512], &0xABCD_u16.to_le_bytes());
    assert_eq!(&data[1022..1024], &0xABCD_u16.to_le_bytes());

    let result = apply_fixup(&mut data);
    assert!(result, "Fixup should succeed for valid record");

    // After fixup, sector ends have original values from USA
    assert_eq!(&data[510..512], &0x1234_u16.to_le_bytes());
    assert_eq!(&data[1022..1024], &0x5678_u16.to_le_bytes());
}

#[test]
fn test_apply_fixup_invalid_magic() {
    let mut data = vec![0_u8; 1024];
    data[0..4].copy_from_slice(b"BAAD"); // Invalid magic

    let result = apply_fixup(&mut data);
    assert!(!result, "Fixup should fail for invalid magic");
}

#[test]
fn test_apply_fixup_buffer_too_small() {
    let mut data = vec![0_u8; 10]; // Too small for header

    let result = apply_fixup(&mut data);
    assert!(!result, "Fixup should fail for buffer too small");
}

#[test]
fn test_apply_fixup_corrupted_check_value() {
    let mut data = create_test_record(5, true, false);

    // Corrupt the check value at first sector end
    data[510..512].copy_from_slice(&0xDEAD_u16.to_le_bytes());

    let result = apply_fixup(&mut data);
    assert!(!result, "Fixup should fail for corrupted check value");
}

#[test]
fn test_create_placeholder_record() {
    let record = create_placeholder_record(12345);

    assert_eq!(record.frs, 12345);
    assert_eq!(record.parent_frs, 5); // Root directory
    assert_eq!(record.name, "<dir:12345>");
    assert!(record.is_directory);
    assert!(record.in_use);
    assert!(record.names.is_empty());
    assert!(record.streams.is_empty());
}

#[test]
fn test_parse_result_variants() {
    // Test ParseResult enum
    let base = ParseResult::Base(create_placeholder_record(1));
    assert!(matches!(base, ParseResult::Base(_)));

    let ext = ParseResult::Extension(ExtensionAttributes {
        base_frs: 100,
        extension_frs: 101,
        names: Vec::new(),
        streams: Vec::new(),
        dir_index_size: 0,
        dir_index_allocated: 0,
    });
    assert!(matches!(ext, ParseResult::Extension(_)));

    let skip = ParseResult::Skip;
    assert!(matches!(skip, ParseResult::Skip));
}

#[test]
fn test_parse_options_default() {
    let opts = ParseOptions::default();
    assert!(!opts.include_deleted);
    assert!(!opts.include_corrupt);
    assert!(!opts.include_extensions);
}

#[test]
fn test_parse_options_forensic() {
    let opts = ParseOptions::FORENSIC;
    assert!(opts.include_deleted);
    assert!(opts.include_corrupt);
    assert!(opts.include_extensions);
    assert!(opts.is_forensic());
}

#[test]
fn test_parsed_record_default() {
    let record = ParsedRecord::default();
    assert_eq!(record.frs, 0);
    assert_eq!(record.sequence_number, 0);
    assert_eq!(record.parent_frs, 0);
    assert!(record.name.is_empty());
    assert!(!record.in_use);
    assert!(!record.is_directory);
}

#[test]
fn test_add_missing_parent_placeholders_empty() {
    let mut records: Vec<ParsedRecord> = Vec::new();
    let added = add_missing_parent_placeholders_to_vec(&mut records);
    assert_eq!(added, 0);
}

#[test]
fn test_add_missing_parent_placeholders_no_missing() {
    let mut records = vec![
        {
            let mut r = create_placeholder_record(5);
            r.parent_frs = 5; // Root references itself
            r
        },
        {
            let mut r = create_placeholder_record(100);
            r.parent_frs = 5; // References root
            r
        },
    ];

    let added = add_missing_parent_placeholders_to_vec(&mut records);
    assert_eq!(added, 0, "No placeholders needed when all parents exist");
}

#[test]
fn test_add_missing_parent_placeholders_with_missing() {
    let mut records = vec![{
        let mut r = create_placeholder_record(100);
        r.parent_frs = 50; // References non-existent parent
        r
    }];

    let added = add_missing_parent_placeholders_to_vec(&mut records);
    assert!(added >= 1, "Should add placeholder for missing parent 50");

    // Verify placeholder was added
    let has_50 = records.iter().any(|r| r.frs == 50);
    assert!(has_50, "Placeholder for FRS 50 should exist");
}

// ========================================================================
// Property-Based Tests
// ========================================================================

mod proptest_tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// apply_fixup should never panic regardless of input
        #[test]
        fn apply_fixup_never_panics(mut data in prop::collection::vec(any::<u8>(), 0..2048)) {
            // Should return true or false, never panic
            // For random data, fixup usually fails (returns false) because
            // the data doesn't have valid MFT record structure
            let result = apply_fixup(&mut data);
            // Use black_box to prevent optimization and ensure result is used
            core::hint::black_box(result);
        }

        /// create_placeholder_record should always produce valid records
        #[test]
        fn placeholder_record_always_valid(frs in 0_u64..1_000_000) {
            let record = create_placeholder_record(frs);
            prop_assert_eq!(record.frs, frs);
            prop_assert!(record.is_directory);
            prop_assert!(record.in_use);
            prop_assert_eq!(record.parent_frs, 5); // Always root
        }

        /// ParseOptions should have consistent is_forensic behavior
        #[test]
        fn parse_options_forensic_consistency(
            include_deleted in any::<bool>(),
            include_corrupt in any::<bool>(),
            include_extensions in any::<bool>()
        ) {
            let opts = ParseOptions {
                include_deleted,
                include_corrupt,
                include_extensions,
            };
            let expected = include_deleted || include_corrupt || include_extensions;
            prop_assert_eq!(opts.is_forensic(), expected);
        }

        /// parse_record should handle any buffer without panicking
        #[test]
        fn parse_record_never_panics(
            data in prop::collection::vec(any::<u8>(), 0..4096),
            frs in 0_u64..1_000_000
        ) {
            // Should return Some or None, never panic
            let result = parse_record(&data, frs);
            // Result is valid (Some or None)
            prop_assert!(result.is_some() || result.is_none());
        }

        /// parse_record_full should handle any buffer without panicking
        #[test]
        fn parse_record_full_never_panics(
            data in prop::collection::vec(any::<u8>(), 0..4096),
            frs in 0_u64..1_000_000
        ) {
            // Should return a ParseResult variant, never panic
            let result = parse_record_full(&data, frs);
            // Result is valid (one of the variants: Base, Extension, or Skip)
            prop_assert!(matches!(result, ParseResult::Base(_) | ParseResult::Extension(_) | ParseResult::Skip));
        }
    }
}

/// Test that extension records with `$FILE_NAME` are properly merged into
/// base records that have no `$FILE_NAME` attribute.
#[test]
fn test_extension_merge_with_empty_base_name() {
    // Simulate the case where base record has no $FILE_NAME
    // and extension record has the $FILE_NAME

    let mut record_merger = MftRecordMerger::with_capacity(10);

    // Add base record with empty name
    let base = ParsedRecord {
        frs: 100,
        sequence_number: 1,
        lsn: 0,
        parent_frs: 0,       // Wrong - should be updated from extension
        name: String::new(), // Empty - should be updated from extension
        namespace: 255,      // Invalid
        names: Vec::new(),   // No names in base record
        streams: Vec::new(),
        size: 0,
        allocated_size: 0,
        std_info: ExtendedStandardInfo::default(),
        in_use: true,
        is_directory: true,
        fn_created: 0,
        fn_modified: 0,
        fn_accessed: 0,
        fn_mft_changed: 0,
        reparse_tag: 0,
        is_deleted: false,
        is_corrupt: false,
        is_extension: false,
        base_frs: 0,
    };
    record_merger.add_result(ParseResult::Base(base));

    // Add extension record with the actual name
    let ext = ExtensionAttributes {
        base_frs: 100,
        extension_frs: 200,
        names: vec![NameInfo {
            name: "test_directory".to_owned(),
            parent_frs: 5, // Root
            namespace: 1,  // Win32
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
            source_frs: 200,
        }],
        streams: Vec::new(),
        dir_index_size: 0,
        dir_index_allocated: 0,
    };
    record_merger.add_result(ParseResult::Extension(ext));

    // Merge
    let result = record_merger.merge();

    // Check that the base record now has the name from extension
    assert_eq!(result.len(), 1, "Should have exactly 1 merged record");
    let rec = &result[0];
    assert_eq!(rec.frs, 100);
    assert_eq!(
        rec.name, "test_directory",
        "Name should be merged from extension"
    );
    assert_eq!(
        rec.parent_frs, 5,
        "parent_frs should be merged from extension"
    );
    assert_eq!(
        rec.namespace, 1,
        "namespace should be merged from extension"
    );
}

/// Test that extension records are merged even when processed before base
/// record
#[test]
fn test_extension_before_base_merge() {
    let mut record_merger = MftRecordMerger::with_capacity(10);

    // Add extension record FIRST (before base record)
    let ext = ExtensionAttributes {
        base_frs: 100,
        extension_frs: 200,
        names: vec![NameInfo {
            name: "test_directory".to_owned(),
            parent_frs: 5,
            namespace: 1,
            fn_created: 0,
            fn_modified: 0,
            fn_accessed: 0,
            fn_mft_changed: 0,
            source_frs: 200,
        }],
        streams: Vec::new(),
        dir_index_size: 0,
        dir_index_allocated: 0,
    };
    record_merger.add_result(ParseResult::Extension(ext));

    // Add base record AFTER extension
    let base = ParsedRecord {
        frs: 100,
        sequence_number: 1,
        lsn: 0,
        parent_frs: 0,
        name: String::new(),
        namespace: 255,
        names: Vec::new(),
        streams: Vec::new(),
        size: 0,
        allocated_size: 0,
        std_info: ExtendedStandardInfo::default(),
        in_use: true,
        is_directory: true,
        fn_created: 0,
        fn_modified: 0,
        fn_accessed: 0,
        fn_mft_changed: 0,
        reparse_tag: 0,
        is_deleted: false,
        is_corrupt: false,
        is_extension: false,
        base_frs: 0,
    };
    record_merger.add_result(ParseResult::Base(base));

    // Merge
    let result = record_merger.merge();

    // Check that the base record now has the name from extension
    assert_eq!(result.len(), 1, "Should have exactly 1 merged record");
    let rec = &result[0];
    assert_eq!(rec.frs, 100);
    assert_eq!(
        rec.name, "test_directory",
        "Name should be merged from extension"
    );
    assert_eq!(
        rec.parent_frs, 5,
        "parent_frs should be merged from extension"
    );
}
