//! NTFS non-resident attribute data-run parsing.

use core::mem::size_of;

use super::records::{
    AttributeRecordHeader, NonResidentAttributeData, parse_attribute_record_header,
    parse_non_resident_attribute_data,
};

/// A single data run (extent) from a non-resident attribute.
///
/// Data runs describe the physical layout of non-resident attribute data
/// on disk. Each run specifies a contiguous range of clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataRun {
    /// Virtual Cluster Number - offset within the attribute data.
    pub vcn: i64,
    /// Number of clusters in this run.
    pub cluster_count: u64,
    /// Logical Cluster Number - physical location on disk.
    /// A value of 0 indicates a sparse (unallocated) run.
    pub lcn: i64,
}

impl DataRun {
    /// Returns true if this is a sparse (unallocated) run.
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.lcn == 0
    }

    /// Returns the byte offset of this run on the volume.
    #[must_use]
    #[expect(clippy::cast_sign_loss, reason = "lcn is checked positive before cast")]
    pub fn byte_offset(&self, bytes_per_cluster: u32) -> u64 {
        if self.lcn <= 0 {
            0
        } else {
            self.lcn as u64 * u64::from(bytes_per_cluster)
        }
    }

    /// Returns the size of this run in bytes.
    #[must_use]
    pub fn byte_size(&self, bytes_per_cluster: u32) -> u64 {
        self.cluster_count * u64::from(bytes_per_cluster)
    }
}

/// Parses data runs (mapping pairs) from a non-resident attribute.
#[must_use]
#[expect(clippy::similar_names, reason = "vcn and lcn are standard NTFS terms")]
pub fn parse_data_runs(data: &[u8], lowest_vcn: i64) -> Vec<DataRun> {
    let mut runs = Vec::new();
    let mut offset = 0;
    let mut current_vcn = lowest_vcn;
    let mut current_lcn: i64 = 0;

    while let Some(&header) = data.get(offset) {
        if header == 0 {
            break;
        }

        let length_size = (header & 0x0F) as usize;
        let offset_size = ((header >> 4_i32) & 0x0F) as usize;
        offset += 1;

        let Some(length_bytes) = data.get(offset..offset + length_size) else {
            break;
        };
        let run_length = parse_variable_length_unsigned(length_bytes);
        offset += length_size;

        let lcn_delta = if offset_size > 0 {
            let Some(offset_bytes) = data.get(offset..offset + offset_size) else {
                break;
            };
            parse_variable_length_signed(offset_bytes)
        } else {
            0
        };
        offset += offset_size;
        current_lcn += lcn_delta;

        runs.push(DataRun {
            vcn: current_vcn,
            cluster_count: run_length,
            lcn: if offset_size > 0 { current_lcn } else { 0 },
        });

        #[expect(
            clippy::cast_possible_wrap,
            reason = "cluster counts are small enough to fit in i64"
        )]
        {
            current_vcn += run_length as i64;
        }
    }

    runs
}

#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity alongside parse_variable_length_signed"
)]
/// Parses a little-endian variable-length unsigned integer from a data run
/// field.
fn parse_variable_length_unsigned(data: &[u8]) -> u64 {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().enumerate() {
        value |= u64::from(byte) << (i * 8);
    }
    value
}

#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity alongside parse_variable_length_unsigned"
)]
/// Parses a sign-extended little-endian variable-length integer from a data run
/// field.
fn parse_variable_length_signed(data: &[u8]) -> i64 {
    let Some(&last_byte) = data.last() else {
        return 0;
    };

    let mut value: i64 = 0;
    for (idx, &byte) in data.iter().enumerate() {
        value |= i64::from(byte) << (idx * 8);
    }

    if last_byte & 0x80 != 0 {
        let shift = data.len() * 8;
        if shift < 64 {
            value |= !0_i64 << shift;
        }
    }

    value
}

/// Extracts data runs from a non-resident attribute record.
#[must_use]
pub fn extract_data_runs_from_attribute(attr_data: &[u8]) -> Vec<DataRun> {
    let header_size = size_of::<AttributeRecordHeader>();
    let Some(header_slice) = attr_data.get(0..header_size) else {
        return Vec::new();
    };

    let Some(header) = parse_attribute_record_header(header_slice) else {
        return Vec::new();
    };
    if header.is_non_resident == 0 {
        return Vec::new();
    }

    let nr_size = size_of::<NonResidentAttributeData>();
    let Some(nr_slice) = attr_data.get(header_size..header_size + nr_size) else {
        return Vec::new();
    };

    let Some(nr_data) = parse_non_resident_attribute_data(nr_slice) else {
        return Vec::new();
    };
    let mapping_pairs_offset = nr_data.mapping_pairs_offset as usize;
    let Some(mapping_pairs_data) = attr_data.get(mapping_pairs_offset..) else {
        return Vec::new();
    };

    parse_data_runs(mapping_pairs_data, nr_data.lowest_vcn)
}
