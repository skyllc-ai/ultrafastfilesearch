// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Display and performance-oriented tests for the split `index` module.

use super::*;

#[test]
fn display_stats() {
    // Create a simple index with some files
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Add a few files with different extensions
    let txt_ext_id = index.extensions.intern("txt");
    let pdf_ext_id = index.extensions.intern("pdf");
    let jpg_ext_id = index.extensions.intern("jpg");

    // Record extensions
    index.extensions.record_file(txt_ext_id, 1000);
    index.extensions.record_file(txt_ext_id, 2000);
    index.extensions.record_file(pdf_ext_id, 10_000_000);
    index.extensions.record_file(jpg_ext_id, 5_000_000);

    // Update stats manually
    index.stats.record_count = 4;
    index.stats.file_count = 4;
    index.stats.total_bytes = 15_003_000;
    index.stats.hidden_bytes = 1000;
    index.stats.system_bytes = 2000;

    // Size buckets
    index.stats.size_bucket_counts[0] = 2; // 0-1KB
    index.stats.size_bucket_counts[4] = 2; // 1-10MB

    index.stats.size_bucket_bytes[0] = 3000;
    index.stats.size_bucket_bytes[4] = 15_000_000;

    // Call display_stats - this should not panic
    // We can't easily test the output, but we can verify it doesn't crash
    index.display_stats();
}

/// Performance test: Extension index query performance
///
/// Run with: `cargo test --release --
/// test_extension_index_query_performance --nocapture`
#[test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
fn extension_index_query_performance() {
    use std::time::Instant;

    // Create index with 10K files across 10 extensions
    let mut index = MftIndex::with_capacity(crate::platform::DriveLetter::C, 10_000);

    // Create 10 different extensions
    let mut ext_ids = Vec::new();
    for i in 0..10 {
        let ext = format!("ext{}", i);
        let ext_id = index.extensions.intern(&ext);
        ext_ids.push(ext_id);
    }

    // Add 10K files (1000 per extension)
    for i in 0..10_000 {
        let frs = (1000 + i) as u64;
        let ext_id = ext_ids[i % 10];

        // Create record with extension
        let name = format!("file{i}.ext{}", i % 10);
        let offset = index.add_name(&name);
        let rec = index.get_or_create(frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_stream.size.length = 1024;

        // Record in extension table
        index.extensions.record_file(ext_id, 1024);
    }

    // Build extension index
    let build_start = Instant::now();
    index.extension_index = Some(ExtensionIndex::build(&index));
    let build_time = build_start.elapsed();

    assert!(
        build_time.as_millis() < 50,
        "Extension index build took too long: {build_time:?}"
    );

    // Query performance - should be O(matches) not O(n)
    let ext_index = index.extension_index.as_ref().unwrap();

    let query_start = Instant::now();
    let ext0_id = ext_ids[0];
    let records = ext_index.get_records(ext0_id);
    let query_time = query_start.elapsed();

    assert_eq!(records.len(), 1000, "Should find 1000 files with ext0");
    assert!(
        query_time.as_micros() < 100,
        "Extension query took too long: {query_time:?}"
    );
}

/// Performance test: Full post-processing pipeline
///
/// Run with: `cargo test --release -- test_full_postprocessing_performance
/// --nocapture`
#[test]
fn full_postprocessing_performance() {
    use std::time::Instant;

    // Create a realistic index with 100K files
    let mut index = MftIndex::with_capacity(crate::platform::DriveLetter::C, 100_000);

    // Add root directory
    let root_frs = 5;
    let root_rec = index.get_or_create(root_frs.into());
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = Into::into(root_frs); // Self-parent

    // Add 100 directories
    for dir_i in 0..100 {
        let dir_frs = 100 + dir_i;
        let rec = index.get_or_create(dir_frs.into());
        rec.stdinfo.set_directory(true);
        rec.first_name.parent_frs = Into::into(root_frs);
    }

    // Add 1000 files per directory (100K total)
    for dir_i in 0..100 {
        let dir_frs = 100 + dir_i;
        for file_i in 0..1000 {
            let file_frs = 10_000 + dir_i * 1000 + file_i;
            let rec = index.get_or_create(file_frs.into());
            rec.first_name.parent_frs = Into::into(dir_frs);
            rec.first_stream.size.length = 1024;
        }
    }

    // Measure extension index build
    let ext_start = Instant::now();
    index.extension_index = Some(ExtensionIndex::build(&index));
    let ext_time = ext_start.elapsed();

    // Measure directory sorting
    let sort_start = Instant::now();
    index.sort_directory_children();
    let sort_time = sort_start.elapsed();

    // Measure tree metrics
    let tree_start = Instant::now();
    index.compute_tree_metrics();
    let tree_time = tree_start.elapsed();

    let total_time = ext_time + sort_time + tree_time;

    // Verify performance targets (for 100K files)
    assert!(
        ext_time.as_millis() < 50,
        "Extension index too slow: {ext_time:?}"
    );
    assert!(
        sort_time.as_millis() < 200,
        "Sorting too slow: {sort_time:?}"
    );
    assert!(
        tree_time.as_millis() < 100,
        "Tree metrics too slow: {tree_time:?}"
    );
    assert!(
        total_time.as_millis() < 350,
        "Total post-processing too slow: {total_time:?}"
    );
}
