//! Helper functions for the direct-to-index parser.
//!
//! These helpers reduce code duplication in the main parser while maintaining
//! performance through inlining.

// These are NTFS index helpers - casts are intentional and bounds-checked.
#![expect(
    clippy::cast_possible_truncation,
    reason = "NTFS index sizes fit in u32; bounds verified before use"
)]
#![expect(
    clippy::if_not_else,
    reason = "!= NO_ENTRY is clearer for sentinel value checks"
)]

use crate::index::{
    ChildInfo, IndexNameRef, IndexStreamInfo, InternalStreamInfo, LinkInfo, MftIndex, SizeInfo,
    NO_ENTRY,
};

/// Adds a stream to the index and returns its index.
#[inline]
pub fn add_stream_to_index(
    index: &mut MftIndex,
    stream_name: &str,
    stream_size: u64,
    stream_allocated: u64,
) -> u32 {
    let stream_name_offset = index.add_name(stream_name);
    let stream_name_len = stream_name.len();
    let stream_is_ascii = stream_name.is_ascii();
    let extension_id = index.intern_extension(stream_name);
    let stream_name_ref = IndexNameRef::new(
        stream_name_offset,
        stream_name_len as u16,
        stream_is_ascii,
        extension_id,
    );

    let stream_idx = index.streams.len() as u32;
    index.streams.push(IndexStreamInfo {
        size: SizeInfo {
            length: stream_size,
            allocated: stream_allocated,
        },
        next_entry: NO_ENTRY,
        name: stream_name_ref,
        // type_name_id=8 for $DATA (0x80 >> 4), stored in bits 2-7
        flags: 8 << 2,
    });
    stream_idx
}

/// Result of building an internal stream chain.
pub struct InternalStreamChain {
    /// First index in the chain, or `NO_ENTRY` if empty.
    pub first: u32,
    /// Total size of all internal streams.
    pub size_total: u64,
    /// Total allocated size of all internal streams.
    pub alloc_total: u64,
}

/// Builds an internal stream chain from size/allocated pairs.
#[inline]
pub fn build_internal_stream_chain<I>(index: &mut MftIndex, streams: I) -> InternalStreamChain
where
    I: IntoIterator<Item = (u64, u64)>,
{
    let mut size_total = 0_u64;
    let mut alloc_total = 0_u64;
    let mut first = NO_ENTRY;
    let mut last = NO_ENTRY;

    for (ist_size, ist_allocated) in streams {
        size_total = size_total.saturating_add(ist_size);
        alloc_total = alloc_total.saturating_add(ist_allocated);
        let new_idx = index.internal_streams.len() as u32;
        index.internal_streams.push(InternalStreamInfo {
            size: SizeInfo {
                length: ist_size,
                allocated: ist_allocated,
            },
            next_entry: NO_ENTRY,
            flags: 0,
        });
        if last == NO_ENTRY {
            first = new_idx;
        } else {
            index.internal_streams[last as usize].next_entry = new_idx;
        }
        last = new_idx;
    }

    InternalStreamChain {
        first,
        size_total,
        alloc_total,
    }
}

/// Chains stream indices together and returns the first index.
#[inline]
pub fn chain_streams(index: &mut MftIndex, stream_indices: &[u32]) {
    for i in 0..stream_indices.len().saturating_sub(1) {
        let current_idx = stream_indices[i] as usize;
        let next_idx = stream_indices[i + 1];
        index.streams[current_idx].next_entry = next_idx;
    }
}

/// Chains link indices together.
#[inline]
pub fn chain_links(index: &mut MftIndex, link_indices: &[u32]) {
    for i in 0..link_indices.len().saturating_sub(1) {
        let current_idx = link_indices[i] as usize;
        let next_idx = link_indices[i + 1];
        index.links[current_idx].next_entry = next_idx;
    }
}

/// Adds a link to the index and returns its index.
#[inline]
pub fn add_link_to_index(index: &mut MftIndex, link_name: &str, link_parent: u64) -> u32 {
    let link_offset = index.add_name(link_name);
    let link_len = link_name.len();
    let link_is_ascii = link_name.is_ascii();
    let extension_id = index.intern_extension(link_name);
    let link_name_ref = IndexNameRef::new(link_offset, link_len as u16, link_is_ascii, extension_id);

    let link_idx = index.links.len() as u32;
    index.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: link_name_ref,
        parent_frs: link_parent,
    });
    link_idx
}

/// Adds a child entry to a parent record for tree metrics computation.
#[inline]
pub fn add_child_entry(index: &mut MftIndex, parent_frs: u64, child_frs: u64, name_idx: u16) {
    if parent_frs == child_frs || parent_frs == 0 || parent_frs == u64::from(NO_ENTRY) {
        return;
    }

    // Ensure parent exists
    let parent_idx = {
        let p_frs_usize = parent_frs as usize;
        if p_frs_usize >= index.frs_to_idx.len() {
            index.frs_to_idx.resize(p_frs_usize + 1, NO_ENTRY);
        }
        if index.frs_to_idx[p_frs_usize] == NO_ENTRY {
            let new_idx = index.records.len() as u32;
            index.frs_to_idx[p_frs_usize] = new_idx;
            index.records.push(crate::index::FileRecord::new(parent_frs));
        }
        index.frs_to_idx[p_frs_usize]
    };

    // Add child entry
    let child_idx = index.children.len() as u32;
    let parent = &mut index.records[parent_idx as usize];
    let old_first_child = parent.first_child;
    parent.first_child = child_idx;

    index.children.push(ChildInfo {
        next_entry: old_first_child,
        child_frs,
        name_index: name_idx,
    });
}

/// Data snapshot from an extension record that needs to be merged into the base.
pub struct ExtensionSnapshot {
    /// Head of the extension's stream chain.
    pub stream_head: u32,
    /// Number of additional streams from extension (excluding default).
    pub stream_count: u16,
    /// Total extra count from extension (excluding default).
    pub total_extra: u16,
    /// Head of the extension's name chain.
    pub name_next: u32,
    /// Number of names from extension.
    pub name_count: u16,
    /// Head of the extension's internal stream chain.
    pub internal_head: u32,
    /// Size of internal streams from extension.
    pub internal_size: u64,
    /// Allocated size of internal streams from extension.
    pub internal_alloc: u64,
    /// Default stream length from extension.
    pub first_stream_len: u64,
    /// Default stream allocated from extension.
    pub first_stream_alloc: u64,
}

/// Merges extension streams into the base record's stream chain.
#[inline]
pub fn merge_extension_streams(
    index: &mut MftIndex,
    frs: u64,
    base_stream_tail: Option<u32>,
    first_internal: u32,
    ext: &ExtensionSnapshot,
) {
    // Merge user-visible streams
    if ext.stream_count > 0 {
        let tail = base_stream_tail.unwrap_or(NO_ENTRY);
        if tail != NO_ENTRY {
            index.streams[tail as usize].next_entry = ext.stream_head;
        } else {
            let record = index.get_or_create(frs);
            record.first_stream.next_entry = ext.stream_head;
        }
        let record = index.get_or_create(frs);
        record.stream_count += ext.stream_count;
        record.total_stream_count += ext.stream_count;
    }

    // Merge internal streams
    if ext.internal_head != NO_ENTRY {
        if first_internal != NO_ENTRY {
            let mut tail = first_internal;
            while index.internal_streams[tail as usize].next_entry != NO_ENTRY {
                tail = index.internal_streams[tail as usize].next_entry;
            }
            index.internal_streams[tail as usize].next_entry = ext.internal_head;
        } else {
            let record = index.get_or_create(frs);
            record.first_internal_stream = ext.internal_head;
        }
        let record = index.get_or_create(frs);
        record.internal_streams_size += ext.internal_size;
        record.internal_streams_allocated += ext.internal_alloc;
        record.total_stream_count += ext.total_extra.saturating_sub(ext.stream_count);
    }
}

/// Merges extension names into the base record's name chain.
#[inline]
pub fn merge_extension_names(
    index: &mut MftIndex,
    frs: u64,
    base_name_tail: Option<u32>,
    ext: &ExtensionSnapshot,
) {
    if ext.name_count > 0 {
        let tail = base_name_tail.unwrap_or(NO_ENTRY);
        if tail != NO_ENTRY {
            index.links[tail as usize].next_entry = ext.name_next;
        } else {
            let record = index.get_or_create(frs);
            record.first_name.next_entry = ext.name_next;
        }
        let record = index.get_or_create(frs);
        record.name_count += ext.name_count;
    }
}

