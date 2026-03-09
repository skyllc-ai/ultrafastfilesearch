//! Shared helpers for split `index` tests.

use super::*;

/// Append a filename to a fragment and return its packed reference.
pub(super) fn push_fragment_name(fragment: &mut MftIndexFragment, name: &str) -> IndexNameRef {
    let offset = fragment.names.len() as u32;
    fragment.names.push_str(name);
    IndexNameRef::new(
        offset,
        u16::try_from(name.len()).unwrap(),
        name.is_ascii(),
        0,
    )
}

/// Append a filename to the main index names buffer and return its packed
/// reference.
pub(super) fn push_index_name(index: &mut MftIndex, name: &str) -> IndexNameRef {
    let offset = index.add_name(name);
    IndexNameRef::new(
        offset,
        u16::try_from(name.len()).unwrap(),
        name.is_ascii(),
        IndexNameRef::NO_EXTENSION,
    )
}

/// Resolve a record index from an FRS for test assertions.
pub(super) fn record_idx(index: &MftIndex, frs: u64) -> usize {
    index.frs_to_idx_opt(frs).unwrap()
}

/// Count the number of child edges currently attached to a directory.
pub(super) fn child_count(index: &MftIndex, directory_frs: u64) -> usize {
    let mut count = 0;
    let mut child_idx = index.records[record_idx(index, directory_frs)].first_child;
    while child_idx != NO_ENTRY {
        count += 1;
        child_idx = index.children[child_idx as usize].next_entry;
    }
    count
}
