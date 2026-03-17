//! Bitmap helpers for interpreting the `$MFT::$BITMAP` allocation stream.

// Low-level bitmap operations
#![allow(clippy::all, clippy::nursery, clippy::pedantic)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

/// Bitmap indicating which MFT records are in use.
///
/// The `$MFT::$BITMAP` stream contains one bit per MFT record.
/// If the bit is set (1), the record is in use; if clear (0), it's free.
#[derive(Debug, Clone)]
pub struct MftBitmap {
    /// Raw bitmap data.
    data: Vec<u8>,
    /// Number of records this bitmap covers.
    record_count: usize,
}

impl MftBitmap {
    /// Creates a new bitmap from raw bytes.
    #[must_use]
    pub fn from_bytes(data: Vec<u8>) -> Self {
        let record_count = data.len() * 8;
        Self { data, record_count }
    }

    /// Creates a bitmap where all records are marked as valid.
    ///
    /// Used as a fallback when the actual bitmap cannot be read.
    #[must_use]
    pub fn new_all_valid(record_count: usize) -> Self {
        let byte_count = record_count.div_ceil(8);
        Self {
            data: vec![0xFF; byte_count],
            record_count,
        }
    }

    /// Checks if a specific record is in use.
    #[must_use]
    pub fn is_record_in_use(&self, frs: u64) -> bool {
        let frs = frs as usize;
        if frs >= self.record_count {
            return false;
        }

        let byte_index = frs / 8;
        let bit_index = frs % 8;

        if byte_index >= self.data.len() {
            return false;
        }

        (self.data[byte_index] & (1 << bit_index)) != 0
    }

    /// Returns the number of records marked as in use.
    #[must_use]
    pub fn count_in_use(&self) -> usize {
        self.data
            .iter()
            .map(|&byte| byte.count_ones() as usize)
            .sum()
    }

    /// Returns the highest FRS number that is marked as in use.
    ///
    /// This scans the bitmap backwards to find the last set bit.
    /// Returns 0 if no records are in use.
    #[must_use]
    pub fn max_frs_in_use(&self) -> u64 {
        // Scan backwards through bytes to find the last non-zero byte
        for (byte_idx, &byte) in self.data.iter().enumerate().rev() {
            if byte != 0 {
                // Found a non-zero byte, find the highest bit set
                let bit_idx = 7 - byte.leading_zeros() as usize;
                return (byte_idx * 8 + bit_idx) as u64;
            }
        }
        0
    }

    /// Returns the total number of records this bitmap covers.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }

    /// Returns the raw bitmap data.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Returns an iterator over the FRS numbers of records that are in use.
    pub fn in_use_records(&self) -> impl Iterator<Item = u64> + '_ {
        self.data.iter().enumerate().flat_map(|(byte_idx, &byte)| {
            (0..8).filter_map(move |bit_idx| {
                if (byte & (1 << bit_idx)) != 0 {
                    Some((byte_idx * 8 + bit_idx) as u64)
                } else {
                    None
                }
            })
        })
    }

    /// Finds the first N records that are in use, starting from a given FRS.
    pub fn find_in_use_range(&self, start_frs: u64, count: usize) -> Vec<u64> {
        let mut result = Vec::with_capacity(count);
        let start = start_frs as usize;

        for frs in start..self.record_count {
            if result.len() >= count {
                break;
            }

            let byte_index = frs / 8;
            let bit_index = frs % 8;

            if byte_index < self.data.len() && (self.data[byte_index] & (1 << bit_index)) != 0 {
                result.push(frs as u64);
            }
        }

        result
    }

    /// Calculates skip ranges for a cluster-aligned read.
    #[must_use]
    pub fn calculate_skip_range(&self, start_frs: u64, end_frs: u64) -> (u64, u64) {
        let start = start_frs as usize;
        let end = (end_frs as usize).min(self.record_count);

        if start >= end {
            return (0, 0);
        }

        let mut skip_begin = 0_u64;
        for frs in start..end {
            if self.is_record_in_use(frs as u64) {
                break;
            }
            skip_begin += 1;
        }

        if skip_begin == (end - start) as u64 {
            return (skip_begin, 0);
        }

        let mut skip_end = 0_u64;
        for frs in (start..end).rev() {
            if self.is_record_in_use(frs as u64) {
                break;
            }
            skip_end += 1;
        }

        (skip_begin, skip_end)
    }

    /// Checks if an entire cluster range has any in-use records.
    #[must_use]
    pub fn cluster_has_in_use(&self, start_frs: u64, records_per_cluster: u32) -> bool {
        let start = start_frs as usize;
        let end = (start + records_per_cluster as usize).min(self.record_count);

        let start_byte = start / 8;
        let end_byte = end.div_ceil(8);

        for byte_idx in start_byte..end_byte.min(self.data.len()) {
            let byte = self.data[byte_idx];

            let mask = if byte_idx == start_byte && start % 8 != 0 {
                0xFF_u8 << (start % 8)
            } else if byte_idx == end_byte - 1 && end % 8 != 0 {
                (1_u8 << (end % 8)) - 1
            } else {
                0xFF
            };

            if (byte & mask) != 0 {
                return true;
            }
        }

        false
    }

    /// Returns ranges of clusters that contain in-use records.
    pub fn in_use_cluster_ranges(
        &self,
        records_per_cluster: u32,
    ) -> impl Iterator<Item = (u64, u64)> + '_ {
        let total_clusters = self.record_count.div_ceil(records_per_cluster as usize);

        InUseClusterRangeIterator {
            bitmap: self,
            records_per_cluster,
            current_cluster: 0,
            total_clusters: total_clusters as u64,
        }
    }
}

/// Iterator over ranges of clusters containing in-use records.
struct InUseClusterRangeIterator<'a> {
    bitmap: &'a MftBitmap,
    records_per_cluster: u32,
    current_cluster: u64,
    total_clusters: u64,
}

impl Iterator for InUseClusterRangeIterator<'_> {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_cluster < self.total_clusters {
            let start_frs = self.current_cluster * u64::from(self.records_per_cluster);
            if self
                .bitmap
                .cluster_has_in_use(start_frs, self.records_per_cluster)
            {
                break;
            }
            self.current_cluster += 1;
        }

        if self.current_cluster >= self.total_clusters {
            return None;
        }

        let range_start = self.current_cluster;
        while self.current_cluster < self.total_clusters {
            let start_frs = self.current_cluster * u64::from(self.records_per_cluster);
            if !self
                .bitmap
                .cluster_has_in_use(start_frs, self.records_per_cluster)
            {
                break;
            }
            self.current_cluster += 1;
        }

        Some((range_start, self.current_cluster - range_start))
    }
}
