//! Reader statistics and progress helper types.

use core::time::Duration;

#[cfg(windows)]
use tracing::{debug, info};

/// Statistics computed during MFT parsing and `DataFrame` building.
///
/// This struct is populated during the single-pass DF build loop,
/// eliminating the need for a separate statistics pass.
#[derive(Debug, Clone, Default)]
pub struct MftStats {
    /// Number of directory records.
    pub dir_count: u64,
    /// Number of file records.
    pub file_count: u64,
    /// Number of hidden files/directories.
    pub hidden_count: u64,
    /// Number of system files/directories.
    pub system_count: u64,
    /// Number of compressed files.
    pub compressed_count: u64,
    /// Number of encrypted files.
    pub encrypted_count: u64,
    /// Number of sparse files.
    pub sparse_count: u64,
    /// Number of reparse points.
    pub reparse_count: u64,
    /// Number of files with multiple data streams.
    pub multi_stream_count: u64,
    /// Number of files with multiple names.
    pub multi_name_count: u64,
    /// Total logical file size in bytes.
    pub total_file_size: u64,
    /// Total allocated size in bytes.
    pub total_allocated_size: u64,
}

impl MftStats {
    /// Returns the slack space (allocated - logical size).
    #[must_use]
    pub const fn slack_space(&self) -> u64 {
        self.total_allocated_size
            .saturating_sub(self.total_file_size)
    }

    /// Returns the slack percentage (0.0 to 100.0).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for progress display"
    )]
    #[expect(
        clippy::float_arithmetic,
        reason = "float arithmetic required for percentage calculation"
    )]
    pub fn slack_percentage(&self) -> f64 {
        if self.total_allocated_size > 0 {
            (self.slack_space() as f64 / self.total_allocated_size as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Logs the aggregated statistics summary.
    #[cfg(windows)]
    pub(super) fn log_summary(&self) {
        info!(
            directories = self.dir_count,
            files = self.file_count,
            "📊 Record type breakdown"
        );

        info!(
            hidden = self.hidden_count,
            system = self.system_count,
            compressed = self.compressed_count,
            encrypted = self.encrypted_count,
            sparse = self.sparse_count,
            reparse_points = self.reparse_count,
            "🏷️  Attribute flags summary"
        );

        if self.multi_stream_count > 0 || self.multi_name_count > 0 {
            info!(
                files_with_ads = self.multi_stream_count,
                files_with_hardlinks = self.multi_name_count,
                "🔗 Extended attributes"
            );
        }

        debug!(
            total_file_size_gb = format!(
                "{:.2}",
                self.total_file_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            total_allocated_gb = format!(
                "{:.2}",
                self.total_allocated_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            slack_space_mb = format!("{:.2}", self.slack_space() as f64 / (1024.0 * 1024.0)),
            slack_percentage = format!("{:.1}%", self.slack_percentage()),
            "💾 Storage analysis"
        );
    }
}

/// Progress information during MFT reading.
#[derive(Debug, Clone)]
pub struct MftProgress {
    /// Number of records read so far.
    pub records_read: u64,
    /// Total number of records (if known).
    pub total_records: Option<u64>,
    /// Bytes read from disk.
    pub bytes_read: u64,
    /// Time elapsed since start.
    pub elapsed: Duration,
}

impl MftProgress {
    /// Returns the percentage complete (0.0 to 100.0), if total is known.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for progress display"
    )]
    #[expect(
        clippy::float_arithmetic,
        reason = "float arithmetic required for percentage calculation"
    )]
    pub fn percentage(&self) -> Option<f64> {
        self.total_records
            .map(|total| (self.records_read as f64 / total as f64) * 100.0_f64)
    }

    /// Returns the read speed in MB/s.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for progress display"
    )]
    #[expect(
        clippy::float_arithmetic,
        reason = "float arithmetic required for speed calculation"
    )]
    pub fn speed_mbps(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            (self.bytes_read as f64 / 1_048_576.0) / secs
        } else {
            0.0
        }
    }
}
