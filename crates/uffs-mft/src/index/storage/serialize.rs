// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

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
    pub fn serialize(
        &self,
        volume_serial: u64,
        usn_journal_id: u64,
        next_usn: crate::usn::Usn,
    ) -> Vec<u8> {
        let header = IndexHeader::new(self, volume_serial, usn_journal_id, next_usn);

        // Exact capacity estimate — avoids reallocations during serialization.
        let ext_idx_size = self
            .extension_index
            .as_ref()
            .map_or(4, |ei| 4 + ei.offsets.len() * 4 + ei.postings.len() * 4);
        let estimated_size = 128 // header
            + 8 + self.frs_to_idx.len() * 4
            + self.records.len() * size_of::<crate::index::types::FileRecord>()
            + self.names.len()
            + self.links.len() * size_of::<crate::index::types::LinkInfo>()
            + self.streams.len() * size_of::<crate::index::types::IndexStreamInfo>()
            + self.children.len() * size_of::<crate::index::model::ChildInfo>()
            + self.extensions.len() * 20 // rough per-extension
            + ext_idx_size;

        let mut buffer = Vec::with_capacity(estimated_size);

        // Write header
        buffer.extend_from_slice(&header.magic);
        buffer.extend_from_slice(&header.version.to_le_bytes());
        // Wire format unchanged: header.volume is a DriveLetter (u8
        // ASCII byte), widened to u32-LE so older readers that did
        // `char::from_u32(...)` keep parsing the same 4 bytes.
        buffer.extend_from_slice(&u32::from(header.volume.as_byte()).to_le_bytes());
        buffer.extend_from_slice(&header.volume_serial.to_le_bytes());
        buffer.extend_from_slice(&header.usn_journal_id.to_le_bytes());
        buffer.extend_from_slice(&header.next_usn.raw().to_le_bytes());
        buffer.extend_from_slice(&header.created_at.to_le_bytes());
        buffer.extend_from_slice(&header.record_count.to_le_bytes());
        buffer.extend_from_slice(&header.names_size.to_le_bytes());
        buffer.extend_from_slice(&header.links_count.to_le_bytes());
        buffer.extend_from_slice(&header.streams_count.to_le_bytes());
        buffer.extend_from_slice(&header.children_count.to_le_bytes());
        // v12: build_epoch
        buffer.extend_from_slice(&header.build_epoch.to_le_bytes());

        // Write frs_to_idx table size and data — bulk cast
        buffer.extend_from_slice(&(self.frs_to_idx.len() as u64).to_le_bytes());
        buffer.extend_from_slice(bytemuck::cast_slice(&self.frs_to_idx));

        // v10: Records — single bulk copy via bytemuck (Pod layout).
        // Each record is 240 bytes (vs 195 in v9) but the extra 45 bytes of
        // padding compress to nearly zero with zstd.
        buffer.extend_from_slice(bytemuck::cast_slice(&self.records));

        // Names — raw bytes
        buffer.extend_from_slice(self.names.as_bytes());

        // Links — bulk copy (LinkInfo is Pod, 24 bytes each)
        buffer.extend_from_slice(bytemuck::cast_slice(&self.links));

        // Streams — bulk copy (IndexStreamInfo is Pod, 32 bytes each)
        buffer.extend_from_slice(bytemuck::cast_slice(&self.streams));

        // v11: Children — bulk copy (ChildInfo is now Pod, 24 bytes each)
        buffer.extend_from_slice(bytemuck::cast_slice(&self.children));

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

        // ─── v10: ExtensionIndex CSR ──────────────────────────────────
        // Ensure the extension index is built before serializing.
        // Callers should have called build_extension_index() already.
        if let Some(ext_idx) = &self.extension_index {
            let offsets_count = len_to_u32(ext_idx.offsets.len());
            buffer.extend_from_slice(&offsets_count.to_le_bytes());
            // Bulk cast u32 slices — same LE layout, no per-element overhead
            buffer.extend_from_slice(bytemuck::cast_slice(&ext_idx.offsets));
            buffer.extend_from_slice(bytemuck::cast_slice(&ext_idx.postings));
        } else {
            // No extension index — write zero count.
            buffer.extend_from_slice(&0_u32.to_le_bytes());
        }

        buffer
    }
}
