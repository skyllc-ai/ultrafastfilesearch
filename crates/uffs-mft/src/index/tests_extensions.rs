// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Extension table, extension index, and related stats tests.

use super::*;

#[test]
fn extension_table_interning() {
    let mut table = ExtensionTable::new();

    // Test empty extension
    assert_eq!(table.intern(""), 0);
    assert_eq!(table.intern("."), 0);

    // Test basic interning
    let txt_id = table.intern("txt");
    assert_eq!(txt_id, 1); // First real extension gets ID 1

    // Test normalization (lowercase, no dot)
    let txt_id2 = table.intern(".TXT");
    assert_eq!(txt_id2, txt_id); // Should return same ID

    let txt_id3 = table.intern("TxT");
    assert_eq!(txt_id3, txt_id); // Should return same ID

    // Test different extension
    let rs_id = table.intern("rs");
    assert_eq!(rs_id, 2);

    // Verify lookups
    assert_eq!(table.get_extension(0), Some(""));
    assert_eq!(table.get_extension(txt_id), Some("txt"));
    assert_eq!(table.get_extension(rs_id), Some("rs"));

    // Test counts
    assert_eq!(table.len(), 3); // "", "txt", "rs"
}

#[test]
fn extension_table_record_file() {
    let mut table = ExtensionTable::new();

    let txt_id = table.intern("txt");
    let rs_id = table.intern("rs");

    // Record some files
    table.record_file(txt_id, 1024);
    table.record_file(txt_id, 2048);
    table.record_file(rs_id, 512);

    // Verify counts and bytes
    assert_eq!(table.get_count(txt_id), 2);
    assert_eq!(table.get_bytes(txt_id), 3072);

    assert_eq!(table.get_count(rs_id), 1);
    assert_eq!(table.get_bytes(rs_id), 512);

    assert_eq!(table.get_count(0), 0); // No files without extension
    assert_eq!(table.get_bytes(0), 0);
}

#[test]
fn intern_extension() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Test basic extension extraction
    assert_eq!(index.intern_extension("test.txt"), 1);
    assert_eq!(index.intern_extension("hello.rs"), 2);

    // Test normalization (case-insensitive)
    assert_eq!(index.intern_extension("FILE.TXT"), 1); // Same as "txt"
    assert_eq!(index.intern_extension("main.RS"), 2); // Same as "rs"
}

#[test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
fn extension_table_serialization() {
    // Create an index with some extensions
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add names and extensions first (before getting mutable references to records)
    let name1_offset = index.add_name("test.txt");
    let ext_id1 = index.intern_extension("test.txt");
    index.extensions.record_file(ext_id1, 1024);

    let name2_offset = index.add_name("main.rs");
    let ext_id2 = index.intern_extension("main.rs");
    index.extensions.record_file(ext_id2, 2048);

    let name3_offset = index.add_name("another.txt");
    let ext_id3 = index.intern_extension("another.txt");
    index.extensions.record_file(ext_id3, 512);

    // Now create records and set their fields
    let record1 = index.get_or_create(100.into());
    record1.stdinfo.set_directory(false);
    record1.first_name.name = IndexNameRef::new(name1_offset, 8, true, ext_id1);

    let record2 = index.get_or_create(101.into());
    record2.stdinfo.set_directory(false);
    record2.first_name.name = IndexNameRef::new(name2_offset, 7, true, ext_id2);

    let record3 = index.get_or_create(102.into());
    record3.stdinfo.set_directory(false);
    record3.first_name.name = IndexNameRef::new(name3_offset, 11, true, ext_id3);

    // Serialize
    let serialized = index.serialize(12345, 67890, crate::usn::Usn::new(100));

    // Deserialize
    let (deserialized, header) =
        MftIndex::deserialize(&serialized).expect("Deserialization failed");

    // Verify header
    assert_eq!(header.volume, crate::platform::DriveLetter::C);
    assert_eq!(header.volume_serial, 12345);
    assert_eq!(header.usn_journal_id, 67890);
    assert_eq!(header.next_usn, crate::usn::Usn::new(100));

    // Verify extension table was preserved
    assert_eq!(deserialized.extensions.len(), index.extensions.len());

    // Verify extension strings
    assert_eq!(deserialized.extensions.get_extension(ext_id1), Some("txt"));
    assert_eq!(deserialized.extensions.get_extension(ext_id2), Some("rs"));

    // Verify counts and bytes
    assert_eq!(deserialized.extensions.get_count(ext_id1), 2); // test.txt + another.txt
    assert_eq!(deserialized.extensions.get_bytes(ext_id1), 1536); // 1024 + 512
    assert_eq!(deserialized.extensions.get_count(ext_id2), 1);
    assert_eq!(deserialized.extensions.get_bytes(ext_id2), 2048);

    // Verify records
    assert_eq!(deserialized.records.len(), 3);

    // Verify extension_id values in records
    assert_eq!(
        deserialized.records[0].first_name.name.extension_id(),
        ext_id1
    );
    assert_eq!(
        deserialized.records[1].first_name.name.extension_id(),
        ext_id2
    );
    assert_eq!(
        deserialized.records[2].first_name.name.extension_id(),
        ext_id3
    );
}

#[test]
fn extension_index_build() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add files with different extensions
    let name1 = "file1.txt";
    let name2 = "file2.txt";
    let name3 = "file3.rs";
    let name4 = "file4.rs";
    let name5 = "README"; // no extension

    let offset1 = index.add_name(name1);
    let offset2 = index.add_name(name2);
    let offset3 = index.add_name(name3);
    let offset4 = index.add_name(name4);
    let offset5 = index.add_name(name5);

    let ext_txt = index.intern_extension(name1);
    let ext_rs = index.intern_extension(name3);
    let ext_none = index.intern_extension(name5);

    // Create records
    let rec1 = index.get_or_create(100.into());
    rec1.first_name.name =
        IndexNameRef::new(offset1, u16::try_from(name1.len()).unwrap(), true, ext_txt);

    let rec2 = index.get_or_create(101.into());
    rec2.first_name.name =
        IndexNameRef::new(offset2, u16::try_from(name2.len()).unwrap(), true, ext_txt);

    let rec3 = index.get_or_create(102.into());
    rec3.first_name.name =
        IndexNameRef::new(offset3, u16::try_from(name3.len()).unwrap(), true, ext_rs);

    let rec4 = index.get_or_create(103.into());
    rec4.first_name.name =
        IndexNameRef::new(offset4, u16::try_from(name4.len()).unwrap(), true, ext_rs);

    let rec5 = index.get_or_create(104.into());
    rec5.first_name.name =
        IndexNameRef::new(offset5, u16::try_from(name5.len()).unwrap(), true, ext_none);

    // Build extension index
    index.build_extension_index();

    let ext_index = index
        .extension_index
        .as_ref()
        .expect("Extension index not built");

    // Verify txt files
    let txt_records = ext_index.get_records(ext_txt);
    assert_eq!(txt_records.len(), 2);
    assert!(txt_records.contains(&0)); // rec1
    assert!(txt_records.contains(&1)); // rec2

    // Verify rs files
    let rs_records = ext_index.get_records(ext_rs);
    assert_eq!(rs_records.len(), 2);
    assert!(rs_records.contains(&2)); // rec3
    assert!(rs_records.contains(&3)); // rec4

    // Verify no-extension files
    let none_records = ext_index.get_records(ext_none);
    assert_eq!(none_records.len(), 1);
    assert!(none_records.contains(&4)); // rec5

    // Verify counts
    assert_eq!(ext_index.count(ext_txt), 2);
    assert_eq!(ext_index.count(ext_rs), 2);
    assert_eq!(ext_index.count(ext_none), 1);

    // Verify total postings
    assert_eq!(ext_index.len(), 5);
}

#[test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
fn extension_index_with_hard_links() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a file with multiple hard links with different extensions
    let name1 = "file.txt";
    let name2 = "link.rs"; // hard link with different extension

    let offset1 = index.add_name(name1);
    let offset2 = index.add_name(name2);

    let ext_txt = index.intern_extension(name1);
    let ext_rs = index.intern_extension(name2);

    // Get link_idx before borrowing mutably
    let link_idx = u32::try_from(index.links.len()).unwrap();

    // Create record with primary name
    let rec = index.get_or_create(100.into());
    rec.first_name.name =
        IndexNameRef::new(offset1, u16::try_from(name1.len()).unwrap(), true, ext_txt);
    rec.name_count = 2;
    rec.first_name.next_entry = link_idx;

    // Add hard link
    index.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: IndexNameRef::new(offset2, u16::try_from(name2.len()).unwrap(), true, ext_rs),
        _pad0: [0; 4],
        parent_frs: Into::into(5_u64), // same parent
    });

    // Build extension index
    index.build_extension_index();

    let ext_index = index
        .extension_index
        .as_ref()
        .expect("Extension index not built");

    // Verify both extensions point to the same record
    let txt_records = ext_index.get_records(ext_txt);
    assert_eq!(txt_records.len(), 1);
    assert_eq!(txt_records[0], 0);

    let rs_records = ext_index.get_records(ext_rs);
    assert_eq!(rs_records.len(), 1);
    assert_eq!(rs_records[0], 0);

    // Total postings should be 2 (one record, two names)
    assert_eq!(ext_index.len(), 2);
}

#[test]
fn extension_index_empty() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Build on empty index
    index.build_extension_index();

    let ext_index = index
        .extension_index
        .as_ref()
        .expect("Extension index not built");

    // Should be empty
    assert!(ext_index.is_empty());
    assert_eq!(ext_index.len(), 0);

    // Query for any extension should return empty
    let records = ext_index.get_records(1);
    assert_eq!(records.len(), 0);
}

#[test]
fn size_bucket_assignment() {
    // Test bucket boundaries
    assert_eq!(MftStats::size_bucket(0), 0); // 0 bytes → bucket 0
    assert_eq!(MftStats::size_bucket(512), 0); // 512 bytes → bucket 0
    assert_eq!(MftStats::size_bucket(1023), 0); // 1023 bytes → bucket 0

    assert_eq!(MftStats::size_bucket(1024), 1); // 1 KB → bucket 1
    assert_eq!(MftStats::size_bucket(5 * 1024), 1); // 5 KB → bucket 1
    assert_eq!(MftStats::size_bucket(10 * 1024 - 1), 1); // 10 KB - 1 → bucket 1

    assert_eq!(MftStats::size_bucket(10 * 1024), 2); // 10 KB → bucket 2
    assert_eq!(MftStats::size_bucket(50 * 1024), 2); // 50 KB → bucket 2
    assert_eq!(MftStats::size_bucket(100 * 1024 - 1), 2); // 100 KB - 1 → bucket 2

    assert_eq!(MftStats::size_bucket(100 * 1024), 3); // 100 KB → bucket 3
    assert_eq!(MftStats::size_bucket(500 * 1024), 3); // 500 KB → bucket 3
    assert_eq!(MftStats::size_bucket(1024 * 1024 - 1), 3); // 1 MB - 1 → bucket 3

    assert_eq!(MftStats::size_bucket(1024 * 1024), 4); // 1 MB → bucket 4
    assert_eq!(MftStats::size_bucket(5 * 1024 * 1024), 4); // 5 MB → bucket 4

    assert_eq!(MftStats::size_bucket(10 * 1024 * 1024), 5); // 10 MB → bucket 5
    assert_eq!(MftStats::size_bucket(50 * 1024 * 1024), 5); // 50 MB → bucket 5

    assert_eq!(MftStats::size_bucket(100 * 1024 * 1024), 6); // 100 MB → bucket 6
    assert_eq!(MftStats::size_bucket(500 * 1024 * 1024), 6); // 500 MB → bucket 6

    assert_eq!(MftStats::size_bucket(1024 * 1024 * 1024), 7); // 1 GB → bucket 7
    assert_eq!(MftStats::size_bucket(10 * 1024 * 1024 * 1024), 7); // 10 GB → bucket 7
}

#[test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
fn extension_table_top_by_bytes() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add files with different extensions and sizes
    let files = [
        ("large.mp4", 1_000_000_000), // 1 GB
        ("medium.mp4", 500_000_000),  // 500 MB
        ("small.txt", 1_000),         // 1 KB
        ("tiny.txt", 500),            // 500 bytes
        ("doc.pdf", 10_000_000),      // 10 MB
        ("image.jpg", 5_000_000),     // 5 MB
    ];

    for (i, (name, size)) in files.iter().enumerate() {
        let frs = (i + 100) as u64;
        let offset = index.add_name(name);
        let ext_id = index.intern_extension(name);

        let rec = index.get_or_create(frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_stream.size = SizeInfo {
            length: *size,
            allocated: *size,
        };

        // Record the file size in the extension table
        index.extensions.record_file(ext_id, *size);
    }

    // Get top 3 extensions by bytes
    let top_3 = index.extensions.top_by_bytes(3);

    assert_eq!(top_3.len(), 3);

    // Should be sorted by bytes descending
    // mp4: 1.5 GB total (2 files)
    // pdf: 10 MB (1 file)
    // jpg: 5 MB (1 file)
    assert_eq!(top_3[0].1, "mp4");
    assert_eq!(top_3[0].2, 1_500_000_000); // total bytes
    assert_eq!(top_3[0].3, 2); // file count

    assert_eq!(top_3[1].1, "pdf");
    assert_eq!(top_3[1].2, 10_000_000);

    assert_eq!(top_3[2].1, "jpg");
    assert_eq!(top_3[2].2, 5_000_000);
}

#[test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
fn extension_table_top_by_count() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add files with different extensions
    let files = [
        ("file1.txt", 1000),
        ("file2.txt", 2000),
        ("file3.txt", 3000),
        ("doc1.pdf", 10000),
        ("doc2.pdf", 20000),
        ("image.jpg", 50000),
    ];

    for (i, (name, size)) in files.iter().enumerate() {
        let frs = (i + 100) as u64;
        let offset = index.add_name(name);
        let ext_id = index.intern_extension(name);

        let rec = index.get_or_create(frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_stream.size = SizeInfo {
            length: *size,
            allocated: *size,
        };

        // Record the file size in the extension table
        index.extensions.record_file(ext_id, *size);
    }

    // Get top 2 extensions by count
    let top_2 = index.extensions.top_by_count(2);

    assert_eq!(top_2.len(), 2);

    // Should be sorted by count descending
    // txt: 3 files
    // pdf: 2 files
    assert_eq!(top_2[0].1, "txt");
    assert_eq!(top_2[0].2, 3); // file count
    assert_eq!(top_2[0].3, 6000); // total bytes

    assert_eq!(top_2[1].1, "pdf");
    assert_eq!(top_2[1].2, 2);
    assert_eq!(top_2[1].3, 30000);
}

#[test]
fn byte_tracking_accuracy() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add files with different sizes and attributes
    let files = [
        ("file1.txt", 1_000, false, false, false), // 1,000 bytes, normal
        ("file2.txt", 10_000, true, false, false), // 10,000 bytes, hidden
        ("file3.txt", 100_000, false, true, false), // 100,000 bytes, system
        ("dir1", 0, false, false, true),           // directory
        ("file4.pdf", 1_048_576, false, false, false), // 1 MB, normal
        ("file5.pdf", 10_485_760, true, true, false), // 10 MB, hidden+system
    ];

    let mut expected_total = 0_u64;
    let mut expected_hidden = 0_u64;
    let mut expected_system = 0_u64;
    let mut expected_dir = 0_u64;

    for (i, (name, size, is_hidden, is_system, is_dir)) in files.iter().enumerate() {
        let frs = (i + 100) as u64;
        let offset = index.add_name(name);
        let ext_id = index.intern_extension(name);

        let rec = index.get_or_create(frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_stream.size = SizeInfo {
            length: *size,
            allocated: *size,
        };

        // Set attributes
        rec.stdinfo.set_directory(*is_dir);
        if *is_hidden {
            rec.stdinfo.flags |= StandardInfo::IS_HIDDEN;
        }
        if *is_system {
            rec.stdinfo.flags |= StandardInfo::IS_SYSTEM;
        }

        // Record the file size in the extension table
        index.extensions.record_file(ext_id, *size);

        // Track expected values
        expected_total += *size;
        if *is_hidden {
            expected_hidden += *size;
        }
        if *is_system {
            expected_system += *size;
        }
        if *is_dir {
            expected_dir += *size;
        }
    }

    // Recompute stats
    index.recompute_stats();

    // Verify byte totals
    assert_eq!(index.stats.total_bytes, expected_total);
    assert_eq!(index.stats.hidden_bytes, expected_hidden);
    assert_eq!(index.stats.system_bytes, expected_system);
    assert_eq!(index.stats.dir_bytes, expected_dir);

    // Verify size buckets
    // file1: 1,000 bytes → bucket 0 (< 1KB)
    // file2: 10,000 bytes → bucket 1 (1-10KB)
    // file3: 100,000 bytes → bucket 2 (10-100KB)
    // dir1: 0 bytes → bucket 0
    // file4: 1,000,000 bytes → bucket 4 (1-10MB)
    // file5: 10,000,000 bytes → bucket 5 (10-100MB)
    assert_eq!(index.stats.size_bucket_counts[0], 2); // 0 bytes + 1,000 bytes
    assert_eq!(index.stats.size_bucket_counts[1], 1); // 10,000 bytes
    assert_eq!(index.stats.size_bucket_counts[2], 1); // 100,000 bytes
    assert_eq!(index.stats.size_bucket_counts[3], 0); // none
    assert_eq!(index.stats.size_bucket_counts[4], 1); // 1,000,000 bytes
    assert_eq!(index.stats.size_bucket_counts[5], 1); // 10,000,000 bytes

    assert_eq!(index.stats.size_bucket_bytes[0], 1_000); // 0 + 1,000
    assert_eq!(index.stats.size_bucket_bytes[1], 10_000);
    assert_eq!(index.stats.size_bucket_bytes[2], 100_000);
    assert_eq!(index.stats.size_bucket_bytes[3], 0);
    assert_eq!(index.stats.size_bucket_bytes[4], 1_048_576);
    assert_eq!(index.stats.size_bucket_bytes[5], 10_485_760);
}

#[test]
fn extension_index_performance() {
    use std::time::Instant;

    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a large index with 10,000 files
    // 100 txt files, 9,900 other files
    let ext_txt = index.extensions.intern("txt");
    let ext_rs = index.extensions.intern("rs");
    let ext_py = index.extensions.intern("py");

    for i in 0_u64..10_000 {
        let (name, ext_id) = if i < 100 {
            (format!("file{}.txt", i), ext_txt)
        } else if i < 200 {
            (format!("file{}.rs", i), ext_rs)
        } else {
            (format!("file{}.py", i), ext_py)
        };

        let offset = index.add_name(&name);
        let rec = index.get_or_create(i.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
    }

    // Build extension index
    index.build_extension_index();

    // Benchmark O(n) scan
    let start = Instant::now();
    let mut count_scan = 0;
    for record in &index.records {
        if record.first_name.name.extension_id() == ext_txt {
            count_scan += 1;
        }
    }
    let scan_time = start.elapsed();

    // Benchmark O(matches) lookup
    let start = Instant::now();
    let ext_index = index.extension_index.as_ref().unwrap();
    let txt_records = ext_index.get_records(ext_txt);
    let count_lookup = txt_records.len();
    let lookup_time = start.elapsed();

    // Verify correctness
    assert_eq!(count_scan, 100);
    assert_eq!(count_lookup, 100);

    // Print performance comparison
    println!("\nExtension Index Performance (10,000 files, 100 matches):");
    println!("  O(n) scan:       {:?}", scan_time);
    println!("  O(matches) lookup: {:?}", lookup_time);
    if lookup_time.as_nanos() > 0 {
        let speedup = scan_time.as_nanos() / lookup_time.as_nanos();
        println!("  Speedup:         {}x", speedup);
    }

    // Lookup should be faster (though on small datasets the difference may
    // be small) The real benefit shows with millions of files
}
