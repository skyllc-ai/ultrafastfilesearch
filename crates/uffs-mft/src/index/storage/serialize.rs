//! Binary serialization for `MftIndex` snapshots.

use super::IndexHeader;
use crate::index::{MftIndex, len_to_u16, len_to_u32};

impl MftIndex {
    /// Serializes the index to a byte vector.
    ///
    /// Format:
    /// - Header (fixed size)
    /// - `frs_to_idx` table (u32 array)
    /// - records (`FileRecord` array)
    /// - names (UTF-8 string)
    /// - links (`LinkInfo` array)
    /// - streams (`IndexStreamInfo` array)
    /// - children (`ChildInfo` array)
    ///
    /// # Arguments
    ///
    /// * `volume_serial` - Volume serial number for validation
    /// * `usn_journal_id` - USN Journal ID at time of serialization
    /// * `next_usn` - Next USN to read from (checkpoint)
    #[must_use]
    pub fn serialize(&self, volume_serial: u64, usn_journal_id: u64, next_usn: i64) -> Vec<u8> {
        let header = IndexHeader::new(self, volume_serial, usn_journal_id, next_usn);

        // Estimate size (rough estimate for capacity)
        let estimated_size = 128 // header
            + self.frs_to_idx.len() * 4
            + self.records.len() * 128 // rough estimate per record
            + self.names.len()
            + self.links.len() * 24
            + self.streams.len() * 32
            + self.children.len() * 16;

        let mut buffer = Vec::with_capacity(estimated_size);

        // Write header
        buffer.extend_from_slice(&header.magic);
        buffer.extend_from_slice(&header.version.to_le_bytes());
        buffer.extend_from_slice(&(header.volume as u32).to_le_bytes());
        buffer.extend_from_slice(&header.volume_serial.to_le_bytes());
        buffer.extend_from_slice(&header.usn_journal_id.to_le_bytes());
        buffer.extend_from_slice(&header.next_usn.to_le_bytes());
        buffer.extend_from_slice(&header.created_at.to_le_bytes());
        buffer.extend_from_slice(&header.record_count.to_le_bytes());
        buffer.extend_from_slice(&header.names_size.to_le_bytes());
        buffer.extend_from_slice(&header.links_count.to_le_bytes());
        buffer.extend_from_slice(&header.streams_count.to_le_bytes());
        buffer.extend_from_slice(&header.children_count.to_le_bytes());

        // Write frs_to_idx table size and data
        buffer.extend_from_slice(&(self.frs_to_idx.len() as u64).to_le_bytes());
        for &idx in &self.frs_to_idx {
            buffer.extend_from_slice(&idx.to_le_bytes());
        }

        // Write records
        for record in &self.records {
            // FileRecord fields
            buffer.extend_from_slice(&record.frs.to_le_bytes());
            // Version 4+: sequence_number and namespace
            buffer.extend_from_slice(&record.sequence_number.to_le_bytes());
            buffer.push(record.namespace);
            buffer.push(record.forensic_flags); // Version 7: renamed from reserved
            // Version 5+: LSN (Log File Sequence Number)
            buffer.extend_from_slice(&record.lsn.to_le_bytes());
            // Version 6+: reparse_tag
            buffer.extend_from_slice(&record.reparse_tag.to_le_bytes());
            // Version 7+: base_frs for extension records
            buffer.extend_from_slice(&record.base_frs.to_le_bytes());
            // StandardInfo
            buffer.extend_from_slice(&record.stdinfo.created.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.modified.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.accessed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.mft_changed.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.flags.to_le_bytes());
            // Version 5+: NTFS 3.0+ forensic fields
            buffer.extend_from_slice(&record.stdinfo.usn.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.security_id.to_le_bytes());
            buffer.extend_from_slice(&record.stdinfo.owner_id.to_le_bytes());
            // Counts
            buffer.extend_from_slice(&record.name_count.to_le_bytes());
            buffer.extend_from_slice(&record.stream_count.to_le_bytes());
            // Version 8+: total_stream_count for full tree-metrics accounting
            buffer.extend_from_slice(&record.total_stream_count.to_le_bytes());
            buffer.extend_from_slice(&record.first_child.to_le_bytes());
            // first_name (LinkInfo)
            buffer.extend_from_slice(&record.first_name.next_entry.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.name.offset.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.name.meta.to_le_bytes());
            buffer.extend_from_slice(&record.first_name.parent_frs.to_le_bytes());
            // first_stream (IndexStreamInfo)
            buffer.extend_from_slice(&record.first_stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.name.meta.to_le_bytes());
            buffer.extend_from_slice(&record.first_stream.flags.to_le_bytes());
            // Tree metrics (Version 3+)
            buffer.extend_from_slice(&record.descendants.to_le_bytes());
            buffer.extend_from_slice(&record.treesize.to_le_bytes());
            buffer.extend_from_slice(&record.tree_allocated.to_le_bytes());
            // $FILE_NAME timestamps (Version 4+)
            buffer.extend_from_slice(&record.fn_created.to_le_bytes());
            buffer.extend_from_slice(&record.fn_modified.to_le_bytes());
            buffer.extend_from_slice(&record.fn_accessed.to_le_bytes());
            buffer.extend_from_slice(&record.fn_mft_changed.to_le_bytes());
        }

        // Write names
        buffer.extend_from_slice(self.names.as_bytes());

        // Write links (overflow links, not first_name)
        for link in &self.links {
            buffer.extend_from_slice(&link.next_entry.to_le_bytes());
            buffer.extend_from_slice(&link.name.offset.to_le_bytes());
            buffer.extend_from_slice(&link.name.meta.to_le_bytes());
            buffer.extend_from_slice(&link.parent_frs.to_le_bytes());
        }

        // Write streams (overflow streams, not first_stream)
        for stream in &self.streams {
            buffer.extend_from_slice(&stream.size.length.to_le_bytes());
            buffer.extend_from_slice(&stream.size.allocated.to_le_bytes());
            buffer.extend_from_slice(&stream.next_entry.to_le_bytes());
            buffer.extend_from_slice(&stream.name.offset.to_le_bytes());
            buffer.extend_from_slice(&stream.name.meta.to_le_bytes());
            buffer.extend_from_slice(&stream.flags.to_le_bytes());
        }

        // Write children
        for child in &self.children {
            buffer.extend_from_slice(&child.next_entry.to_le_bytes());
            buffer.extend_from_slice(&child.child_frs.to_le_bytes());
            buffer.extend_from_slice(&child.name_index.to_le_bytes());
        }

        // Write ExtensionTable
        // Extension count (u32)
        let ext_count = len_to_u32(self.extensions.len());
        buffer.extend_from_slice(&ext_count.to_le_bytes());

        // For each extension (starting from index 1, since 0 is NO_EXTENSION)
        for i in 1..self.extensions.len() {
            // i is bounded by extensions.len() which is u16-based
            let ext_id = len_to_u16(i);
            if let Some(ext_str) = self.extensions.get_extension(ext_id) {
                let ext_bytes = ext_str.as_bytes();
                let count = self.extensions.get_count(ext_id);
                let bytes = self.extensions.get_bytes(ext_id);

                // String length (u32)
                let str_len = len_to_u32(ext_bytes.len());
                buffer.extend_from_slice(&str_len.to_le_bytes());
                // String bytes
                buffer.extend_from_slice(ext_bytes);
                // Count (u32)
                buffer.extend_from_slice(&count.to_le_bytes());
                // Bytes (u64)
                buffer.extend_from_slice(&bytes.to_le_bytes());
            }
        }

        buffer
    }
}
