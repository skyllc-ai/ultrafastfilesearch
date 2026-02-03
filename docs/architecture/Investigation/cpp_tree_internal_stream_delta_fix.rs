// -----------------------------------------------------------------------------
// cpp_tree.rs (Rust) — C++ parity port
//
// Key invariants (matching C++):
//   1) Propagation channel (Channel A): parents accumulate *delta()* shares of ALL
//      streams (default + ADS + INTERNAL Windows streams like $REPARSE, $OBJECT_ID,
//      $SECURITY_DESCRIPTOR, etc).
//   2) Printed channel (Channel B): a record's own printed Size/Allocated/Descendants
//      is based on the *default stream only* (type_name_id == 0) plus the aggregated
//      child totals.
//
// IMPORTANT FIX IN THIS VERSION:
//   - Internal streams MUST be delta()'d PER STREAM, not as a single aggregated sum.
//     delta() is non-linear due to integer division floors. Grouping internal streams
//     changes remainder distribution for hardlinks and causes the final 1–12 byte
//     mismatches seen in WinSxS/WindowsApps.
// -----------------------------------------------------------------------------

use crate::{FileSizeType, MftIndex, NO_ENTRY};

#[derive(Default, Clone, Copy)]
struct PreprocessResult {
    length: u64,
    allocated: u64,
    bulkiness: u64,
    treesize: u64,
}

fn delta(value: u64, i: u16, n: u16) -> u64 {
    if n <= 1 {
        return value;
    }

    // Exact match to C++: value*(i+1)/n - value*i/n (integer divisions)
    // where i is name_info and n is total_names.
    let n = n as u64;
    let i = i as u64;
    (value * (i + 1) / n) - (value * i / n)
}

pub struct CppTree<'a> {
    index: &'a mut MftIndex,
}

impl<'a> CppTree<'a> {
    pub fn new(index: &'a mut MftIndex) -> Self {
        Self { index }
    }

    fn preprocess(&mut self, idx: u32, name_info: u16, total_names: u16) -> PreprocessResult {
        // We need an immutable snapshot of some fields while we mutate the record.
        let record = &self.index.records[idx as usize];
        let stream_count: u64 = record.total_stream_count.into();

        // --- recurse into children first (returns propagated totals) ---
        let mut children_size = PreprocessResult::default();
        let mut child_entry_idx = record.first_child;
        while child_entry_idx != NO_ENTRY {
            let child_info = &self.index.children[child_entry_idx as usize];

            let child_name_count = self.index.records[child_info.record_idx as usize].name_count;
            debug_assert!(child_name_count != 0);

            // C++ uses reverse name order: name_info = (name_count - 1 - name_index)
            // so the "primary" name tends to get the remainder bytes.
            let child_name_info = child_name_count - 1 - child_info.name_index;

            let subresult = self.preprocess(child_info.record_idx, child_name_info, child_name_count);

            children_size.length += subresult.length;
            children_size.allocated += subresult.allocated;
            children_size.bulkiness += subresult.bulkiness;
            children_size.treesize += subresult.treesize;

            child_entry_idx = child_info.next_entry;
        }

        // --- accumulate propagation (Channel A): delta shares of ALL streams ---
        let first_stream = &record.first_stream;
        let first_len = first_stream.size.length.into_u64();
        let first_alloc = first_stream.size.allocated.into_u64();
        let first_bulk = first_stream.size.bulkiness.into_u64();

        let mut result = children_size;
        result.treesize += stream_count;

        // Default stream always participates in propagation (delta)
        result.length += delta(first_len, name_info, total_names);
        result.allocated += delta(first_alloc, name_info, total_names);
        result.bulkiness += delta(first_bulk, name_info, total_names);

        // Internal (filtered) streams MUST be applied per-stream (non-linear delta)
        // NOTE: requires MftIndex + FileRecord to expose first_internal_stream + internal_streams.
        let mut internal_entry = record.first_internal_stream;
        while internal_entry != NO_ENTRY {
            let internal_idx = (internal_entry - 1) as usize;
            let st = &self.index.internal_streams[internal_idx];

            let len = st.size.length.into_u64();
            let alloc = st.size.allocated.into_u64();
            let bulk = st.size.bulkiness.into_u64();

            result.length += delta(len, name_info, total_names);
            result.allocated += delta(alloc, name_info, total_names);
            result.bulkiness += delta(bulk, name_info, total_names);

            internal_entry = st.next_entry;
        }

        // Named streams (ADS) — already stored; apply per-stream
        let mut current_stream_entry = record.first_stream.next_entry;
        let mut streams_to_visit = record.stream_count.saturating_sub(1);
        while current_stream_entry != NO_ENTRY && streams_to_visit != 0 {
            let stream_entry_idx = (current_stream_entry - 1) as usize;
            let stream_info = &self.index.streams[stream_entry_idx];

            let len = stream_info.size.length.into_u64();
            let alloc = stream_info.size.allocated.into_u64();
            let bulk = stream_info.size.bulkiness.into_u64();

            result.length += delta(len, name_info, total_names);
            result.allocated += delta(alloc, name_info, total_names);
            result.bulkiness += delta(bulk, name_info, total_names);

            current_stream_entry = stream_info.next_entry;
            streams_to_visit -= 1;
        }

        // --- store printed values (Channel B): default stream + children totals ---
        // These are what become the CSV "Size" / "Size on Disk" / "Descendants".
        let record_mut = &mut self.index.records[idx as usize];

        if record.is_directory() {
            // Directory printed size = default stream (I30) + sum(child propagated length)
            record_mut.treesize = record_mut.first_stream.size.length
                + FileSizeType::from_u64(children_size.length);

            record_mut.tree_allocated = record_mut.first_stream.size.allocated
                + FileSizeType::from_u64(children_size.allocated);

            // C++ output uses the "printed" treesize for descendants (Channel B)
            // which excludes internal streams but includes the directory default stream.
            record_mut.descendants = children_size.treesize + 1;
        } else {
            // Files: printed size/alloc are the *default stream only*.
            // ADS rows are printed separately from StreamInfo; internal streams are never printed.
            record_mut.treesize = record_mut.first_stream.size.length;
            record_mut.tree_allocated = record_mut.first_stream.size.allocated;
            record_mut.descendants = 1;
        }

        result
    }

    pub fn compute_tree_metrics(&mut self) {
        struct Traversal<'a> {
            tree: CppTree<'a>,
        }

        impl<'a> Traversal<'a> {
            fn run(&mut self) {
                // Root FRS is always 5 on NTFS
                if let Some(&root_idx) = self.tree.index.frs_to_idx.get(&5) {
                    self.tree.preprocess(root_idx, 0, 1);
                }

                // IMPORTANT FOR LIVE SCAN ROBUSTNESS:
                // If the live pipeline fails to link some directory chain back to root,
                // those nodes would otherwise keep default 0 metrics.
                // We do a best-effort pass to ensure all connected components get metrics.
                // This does NOT change root totals; it only initializes orphan subtrees.
                for i in 0..self.tree.index.records.len() {
                    if self.tree.index.records[i].descendants == 0 {
                        // name_info=0,total_names=1 is safe here because stored metrics
                        // are independent of name_info; only propagated return values vary.
                        self.tree.preprocess(i as u32, 0, 1);
                    }
                }
            }
        }

        let mut traversal = Traversal {
            tree: CppTree::new(self.index),
        };

        traversal.run();
    }
}
