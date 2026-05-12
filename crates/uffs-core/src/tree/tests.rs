// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

use uffs_polars::{Column, DataFrame};

use super::*;

/// Create a test `DataFrame` with a directory structure:
/// ```text
/// root (frs=5)
/// └── Users (frs=100, size=0, alloc=4096)
///     ├── john (frs=101, size=0, alloc=4096)
///     │   └── file.txt (frs=102, size=1000, alloc=4096)
///     └── Documents (frs=103, size=0, alloc=4096)
///         └── doc.pdf (frs=104, size=50000, alloc=53248)
/// ```
fn create_test_df() -> DataFrame {
    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[5_u64, 100, 101, 102, 103, 104]),
        Column::new("parent_frs".into(), &[0_u64, 5, 100, 101, 100, 103]),
        Column::new("is_directory".into(), &[
            true, true, true, false, true, false,
        ]),
        Column::new("size".into(), &[0_u64, 0, 0, 1000, 0, 50000]),
        Column::new("allocated_size".into(), &[
            4096_u64, 4096, 4096, 4096, 4096, 53248,
        ]),
    ])
    .unwrap()
}

#[test]
fn tree_column_parse() {
    assert_eq!(
        TreeColumn::parse("descendants"),
        Some(TreeColumn::Descendants)
    );
    assert_eq!(
        TreeColumn::parse("decendents"),
        Some(TreeColumn::Descendants)
    ); // typo support
    assert_eq!(TreeColumn::parse("treesize"), Some(TreeColumn::TreeSize));
    assert_eq!(TreeColumn::parse("tree_size"), Some(TreeColumn::TreeSize));
    assert_eq!(TreeColumn::parse("bulkiness"), Some(TreeColumn::Bulkiness));
    assert_eq!(TreeColumn::parse("unknown"), None);
}

#[test]
fn tree_index_from_dataframe() {
    let df = create_test_df();
    let tree = TreeIndex::from_dataframe(&df).unwrap();

    // Check children map
    assert_eq!(tree.children.get(&5).map(Vec::len), Some(1)); // root has 1 child
    assert_eq!(tree.children.get(&100).map(Vec::len), Some(2)); // Users has 2 children
    assert_eq!(tree.children.get(&101).map(Vec::len), Some(1)); // john has 1 child
    assert_eq!(tree.children.get(&102), None); // file.txt has no children
}

#[test]
fn descendants_count() {
    let df = create_test_df();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // root (5): Users + john + file.txt + Documents + doc.pdf = 5
    assert_eq!(tree.descendants(5), 5);
    // Users (100): john + file.txt + Documents + doc.pdf = 4
    assert_eq!(tree.descendants(100), 4);
    // john (101): file.txt = 1
    assert_eq!(tree.descendants(101), 1);
    // file.txt (102): 0 (file)
    assert_eq!(tree.descendants(102), 0);
    // Documents (103): doc.pdf = 1
    assert_eq!(tree.descendants(103), 1);
}

#[test]
fn treesize() {
    let df = create_test_df();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // root (5): 0 + 0 + 0 + 1000 + 0 + 50000 = 51000
    assert_eq!(tree.treesize(5), 51000);
    // Users (100): 0 + 0 + 1000 + 0 + 50000 = 51000
    assert_eq!(tree.treesize(100), 51000);
    // john (101): 0 + 1000 = 1000
    assert_eq!(tree.treesize(101), 1000);
    // file.txt (102): 1000
    assert_eq!(tree.treesize(102), 1000);
    // Documents (103): 0 + 50000 = 50000
    assert_eq!(tree.treesize(103), 50000);
}

#[test]
fn tree_allocated() {
    let df = create_test_df();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // root (5): 4096 + 4096 + 4096 + 4096 + 4096 + 53248 = 73728
    assert_eq!(tree.tree_allocated(5), 73728);
    // Users (100): 4096 + 4096 + 4096 + 4096 + 53248 = 69632
    assert_eq!(tree.tree_allocated(100), 69632);
}

#[test]
fn bulkiness() {
    let df = create_test_df();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // file.txt (102): allocated=4096, bulkiness_sum = 4096 (file's own allocated
    // size)
    assert_eq!(tree.bulkiness(102), 4096);

    // doc.pdf (104): allocated=53248, bulkiness_sum = 53248 (file's own allocated
    // size)
    assert_eq!(tree.bulkiness(104), 53248);

    // john (101): tree_allocated = 4096 + 4096 = 8192
    // threshold = 8192 / 100 = 81
    // file.txt bulkiness = 4096 >= 81, so it's filtered out
    // Result: 0 (all children filtered out)
    assert_eq!(tree.bulkiness(101), 0);

    // Documents (103): tree_allocated = 4096 + 53248 = 57344
    // threshold = 57344 / 100 = 573
    // doc.pdf bulkiness = 53248 >= 573, so it's filtered out
    // Result: 0 (all children filtered out)
    assert_eq!(tree.bulkiness(103), 0);
}

#[test]
fn bulkiness_with_many_small_files() {
    // Create a directory with many small files to test bulkiness filtering
    // parent (frs=1) with 10 small files (100 bytes each) and 1 large file (10000
    // bytes)
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[
            1_u64, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
        ]),
        Column::new("parent_frs".into(), &[
            0_u64, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ]),
        Column::new("is_directory".into(), &[
            true, false, false, false, false, false, false, false, false, false, false, false,
        ]),
        Column::new("size".into(), &[
            0_u64, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 10000,
        ]),
        Column::new("allocated_size".into(), &[
            4096_u64, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 10000,
        ]),
    ])
    .unwrap();

    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // parent (1): tree_allocated = 4096 + 10*100 + 10000 = 15096
    // threshold = 15096 / 100 = 150
    // Large file (10000) >= 150, filtered out
    // Small files (100 each) < 150, kept
    // bulkiness_sum = 10 * 100 = 1000
    assert_eq!(tree.bulkiness(1), 1000);
}

#[test]
fn add_tree_columns_works() {
    let df = create_test_df();
    let result = add_tree_columns(&df, &[TreeColumn::Descendants, TreeColumn::TreeSize]).unwrap();

    // Check descendants column exists
    let desc_col = result.column("descendants").unwrap().u64().unwrap();
    assert_eq!(desc_col.get(0), Some(5)); // root
    assert_eq!(desc_col.get(3), Some(0)); // file.txt

    // Check treesize column exists
    let size_col = result.column("treesize").unwrap().u64().unwrap();
    assert_eq!(size_col.get(0), Some(51000)); // root
}

#[test]
fn add_tree_columns_empty() {
    let df = create_test_df();
    let result = add_tree_columns(&df, &[]).unwrap();

    // Should return unchanged DataFrame
    assert_eq!(result.width(), df.width());
}

#[test]
fn memoization() {
    let df = create_test_df();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();

    // First call computes - use the result to avoid unused warning
    let desc_count = tree.descendants(5);
    assert_eq!(desc_count, 5);
    assert!(!tree.metrics_cache.is_empty());

    // Cache should have entries for all nodes traversed
    assert!(tree.metrics_cache.contains_key(&5));
    assert!(tree.metrics_cache.contains_key(&100));
    assert!(tree.metrics_cache.contains_key(&101));
}

/// Benchmark test to measure tree building cost.
///
/// Run with: `cargo test -p uffs-core tree::tests::bench_tree_building`
/// --release -- --nocapture
#[test]
fn bench_tree_building() {
    use std::time::Instant;

    // Create a large DataFrame simulating 100K files
    let count = 100_000_usize;
    let frs_vec: Vec<u64> = (0..count).map(|i| i as u64).collect(); // usize→u64 lossless
    // Create a tree structure: every 100 files share a parent directory
    let parent_vec: Vec<u64> = (0..count)
        .map(|idx| {
            if idx < 100 {
                0 // Root level
            } else {
                (idx / 100) as u64 // usize→u64 lossless
            }
        })
        .collect();
    let is_dir_vec: Vec<bool> = (0..count).map(|idx| idx % 100 == 0).collect();
    let size_vec: Vec<u64> = (0..count).map(|idx| (idx * 1000) as u64).collect(); // usize→u64 lossless
    let alloc_vec: Vec<u64> = (0..count)
        .map(|idx| ((idx * 1000).div_ceil(4096) * 4096) as u64) // usize→u64 lossless
        .collect();

    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), frs_vec),
        Column::new("parent_frs".into(), parent_vec),
        Column::new("is_directory".into(), is_dir_vec),
        Column::new("size".into(), size_vec),
        Column::new("allocated_size".into(), alloc_vec),
    ])
    .unwrap();

    // Measure tree index building
    let start = Instant::now();
    let mut tree = TreeIndex::from_dataframe(&df).unwrap();
    let build_time = start.elapsed();

    // Measure metrics computation for all directories
    let start = Instant::now();
    for frs in 0..count as u64 {
        // usize→u64 lossless
        if frs % 100 == 0 {
            _ = tree.descendants(frs);
        }
    }
    let compute_time = start.elapsed();

    println!("\n=== Tree Building Benchmark ({count} files) ===");
    println!("Tree index build time: {build_time:?}");
    println!("Metrics computation time: {compute_time:?}");
    println!("Total time: {:?}", build_time + compute_time);
    println!(
        "Per-file build cost: {:?}",
        build_time / uffs_mft::len_to_u32(count)
    );
}
