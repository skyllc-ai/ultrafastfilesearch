//! Merge, self-healing, and tree-metrics regression tests.

use super::tests_helpers::{child_count, push_fragment_name, push_index_name, record_idx};
use super::*;

/// Test that extension records processed before base records in the same
/// fragment preserve extension names.
#[test]
fn test_extension_before_base_in_same_fragment() {
    let mut fragment = MftIndexFragment::with_capacity(10);

    let name2_ref = push_fragment_name(&mut fragment, "name2.txt");
    let name3_ref = push_fragment_name(&mut fragment, "name3.txt");

    let link0_idx = fragment.links.len() as u32;
    fragment.links.push(LinkInfo {
        next_entry: link0_idx + 1,
        name: name2_ref,
        parent_frs: 5,
    });
    let link1_idx = fragment.links.len() as u32;
    fragment.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: name3_ref,
        parent_frs: 6,
    });

    let record = fragment.get_or_create(100);
    record.first_name.name = name2_ref;
    record.first_name.parent_frs = 5;
    record.first_name.next_entry = link1_idx;
    record.name_count = 2;

    assert!(fragment.get_or_create(100).first_name.name.is_valid());
    assert_eq!(fragment.get_or_create(100).name_count, 2);

    let name1_ref = push_fragment_name(&mut fragment, "name1.txt");
    let record = fragment.get_or_create(100);
    let existing_first_name = record.first_name;
    let existing_name_valid = existing_first_name.name.is_valid();
    let existing_name_count = if existing_name_valid {
        record.name_count
    } else {
        0
    };

    record.first_name = LinkInfo {
        next_entry: NO_ENTRY,
        name: name1_ref,
        parent_frs: 5,
    };

    let first_name_next_entry = if existing_name_valid {
        let ext_link_idx = fragment.links.len() as u32;
        fragment.links.push(existing_first_name);
        ext_link_idx
    } else {
        NO_ENTRY
    };

    let record = fragment.get_or_create(100);
    record.first_name.next_entry = first_name_next_entry;
    record.name_count = 1 + existing_name_count;

    let record = fragment.get_or_create(100);
    assert_eq!(record.name_count, 3);
    assert!(record.first_name.name.is_valid());

    let first_next = record.first_name.next_entry;
    assert_ne!(first_next, NO_ENTRY);

    let link2 = &fragment.links[first_next as usize];
    assert!(link2.name.is_valid());
    assert_eq!(link2.next_entry, link1_idx);

    let link1 = &fragment.links[link1_idx as usize];
    assert!(link1.name.is_valid());
    assert_eq!(link1.next_entry, NO_ENTRY);
}

/// Test that cross-fragment merge correctly handles extension-only
/// placeholders.
#[test]
fn test_cross_fragment_merge_extension_placeholder() {
    let mut fragment_a = MftIndexFragment::with_capacity(10);
    let ext_name_ref = push_fragment_name(&mut fragment_a, "hardlink.txt");

    let record_a = fragment_a.get_or_create(100);
    record_a.first_name.name = ext_name_ref;
    record_a.first_name.parent_frs = 5;
    record_a.first_name.next_entry = NO_ENTRY;
    record_a.name_count = 1;

    assert!(fragment_a.get_or_create(100).first_name.name.is_valid());
    assert_eq!(fragment_a.get_or_create(100).stdinfo.created, 0);
    assert!(!fragment_a.get_or_create(100).has_base_data());

    let mut fragment_b = MftIndexFragment::with_capacity(10);
    let base_name_ref = push_fragment_name(&mut fragment_b, "original.txt");

    let record_b = fragment_b.get_or_create(100);
    record_b.first_name.name = base_name_ref;
    record_b.first_name.parent_frs = 5;
    record_b.first_name.next_entry = NO_ENTRY;
    record_b.name_count = 1;
    record_b.stdinfo.created = 132_456_789_012_345_678;
    record_b.stdinfo.modified = 132_456_789_012_345_678;

    assert!(fragment_b.get_or_create(100).first_name.name.is_valid());
    assert_ne!(fragment_b.get_or_create(100).stdinfo.created, 0);
    assert!(fragment_b.get_or_create(100).has_base_data());

    let mut index = MftIndex::new('D');
    index.merge_fragments(vec![fragment_a, fragment_b]);

    let record = &index.records[index.frs_to_idx[100] as usize];
    assert!(record.has_base_data());
    assert_ne!(record.stdinfo.created, 0);
    assert!(record.first_name.name.is_valid());
    assert_eq!(record.name_count, 2);
}

/// Test that cross-fragment merge keeps all extension names.
#[test]
fn test_cross_fragment_merge_multiple_extension_names() {
    let mut fragment_a = MftIndexFragment::with_capacity(10);
    let ext_hardlink_b = push_fragment_name(&mut fragment_a, "hardlink_b.txt");
    let ext_hardlink_c = push_fragment_name(&mut fragment_a, "hardlink_c.txt");

    let link_c_idx = fragment_a.links.len() as u32;
    fragment_a.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: ext_hardlink_c,
        parent_frs: 10,
    });

    let record_a = fragment_a.get_or_create(100);
    record_a.first_name.name = ext_hardlink_b;
    record_a.first_name.parent_frs = 5;
    record_a.first_name.next_entry = link_c_idx;
    record_a.name_count = 2;

    let mut fragment_b = MftIndexFragment::with_capacity(10);
    let base_original = push_fragment_name(&mut fragment_b, "original_a.txt");

    let record_b = fragment_b.get_or_create(100);
    record_b.first_name.name = base_original;
    record_b.first_name.parent_frs = 5;
    record_b.first_name.next_entry = NO_ENTRY;
    record_b.name_count = 1;
    record_b.stdinfo.created = 132_456_789_012_345_678;
    record_b.stdinfo.modified = 132_456_789_012_345_678;

    let mut index = MftIndex::new('D');
    index.merge_fragments(vec![fragment_a, fragment_b]);

    let record = &index.records[index.frs_to_idx[100] as usize];
    assert!(record.has_base_data());
    assert_eq!(record.name_count, 3);

    let name_0 = index.get_name(&index.get_name_at(record, 0).unwrap().name);
    let name_1 = index.get_name(&index.get_name_at(record, 1).unwrap().name);
    let name_2 = index.get_name(&index.get_name_at(record, 2).unwrap().name);

    assert_eq!(name_0, "original_a.txt");
    assert_eq!(name_1, "hardlink_b.txt");
    assert_eq!(name_2, "hardlink_c.txt");
}

/// Test that cross-fragment merge works when the base record comes first.
#[test]
fn test_cross_fragment_merge_base_first() {
    let mut fragment_a = MftIndexFragment::with_capacity(10);
    let base_original = push_fragment_name(&mut fragment_a, "original_a.txt");

    let record_a = fragment_a.get_or_create(100);
    record_a.first_name.name = base_original;
    record_a.first_name.parent_frs = 5;
    record_a.first_name.next_entry = NO_ENTRY;
    record_a.name_count = 1;
    record_a.stdinfo.created = 132_456_789_012_345_678;
    record_a.stdinfo.modified = 132_456_789_012_345_678;

    let mut fragment_b = MftIndexFragment::with_capacity(10);
    let ext_hardlink_b = push_fragment_name(&mut fragment_b, "hardlink_b.txt");
    let ext_hardlink_c = push_fragment_name(&mut fragment_b, "hardlink_c.txt");

    let link_c_idx = fragment_b.links.len() as u32;
    fragment_b.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: ext_hardlink_c,
        parent_frs: 10,
    });

    let record_b = fragment_b.get_or_create(100);
    record_b.first_name.name = ext_hardlink_b;
    record_b.first_name.parent_frs = 5;
    record_b.first_name.next_entry = link_c_idx;
    record_b.name_count = 2;

    let mut index = MftIndex::new('D');
    index.merge_fragments(vec![fragment_a, fragment_b]);

    let record = &index.records[index.frs_to_idx[100] as usize];
    assert!(record.has_base_data());
    assert_eq!(record.name_count, 3);

    let name_0 = index.get_name(&index.get_name_at(record, 0).unwrap().name);
    let name_1 = index.get_name(&index.get_name_at(record, 1).unwrap().name);
    let name_2 = index.get_name(&index.get_name_at(record, 2).unwrap().name);

    assert_eq!(name_0, "original_a.txt");
    assert_eq!(name_1, "hardlink_b.txt");
    assert_eq!(name_2, "hardlink_c.txt");
}

/// Test that `rebuild_children_from_names()` rebuilds child lists from parent
/// references.
#[test]
fn test_rebuild_children_from_names_basic() {
    let mut index = MftIndex::new('C');

    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs;
    root_rec.first_child = NO_ENTRY;

    let dir1_frs = 100_u64;
    let dir1_name = push_index_name(&mut index, "dir1");
    let rec = index.get_or_create(dir1_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = dir1_name;
    rec.first_name.parent_frs = root_frs;
    rec.first_child = NO_ENTRY;

    let file1_frs = 200_u64;
    let file1_name = push_index_name(&mut index, "file1.txt");
    let rec = index.get_or_create(file1_frs);
    rec.first_name.name = file1_name;
    rec.first_name.parent_frs = dir1_frs;

    let file2_frs = 201_u64;
    let file2_name = push_index_name(&mut index, "file2.txt");
    let rec = index.get_or_create(file2_frs);
    rec.first_name.name = file2_name;
    rec.first_name.parent_frs = root_frs;

    assert_eq!(
        index.records[record_idx(&index, root_frs)].first_child,
        NO_ENTRY
    );
    assert_eq!(
        index.records[record_idx(&index, dir1_frs)].first_child,
        NO_ENTRY
    );
    assert!(index.children.is_empty());

    index.rebuild_children_from_names();

    assert_ne!(
        index.records[record_idx(&index, root_frs)].first_child,
        NO_ENTRY
    );
    assert_ne!(
        index.records[record_idx(&index, dir1_frs)].first_child,
        NO_ENTRY
    );
    assert_eq!(child_count(&index, root_frs), 2);
    assert_eq!(child_count(&index, dir1_frs), 1);
}

/// Test that `rebuild_children_from_names()` handles hard links correctly.
#[test]
fn test_rebuild_children_from_names_hardlinks() {
    let mut index = MftIndex::new('C');

    let dir1_frs = 100_u64;
    let dir2_frs = 101_u64;
    let file_frs = 200_u64;

    let dir1_name = push_index_name(&mut index, "dir1");
    let dir1_rec = index.get_or_create(dir1_frs);
    dir1_rec.stdinfo.set_directory(true);
    dir1_rec.first_name.name = dir1_name;
    dir1_rec.first_name.parent_frs = dir1_frs;
    dir1_rec.first_child = NO_ENTRY;

    let dir2_name = push_index_name(&mut index, "dir2");
    let dir2_rec = index.get_or_create(dir2_frs);
    dir2_rec.stdinfo.set_directory(true);
    dir2_rec.first_name.name = dir2_name;
    dir2_rec.first_name.parent_frs = dir2_frs;
    dir2_rec.first_child = NO_ENTRY;

    let file_name = push_index_name(&mut index, "file.txt");
    let link_name = push_index_name(&mut index, "file.txt");
    index.links.push(LinkInfo {
        name: link_name,
        parent_frs: dir1_frs,
        next_entry: NO_ENTRY,
    });
    #[expect(
        clippy::cast_possible_truncation,
        reason = "FRS fits in usize on 64-bit"
    )]
    let link_idx = (index.links.len() - 1) as u32;

    let file_rec = index.get_or_create(file_frs);
    file_rec.first_name.name = file_name;
    file_rec.first_name.parent_frs = dir2_frs;
    file_rec.first_name.next_entry = link_idx;
    file_rec.name_count = 2;

    assert_eq!(
        index.records[record_idx(&index, dir1_frs)].first_child,
        NO_ENTRY
    );
    assert_eq!(
        index.records[record_idx(&index, dir2_frs)].first_child,
        NO_ENTRY
    );

    index.rebuild_children_from_names();

    let child1 = &index.children[index.records[record_idx(&index, dir1_frs)].first_child as usize];
    assert_eq!(child1.child_frs, file_frs);
    assert_eq!(child1.name_index, 0);

    let child2 = &index.children[index.records[record_idx(&index, dir2_frs)].first_child as usize];
    assert_eq!(child2.child_frs, file_frs);
    assert_eq!(child2.name_index, 1);
}

/// Test that `rebuild_children_from_names()` skips self-referencing root.
#[test]
fn test_rebuild_children_from_names_skips_root_self_reference() {
    let mut index = MftIndex::new('C');

    let root_frs = 5_u64;
    let rec = index.get_or_create(root_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.parent_frs = root_frs;
    rec.first_child = NO_ENTRY;

    index.rebuild_children_from_names();

    assert_eq!(
        index.records[record_idx(&index, root_frs)].first_child,
        NO_ENTRY
    );
    assert!(index.children.is_empty());
}

/// Test that tree metrics correctly handles empty directories.
#[test]
fn test_tree_metrics_empty_directory_descendants() {
    let mut index = MftIndex::new('C');

    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs;

    let empty_dir_frs = 100_u64;
    let empty_dir_name = push_index_name(&mut index, "EmptyDir");
    let rec = index.get_or_create(empty_dir_frs);
    rec.stdinfo.set_directory(true);
    rec.first_name.name = empty_dir_name;
    rec.first_name.parent_frs = root_frs;

    index.add_child_entry(root_frs, empty_dir_frs, 0);
    index.compute_tree_metrics();

    assert_eq!(
        index.records[record_idx(&index, empty_dir_frs)].descendants,
        1
    );
    assert_eq!(index.records[record_idx(&index, root_frs)].descendants, 2);
}

/// Test that tree metrics correctly handles directories with internal streams.
#[test]
fn test_tree_metrics_internal_streams_two_channel() {
    let mut index = MftIndex::new('C');

    let root_frs = 5_u64;
    let root_rec = index.get_or_create(root_frs);
    root_rec.stdinfo.set_directory(true);
    root_rec.first_name.parent_frs = root_frs;

    let dir_frs = 100_u64;
    let dir_name = push_index_name(&mut index, "DirWithInternal");
    let dir_idx_for_internal = {
        let rec = index.get_or_create(dir_frs);
        rec.stdinfo.set_directory(true);
        rec.first_name.name = dir_name;
        rec.first_name.parent_frs = root_frs;
        rec.total_stream_count = 2;
        index.frs_to_idx[dir_frs as usize] as usize
    };

    let internal_idx = index.internal_streams.len() as u32;
    index.internal_streams.push(InternalStreamInfo {
        next_entry: NO_ENTRY,
        size: SizeInfo {
            length: 256,
            allocated: 512,
        },
        flags: 0,
    });
    index.records[dir_idx_for_internal].first_internal_stream = internal_idx;

    let file_frs = 200_u64;
    let file_name = push_index_name(&mut index, "file.txt");
    let rec = index.get_or_create(file_frs);
    rec.first_name.name = file_name;
    rec.first_name.parent_frs = dir_frs;
    rec.first_stream.size = SizeInfo {
        length: 1000,
        allocated: 4096,
    };

    index.add_child_entry(root_frs, dir_frs, 0);
    index.add_child_entry(dir_frs, file_frs, 0);
    crate::tree_metrics::compute_tree_metrics(&mut index, false, false);

    let dir_idx = record_idx(&index, dir_frs);
    assert_eq!(index.records[dir_idx].treesize, 1000);
    assert_eq!(index.records[dir_idx].descendants, 2);

    let root_idx = record_idx(&index, root_frs);
    assert_eq!(index.records[root_idx].treesize, 1256);
}
