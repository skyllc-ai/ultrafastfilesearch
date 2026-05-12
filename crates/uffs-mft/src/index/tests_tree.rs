// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tree metrics tests for the split `index` module.

use super::*;

#[test]
fn compute_tree_metrics_simple() {
    let mut index = MftIndex::new('C');

    // Create a simple tree:
    // root (FRS 5)
    //   ├── dir1 (FRS 100)
    //   │   ├── file1.txt (FRS 200, 1000 bytes)
    //   │   └── file2.txt (FRS 201, 2000 bytes)
    //   └── file3.txt (FRS 202, 500 bytes)

    // Root directory
    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs; // Self-parent

    // dir1
    let dir1_frs = 100_u64;
    let offset = index.add_name("dir1");
    let rec = index.get_or_create(dir1_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = root_frs;

    // file1.txt (child of dir1)
    let file1_frs = 200_u64;
    let offset = index.add_name("file1.txt");
    let rec = index.get_or_create(file1_frs);
    rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = dir1_frs;
    rec.first_stream.size = SizeInfo {
        length: 1000,
        allocated: 4096,
    };

    // file2.txt (child of dir1)
    let file2_frs = 201_u64;
    let offset = index.add_name("file2.txt");
    let rec = index.get_or_create(file2_frs);
    rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = dir1_frs;
    rec.first_stream.size = SizeInfo {
        length: 2000,
        allocated: 4096,
    };

    // file3.txt (child of root)
    let file3_frs = 202_u64;
    let offset = index.add_name("file3.txt");
    let rec = index.get_or_create(file3_frs);
    rec.first_name.name = IndexNameRef::new(offset, 9, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = root_frs;
    rec.first_stream.size = SizeInfo {
        length: 500,
        allocated: 4096,
    };

    // Add child entries (required for tree metrics algorithm)
    index.add_child_entry(root_frs, dir1_frs, 0);
    index.add_child_entry(root_frs, file3_frs, 0);
    index.add_child_entry(dir1_frs, file1_frs, 0);
    index.add_child_entry(dir1_frs, file2_frs, 0);

    // Compute tree metrics
    index.compute_tree_metrics();

    // Verify file1.txt (leaf)
    // Files have descendants = 0, but contribute 1 to their parent.
    let file1_idx = index.frs_to_idx_opt(file1_frs).unwrap();
    assert_eq!(index.records[file1_idx].descendants, 0);
    assert_eq!(index.records[file1_idx].treesize, 1000);
    assert_eq!(index.records[file1_idx].tree_allocated, 4096);

    // Verify file2.txt (leaf)
    let file2_idx = index.frs_to_idx_opt(file2_frs).unwrap();
    assert_eq!(index.records[file2_idx].descendants, 0);
    assert_eq!(index.records[file2_idx].treesize, 2000);
    assert_eq!(index.records[file2_idx].tree_allocated, 4096);

    // Verify file3.txt (leaf)
    let file3_idx = index.frs_to_idx_opt(file3_frs).unwrap();
    assert_eq!(index.records[file3_idx].descendants, 0);
    assert_eq!(index.records[file3_idx].treesize, 500);
    assert_eq!(index.records[file3_idx].tree_allocated, 4096);

    // Verify dir1 (has 2 children: file1 and file2)
    // descendants = 1 (self) + sum(max(1, child.descendants))
    // dir1 = 1 + 1 + 1 = 3
    let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
    assert_eq!(index.records[dir1_idx].descendants, 3); // 1 + file1(1) + file2(1)
    assert_eq!(index.records[dir1_idx].treesize, 3000); // 0 + 1000 + 2000
    assert_eq!(index.records[dir1_idx].tree_allocated, 8192); // 0 + 4096 + 4096

    // Verify root (has dir1 + file3)
    // descendants = 1 (self) + sum(child.descendants)
    // root = 1 + 3 + 1 = 5
    let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
    assert_eq!(index.records[root_idx].descendants, 5); // 1 + dir1(3) + file3(1)
    assert_eq!(index.records[root_idx].treesize, 3500); // 0 + 3000 + 500
    assert_eq!(index.records[root_idx].tree_allocated, 12288); // 0 + 8192 +
    // 4096
}

#[test]
fn compute_tree_metrics_deep_tree() {
    let mut index = MftIndex::new('C');

    // Create a deep tree:
    // root (FRS 5)
    //   └── dir1 (FRS 100)
    //       └── dir2 (FRS 101)
    //           └── dir3 (FRS 102)
    //               └── file.txt (FRS 200, 1000 bytes)

    // Root
    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs;

    // dir1
    let dir1_frs = 100_u64;
    let offset = index.add_name("dir1");
    let rec = index.get_or_create(dir1_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = root_frs;

    // dir2
    let dir2_frs = 101_u64;
    let offset = index.add_name("dir2");
    let rec = index.get_or_create(dir2_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = dir1_frs;

    // dir3
    let dir3_frs = 102_u64;
    let offset = index.add_name("dir3");
    let rec = index.get_or_create(dir3_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = IndexNameRef::new(offset, 4, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = dir2_frs;

    // file.txt
    let file_frs = 200_u64;
    let offset = index.add_name("file.txt");
    let rec = index.get_or_create(file_frs);
    rec.first_name.name = IndexNameRef::new(offset, 8, true, IndexNameRef::NO_EXTENSION);
    rec.first_name.parent_frs = dir3_frs;
    rec.first_stream.size = SizeInfo {
        length: 1000,
        allocated: 4096,
    };

    // Add child entries (required for tree metrics algorithm)
    index.add_child_entry(root_frs, dir1_frs, 0);
    index.add_child_entry(dir1_frs, dir2_frs, 0);
    index.add_child_entry(dir2_frs, dir3_frs, 0);
    index.add_child_entry(dir3_frs, file_frs, 0);

    // Compute tree metrics
    index.compute_tree_metrics();

    // Files have descendants = 0, directories have descendants = 1 +
    // sum(max(1, child.descendants)) Formula: parent.descendants = 1 +
    // sum(max(1, child.descendants)) file.txt = 0, dir3 = 1+max(1,0)=2,
    // dir2 = 1+2=3, dir1 = 1+3=4, root = 1+4=5

    // Verify file.txt (leaf)
    let file_idx = index.frs_to_idx_opt(file_frs).unwrap();
    assert_eq!(index.records[file_idx].descendants, 0); // Files have 0
    assert_eq!(index.records[file_idx].treesize, 1000);

    // Verify dir3 (has 1 child: file.txt)
    let dir3_idx = index.frs_to_idx_opt(dir3_frs).unwrap();
    assert_eq!(index.records[dir3_idx].descendants, 2); // 1 + max(1, file.txt(0)) = 1 + 1 = 2
    assert_eq!(index.records[dir3_idx].treesize, 1000);

    // Verify dir2 (has 1 child: dir3)
    let dir2_idx = index.frs_to_idx_opt(dir2_frs).unwrap();
    assert_eq!(index.records[dir2_idx].descendants, 3); // 1 + dir3(2)
    assert_eq!(index.records[dir2_idx].treesize, 1000);

    // Verify dir1 (has 1 child: dir2)
    let dir1_idx = index.frs_to_idx_opt(dir1_frs).unwrap();
    assert_eq!(index.records[dir1_idx].descendants, 4); // 1 + dir2(3)
    assert_eq!(index.records[dir1_idx].treesize, 1000);

    // Verify root (has 1 child: dir1)
    let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
    assert_eq!(index.records[root_idx].descendants, 5); // 1 + dir1(4)
    assert_eq!(index.records[root_idx].treesize, 1000);
}

#[test]
fn compute_tree_metrics_empty() {
    let mut index = MftIndex::new('C');

    // Empty index should not crash
    index.compute_tree_metrics();

    assert_eq!(index.records.len(), 0);
}

#[test]
fn compute_tree_metrics_performance() {
    use std::time::Instant;

    let mut index = MftIndex::new('C');

    // Create a large tree with 10,000 files
    // Structure: root -> 100 directories -> 100 files each

    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs;

    let mut frs_counter = 1000_u64;

    // Create 100 directories
    for dir_idx in 0..100_u64 {
        let dir_frs = 100 + dir_idx;
        let dir_name = format!("dir{:03}", dir_idx);
        let offset = index.add_name(&dir_name);
        let rec = index.get_or_create(dir_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = IndexNameRef::new(
            offset,
            u16::try_from(dir_name.len()).unwrap(),
            true,
            IndexNameRef::NO_EXTENSION,
        );
        rec.first_name.parent_frs = root_frs;

        // Add child entry for directory
        index.add_child_entry(root_frs, dir_frs, 0);

        // Create 100 files in each directory
        for file_idx in 0..100 {
            let file_frs = frs_counter;
            frs_counter += 1;

            let file_name = format!("file{:03}.txt", file_idx);
            let offset = index.add_name(&file_name);
            let rec = index.get_or_create(file_frs);
            rec.first_name.name = IndexNameRef::new(
                offset,
                u16::try_from(file_name.len()).unwrap(),
                true,
                IndexNameRef::NO_EXTENSION,
            );
            rec.first_name.parent_frs = dir_frs;
            rec.first_stream.size = SizeInfo {
                length: 1000,
                allocated: 4096,
            };

            // Add child entry for file
            index.add_child_entry(dir_frs, file_frs, 0);
        }
    }

    // Measure tree metrics computation time
    let start = Instant::now();
    index.compute_tree_metrics();
    let elapsed = start.elapsed();

    println!(
        "Computed tree metrics for {} records in {:?}",
        index.records.len(),
        elapsed
    );

    // Verify root has correct descendants count
    // Files have descendants = 0, directories have descendants = 1 +
    // sum(max(1, child.descendants)) Each file = 0 (but contributes 1 to
    // parent) Each dir_i = 1 (self) + 100 files * max(1,0) = 1 + 100 = 101
    // root = 1 (self) + 100 dirs * 101 = 10,101
    let root_idx = index.frs_to_idx_opt(root_frs).unwrap();
    assert_eq!(index.records[root_idx].descendants, 10_101); // 1 + 100 * 101

    // Verify root has correct total size
    assert_eq!(index.records[root_idx].treesize, 10_000_000); // 10,000 files * 1000 bytes

    // Verify a directory has correct descendants
    // Each dir = 1 (self) + 100 files * max(1,0) = 1 + 100 = 101
    let dir0_idx = index.frs_to_idx_opt(100).unwrap();
    assert_eq!(index.records[dir0_idx].descendants, 101); // 1 + 100 * max(1,0)

    // Computation should be fast (< 50ms for 10,000 files)
    assert!(
        elapsed.as_millis() < 100,
        "Tree metrics took too long: {:?}",
        elapsed
    );
}
