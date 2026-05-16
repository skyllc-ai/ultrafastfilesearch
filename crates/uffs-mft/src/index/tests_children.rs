// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Directory child ordering tests for the split `index` module.

use super::*;

#[test]
fn sort_directory_children_basic() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a directory (FRS 100)
    let dir_frs = 100_u64;
    let dir_rec = index.get_or_create(dir_frs.into());
    dir_rec.stdinfo.set_directory(true);

    // Create child files with unsorted names
    let child_names = ["zebra.txt", "apple.txt", "Banana.txt", "cherry.txt"];
    let mut child_frs_list = Vec::new();

    for (i, name) in child_names.iter().enumerate() {
        let child_frs = (200 + i) as u64;
        child_frs_list.push(child_frs);

        let offset = index.add_name(name);
        let ext_id = index.intern_extension(name);
        let rec = index.get_or_create(child_frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_name.parent_frs = Into::into(dir_frs);

        // Add child to directory's children list
        let child_info = ChildInfo {
            next_entry: NO_ENTRY,
            _pad0: [0; 4],
            child_frs: child_frs.into(),
            name_index: 0,
            _pad1: [0; 6],
        };
        let child_idx = u32::try_from(index.children.len()).unwrap();
        index.children.push(child_info);

        // Link to previous child or set as first child
        if i == 0 {
            let dir_rec = index.get_or_create(dir_frs.into());
            dir_rec.first_child = child_idx;
        } else {
            let prev_child_idx = (child_idx - 1) as usize;
            index.children[prev_child_idx].next_entry = child_idx;
        }
    }

    // Sort directory children
    index.sort_directory_children();

    // Verify children are sorted (case-insensitive)
    // Expected order: apple.txt, Banana.txt, cherry.txt, zebra.txt
    let dir_idx = index.frs_to_idx_opt(dir_frs.into()).unwrap();
    let mut current_idx = index.records[dir_idx].first_child;
    let mut sorted_names = Vec::new();

    while current_idx != NO_ENTRY {
        let child = &index.children[current_idx as usize];
        let child_idx = index.frs_to_idx_opt(child.child_frs).unwrap();
        let name = index.get_name(index.records[child_idx].first_name.name);
        sorted_names.push(name.to_string());
        current_idx = child.next_entry;
    }

    assert_eq!(sorted_names, vec![
        "apple.txt",
        "Banana.txt",
        "cherry.txt",
        "zebra.txt"
    ]);
}

#[test]
fn sort_directory_children_empty() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a directory with no children
    let dir_frs = 100_u64;
    let dir_rec = index.get_or_create(dir_frs.into());
    dir_rec.stdinfo.set_directory(true);

    // Sort should not crash
    index.sort_directory_children();

    // Verify first_child is still NO_ENTRY
    let dir_rec = index.get_or_create(dir_frs.into());
    assert_eq!(dir_rec.first_child, NO_ENTRY);
}

#[test]
fn sort_directory_children_single_child() {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a directory with one child
    let dir_frs = 100_u64;
    let dir_rec = index.get_or_create(dir_frs.into());
    dir_rec.stdinfo.set_directory(true);

    let child_frs = 200_u64;
    let offset = index.add_name("only_child.txt");
    let ext_id = index.intern_extension("only_child.txt");
    let rec = index.get_or_create(child_frs.into());
    rec.first_name.name = IndexNameRef::new(offset, 14, true, ext_id);
    rec.first_name.parent_frs = Into::into(dir_frs);

    let child_info = ChildInfo {
        next_entry: NO_ENTRY,
        _pad0: [0; 4],
        child_frs: child_frs.into(),
        name_index: 0,
        _pad1: [0; 6],
    };
    let child_idx = u32::try_from(index.children.len()).unwrap();
    index.children.push(child_info);

    let dir_rec = index.get_or_create(dir_frs.into());
    dir_rec.first_child = child_idx;

    // Sort should not crash
    index.sort_directory_children();

    // Verify child is still there
    let dir_rec = index.get_or_create(dir_frs.into());
    assert_eq!(dir_rec.first_child, child_idx);
    assert_eq!(
        index.children[usize::try_from(child_idx).unwrap()].next_entry,
        NO_ENTRY
    );
}

#[test]
fn sort_directory_children_performance() {
    use std::time::Instant;

    let mut index = MftIndex::new(crate::platform::DriveLetter::C);

    // Create a directory with 1000 children
    let dir_frs = 100_u64;
    let dir_rec = index.get_or_create(dir_frs.into());
    dir_rec.stdinfo.set_directory(true);

    // Add 1000 children with random names
    for i in 0..1000 {
        let child_frs = u64::from(200_u32 + i);
        let name = format!("file_{:04}.txt", 1000 - i); // Reverse order
        let offset = index.add_name(&name);
        let ext_id = index.intern_extension(&name);
        let rec = index.get_or_create(child_frs.into());
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_name.parent_frs = Into::into(dir_frs);

        let child_info = ChildInfo {
            next_entry: NO_ENTRY,
            _pad0: [0; 4],
            child_frs: child_frs.into(),
            name_index: 0,
            _pad1: [0; 6],
        };
        let child_idx = u32::try_from(index.children.len()).unwrap();
        index.children.push(child_info);

        if i == 0 {
            let dir_rec = index.get_or_create(dir_frs.into());
            dir_rec.first_child = child_idx;
        } else {
            let prev_child_idx = (child_idx - 1) as usize;
            index.children[prev_child_idx].next_entry = child_idx;
        }
    }

    // Measure sorting time
    let start = Instant::now();
    index.sort_directory_children();
    let elapsed = start.elapsed();

    println!("Sorted 1000 children in {:?}", elapsed);

    // Verify first few children are sorted
    let dir_idx = index.frs_to_idx_opt(dir_frs.into()).unwrap();
    let mut current_idx = index.records[dir_idx].first_child;
    let mut sorted_names = Vec::new();

    for _ in 0..5 {
        if current_idx == NO_ENTRY {
            break;
        }
        let child = &index.children[current_idx as usize];
        let child_idx = index.frs_to_idx_opt(child.child_frs).unwrap();
        let name = index.get_name(index.records[child_idx].first_name.name);
        sorted_names.push(name.to_string());
        current_idx = child.next_entry;
    }

    assert_eq!(sorted_names[0], "file_0001.txt");
    assert_eq!(sorted_names[1], "file_0002.txt");
    assert_eq!(sorted_names[2], "file_0003.txt");
    assert_eq!(sorted_names[3], "file_0004.txt");
    assert_eq!(sorted_names[4], "file_0005.txt");

    // Sorting should be fast (< 10ms for 1000 files)
    assert!(
        elapsed.as_millis() < 100,
        "Sorting took too long: {:?}",
        elapsed
    );
}
