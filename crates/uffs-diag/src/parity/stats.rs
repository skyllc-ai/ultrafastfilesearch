// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Data structures for parity comparison statistics.

use std::collections::HashMap;

/// Comparison statistics for a single field.
#[derive(Debug, Default)]
pub struct FieldStats {
    /// Total number of values compared.
    pub total_compared: u64,
    /// Number of exact matches.
    pub exact_matches: u64,
    /// Number of mismatches.
    pub mismatches: u64,
    /// Values present only in reference.
    pub reference_only: u64,
    /// Values present only in Rust.
    pub rust_only: u64,
    /// Sum of absolute differences (for numeric fields).
    pub sum_abs_diff: f64,
    /// Maximum absolute difference (for numeric fields).
    pub max_abs_diff: f64,
    /// Sample differences: `(path, reference_value, rust_value)`.
    pub diff_samples: Vec<(String, String, String)>,
}

impl FieldStats {
    /// Calculate the match rate as a percentage.
    #[must_use]
    #[expect(clippy::float_arithmetic, reason = "statistics require float division")]
    pub fn match_rate(&self) -> f64 {
        if self.total_compared == 0 {
            0.0_f64
        } else {
            100.0_f64 * uffs_mft::u64_to_f64(self.exact_matches)
                / uffs_mft::u64_to_f64(self.total_compared)
        }
    }

    /// Merge another `FieldStats` into this one.
    #[expect(
        clippy::float_arithmetic,
        reason = "statistics require accumulating f64 sums"
    )]
    pub fn merge(&mut self, other: Self) {
        self.total_compared += other.total_compared;
        self.exact_matches += other.exact_matches;
        self.mismatches += other.mismatches;
        self.reference_only += other.reference_only;
        self.rust_only += other.rust_only;
        self.sum_abs_diff += other.sum_abs_diff;
        if other.max_abs_diff > self.max_abs_diff {
            self.max_abs_diff = other.max_abs_diff;
        }
        // Keep only first 10 samples total
        let remaining = 10_usize.saturating_sub(self.diff_samples.len());
        self.diff_samples
            .extend(other.diff_samples.into_iter().take(remaining));
    }
}

/// Overall comparison results between reference and Rust outputs.
#[derive(Debug, Default)]
pub struct ComparisonResults {
    /// Path to reference file.
    pub reference_file: String,
    /// Path to Rust file.
    pub rust_file: String,
    /// Total rows in reference output.
    pub reference_total_rows: usize,
    /// Total rows in Rust output.
    pub rust_total_rows: usize,
    /// Number of paths present in both outputs.
    pub common_paths: usize,
    /// Number of paths only in reference output.
    pub reference_only_paths: usize,
    /// Number of paths only in Rust output.
    pub rust_only_paths: usize,
    /// Path match rate as percentage.
    pub path_match_rate: f64,
    /// Per-field comparison statistics.
    pub field_stats: HashMap<String, FieldStats>,
    /// ADS count in reference output.
    pub reference_ads_count: usize,
    /// ADS count in Rust output.
    pub rust_ads_count: usize,
    /// Sample paths only in reference.
    pub sample_reference_only: Vec<String>,
    /// Sample paths only in Rust.
    pub sample_rust_only: Vec<String>,
}
