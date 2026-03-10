//! NTFS boot-sector layout and helpers.

use core::mem::size_of;

/// NTFS Boot Sector structure.
///
/// Located at the first sector of an NTFS volume, contains critical
/// filesystem parameters needed to locate and read the MFT.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NtfsBootSector {
    /// Jump instruction (3 bytes).
    pub jump: [u8; 3],
    /// OEM identifier ("NTFS    ").
    pub oem_id: [u8; 8],
    /// Bytes per sector (usually 512).
    pub bytes_per_sector: u16,
    /// Sectors per cluster.
    pub sectors_per_cluster: u8,
    /// Reserved sectors (unused in NTFS).
    pub reserved_sectors: u16,
    /// Padding (always 0).
    pub padding1: [u8; 3],
    /// Unused.
    pub unused1: u16,
    /// Media descriptor.
    pub media_descriptor: u8,
    /// Padding.
    pub padding2: u16,
    /// Sectors per track.
    pub sectors_per_track: u16,
    /// Number of heads.
    pub number_of_heads: u16,
    /// Hidden sectors.
    pub hidden_sectors: u32,
    /// Unused.
    pub unused2: u32,
    /// Unused.
    pub unused3: u32,
    /// Total sectors on volume.
    pub total_sectors: i64,
    /// Logical Cluster Number of `$MFT`.
    pub mft_start_lcn: i64,
    /// Logical Cluster Number of `$MFTMirr`.
    pub mft_mirror_start_lcn: i64,
    /// Clusters per File Record Segment (can be negative for byte shift).
    pub clusters_per_file_record: i8,
    /// Padding.
    pub padding3: [u8; 3],
    /// Clusters per Index Block.
    pub clusters_per_index_block: u32,
    /// Volume serial number.
    pub volume_serial_number: i64,
    /// Checksum.
    pub checksum: u32,
    /// Bootstrap code.
    pub bootstrap: [u8; 0x200 - 0x54],
}

impl NtfsBootSector {
    /// Validates that this is a valid NTFS boot sector.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check OEM ID starts with "NTFS"
        self.oem_id[0..4] == *b"NTFS"
    }

    /// Returns the cluster size in bytes.
    #[must_use]
    pub fn cluster_size(&self) -> u32 {
        u32::from(self.sectors_per_cluster) * u32::from(self.bytes_per_sector)
    }

    /// Returns the file record size in bytes.
    ///
    /// If `clusters_per_file_record` is positive, it's the number of clusters.
    /// If negative, the size is `2^(-clusters_per_file_record)` bytes.
    #[must_use]
    pub fn file_record_size(&self) -> u32 {
        if self.clusters_per_file_record >= 0 {
            #[expect(clippy::cast_sign_loss, reason = "checked positive above")]
            let clusters = self.clusters_per_file_record as u32;
            clusters * self.cluster_size()
        } else {
            #[expect(clippy::cast_sign_loss, reason = "negated negative value is positive")]
            let shift = (-self.clusters_per_file_record) as u32;
            1_u32 << shift
        }
    }

    /// Returns the byte offset of the MFT on the volume.
    #[must_use]
    pub fn mft_byte_offset(&self) -> u64 {
        #[expect(
            clippy::cast_sign_loss,
            reason = "MFT start LCN is always non-negative"
        )]
        let lcn = self.mft_start_lcn as u64;
        lcn * u64::from(self.cluster_size())
    }
}

#[expect(
    clippy::missing_assert_message,
    reason = "compile-time size checks; messages not needed"
)]
const _: () = {
    assert!(size_of::<NtfsBootSector>() == 512);
};
