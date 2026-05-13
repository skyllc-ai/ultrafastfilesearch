// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Aggregate specification types.
//!
//! An [`AggregateSpec`] describes a single aggregation operation to perform
//! during a search scan. Multiple specs can be composed to produce a rich
//! statistical profile in a single pass.

use crate::search::field::FieldId;

/// A single aggregation operation to compute during a search scan.
#[derive(Debug, Clone, Hash)]
pub struct AggregateSpec {
    /// What kind of aggregation to perform.
    pub kind: AggregateKind,
    /// Optional label for this aggregation in the output.
    pub label: Option<String>,
}

impl AggregateSpec {
    /// Create a new aggregate spec with the given kind.
    #[must_use]
    pub const fn new(kind: AggregateKind) -> Self {
        Self { kind, label: None }
    }

    /// Create a new aggregate spec with a label.
    #[must_use]
    pub(crate) fn with_label(kind: AggregateKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: Some(label.into()),
        }
    }
}

/// The kind of aggregation to compute.
#[derive(Debug, Clone, Hash)]
pub enum AggregateKind {
    /// Total count of matching records.
    Count,

    /// Statistical metrics for a numeric or timestamp field.
    Stats {
        /// Which field to compute statistics on.
        field: FieldId,
        /// Which metrics to compute (empty = all applicable).
        metrics: Vec<ScalarMetric>,
    },

    /// Group records by a field's values and compute per-group metrics.
    Terms {
        /// Which field to group by (must be `groupable`).
        field: FieldId,
        /// Maximum number of groups to return.
        top: u16,
        /// Metrics to compute per group (default: count + `total_bytes`).
        metrics: Vec<BucketMetric>,
        /// Optional sample rows per bucket.
        sample: Option<TopHitsSpec>,
    },

    /// Group records into fixed-size numeric buckets.
    Histogram {
        /// Which field to bucket (must have `bucket_support`).
        field: FieldId,
        /// Bucket interval (for numeric fields, in the field's unit).
        interval: u64,
        /// Metrics per bucket.
        metrics: Vec<BucketMetric>,
    },

    /// Group records by calendar-aligned time intervals.
    DateHistogram {
        /// Which timestamp field.
        field: FieldId,
        /// Calendar interval.
        calendar: CalendarInterval,
        /// Metrics per bucket.
        metrics: Vec<BucketMetric>,
    },

    /// Group records into explicit numeric ranges.
    Range {
        /// Which field (must have `bucket_support`).
        field: FieldId,
        /// Range boundaries (N boundaries → N+1 buckets).
        boundaries: Vec<u64>,
        /// Metrics per bucket.
        metrics: Vec<BucketMetric>,
    },

    /// Count records where a field has no value / is zero / is missing.
    Missing {
        /// Which field to check.
        field: FieldId,
    },

    /// Count distinct values for a field.
    Distinct {
        /// Which field.
        field: FieldId,
    },

    /// Rollup: group by path depth or drive, then compute sub-aggregates.
    Rollup {
        /// Rollup mode.
        mode: RollupMode,
        /// Maximum groups to return.
        top: u16,
        /// Metrics per group.
        metrics: Vec<BucketMetric>,
        /// Optional sample rows per group.
        sample: Option<TopHitsSpec>,
        /// Optional nested sub-aggregation per group (max 1 level deep in v1).
        ///
        /// When set, each top-level bucket runs a sub-aggregation on
        /// its members and the result appears in `BucketWire.drilldown`.
        sub: Option<Box<AggregateSpec>>,
    },

    /// Duplicate candidate detection.
    Duplicates {
        /// Fields to use as composite group key.
        keys: Vec<FieldId>,
        /// Verification mode.
        verify: DuplicateVerify,
        /// Maximum duplicate groups to return.
        top: u16,
        /// Sample rows per duplicate group.
        sample: Option<TopHitsSpec>,
        /// Maximum groups to track (OOM guard).
        max_groups: u32,
    },
}

/// Rollup mode for path-based or drive-based rollups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RollupMode {
    /// Group by drive letter.
    Drive,
    /// Group by path at a specific depth from drive root.
    Path {
        /// Depth from drive root (1 = top-level folder).
        depth: u32,
    },
    /// Group by the immediate children of a specific ancestor record.
    ///
    /// All files that are descendants of `record_idx` are grouped by
    /// whichever direct child of that record they descend from.
    /// Files *directly* inside the ancestor (i.e. whose parent IS
    /// the ancestor) use themselves as the group key.
    Ancestor {
        /// Record index of the ancestor to drill into.
        record_idx: u32,
    },
}

/// Duplicate verification mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DuplicateVerify {
    /// No verification — candidates only (fastest).
    None,
    /// Compare first N bytes of each file.
    FirstBytes {
        /// Bytes to compare (default: 4096).
        count: u32,
    },
    /// Full SHA-256 hash verification.
    Sha256,
}

/// Maximum allowed sample rows per bucket.
/// Maximum number of sample rows per bucket.
pub(crate) const MAX_SAMPLE_COUNT: u8 = 5;

/// Default sample projection: the fields returned for each sample row
/// when the caller doesn't specify a custom projection.
pub(crate) const DEFAULT_PROJECTION: &[FieldId] = &[
    FieldId::Name,
    FieldId::Size,
    FieldId::Modified,
    FieldId::Path,
    FieldId::Type,
    FieldId::Extension,
];

/// Specification for sample rows within a bucket.
///
/// Each bucketed aggregation (Terms, Rollup, Duplicates) can optionally
/// carry a `TopHitsSpec` that tells the accumulator to track the top-N
/// most interesting records per bucket.  During finalization, only the
/// surviving buckets have their sample rows materialized (path resolved,
/// fields projected).
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `count` | 2 |
/// | `sort_field` | `Size` |
/// | `sort_desc` | `true` (largest first) |
/// | `projection` | name, size, modified, path, type, ext |
#[derive(Debug, Clone, Hash)]
pub struct TopHitsSpec {
    /// Number of sample rows per bucket (1–`MAX_SAMPLE_COUNT`).
    pub count: u8,
    /// Sort sample rows by this field.
    pub sort_field: FieldId,
    /// Sort direction: `true` = descending (largest/newest first).
    pub sort_desc: bool,
    /// Fields to include in each sample row.
    ///
    /// When empty, `DEFAULT_PROJECTION` is used during finalization.
    pub projection: Vec<FieldId>,
}

impl Default for TopHitsSpec {
    fn default() -> Self {
        Self {
            count: 2,
            sort_field: FieldId::Size,
            sort_desc: true,
            projection: Vec::new(), // empty = use DEFAULT_PROJECTION
        }
    }
}

impl TopHitsSpec {
    /// Create a spec with just a sample count (all other fields default).
    ///
    /// `count` is clamped to 1–`MAX_SAMPLE_COUNT`.
    #[must_use]
    pub fn with_count(count: u8) -> Self {
        Self {
            count: count.clamp(1, MAX_SAMPLE_COUNT),
            ..Self::default()
        }
    }

    /// Create a fully specified `TopHitsSpec`.
    ///
    /// `count` is clamped to 1–`MAX_SAMPLE_COUNT`.
    #[must_use]
    pub fn new(count: u8, sort_field: FieldId, sort_desc: bool, projection: Vec<FieldId>) -> Self {
        Self {
            count: count.clamp(1, MAX_SAMPLE_COUNT),
            sort_field,
            sort_desc,
            projection,
        }
    }

    /// Return the effective projection — custom if non-empty, otherwise
    /// the default compact set.
    #[must_use]
    pub(crate) fn effective_projection(&self) -> &[FieldId] {
        if self.projection.is_empty() {
            DEFAULT_PROJECTION
        } else {
            &self.projection
        }
    }

    /// Validate the spec against field metadata.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a human-readable message if the sort field is
    /// not numeric/timestamp (i.e. cannot be meaningfully ordered), or if
    /// `count` is zero.
    pub fn validate(&self) -> Result<(), String> {
        if self.count == 0 {
            return Err("TopHitsSpec count must be ≥ 1".to_owned());
        }
        if self.count > MAX_SAMPLE_COUNT {
            return Err(format!(
                "TopHitsSpec count {} exceeds maximum {}",
                self.count, MAX_SAMPLE_COUNT
            ));
        }
        // sort_field should be something orderable — we accept any field
        // that the sort pipeline accepts (numeric, timestamp, boolean).
        let meta = self.sort_field.metadata();
        if !meta.sortable {
            return Err(format!(
                "TopHitsSpec sort_field {:?} is not sortable",
                self.sort_field
            ));
        }
        Ok(())
    }
}

/// A scalar metric computed over a set of records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarMetric {
    /// Sum of values.
    Sum,
    /// Minimum value.
    Min,
    /// Maximum value.
    Max,
    /// Arithmetic mean.
    Avg,
    /// Count of records with a value for this field.
    ValueCount,
    /// Count of records missing a value for this field.
    MissingCount,
}

/// A metric computed per bucket/group in a terms, histogram, or range
/// aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BucketMetric {
    /// Number of records in the bucket.
    Count,
    /// Total logical size (sum of `size`).
    TotalBytes,
    /// Total allocated size (sum of `allocated`).
    TotalAllocated,
    /// Waste: `total_allocated - total_bytes`.
    WasteBytes,
    /// Waste percentage: `waste / total_allocated * 100`.
    WastePct,
    /// Average file size in this bucket.
    AvgSize,
    /// Minimum file size in this bucket.
    MinSize,
    /// Maximum file size in this bucket.
    MaxSize,
    /// Share of total record count (percentage).
    ShareOfTotalCount,
    /// Share of total bytes (percentage).
    ShareOfTotalBytes,
}

/// Calendar-aligned time intervals for date histogram aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalendarInterval {
    /// One hour.
    Hour,
    /// One day.
    Day,
    /// One ISO week (Monday-based).
    Week,
    /// One calendar month.
    Month,
    /// One calendar quarter (3 months).
    Quarter,
    /// One calendar year.
    Year,
}

impl CalendarInterval {
    /// Parse a calendar interval from a string.
    ///
    /// # Errors
    ///
    /// Returns `None` if the string is not a recognized interval.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        match input.to_ascii_lowercase().as_str() {
            "hour" | "h" | "hourly" => Some(Self::Hour),
            "day" | "d" | "daily" => Some(Self::Day),
            "week" | "w" | "weekly" => Some(Self::Week),
            "month" | "m" | "monthly" => Some(Self::Month),
            "quarter" | "q" | "quarterly" => Some(Self::Quarter),
            "year" | "y" | "yearly" | "annual" => Some(Self::Year),
            _ => None,
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests assert against fixtures with known shape; indexing panic = test failure"
)]
mod tests {
    use super::*;

    #[test]
    fn count_spec() {
        let spec = AggregateSpec::new(AggregateKind::Count);
        assert!(spec.label.is_none());
        assert!(matches!(spec.kind, AggregateKind::Count));
    }

    #[test]
    fn stats_spec() {
        let spec = AggregateSpec::with_label(
            AggregateKind::Stats {
                field: FieldId::Size,
                metrics: vec![ScalarMetric::Sum, ScalarMetric::Avg],
            },
            "size_stats",
        );
        assert_eq!(spec.label.as_deref(), Some("size_stats"));
    }

    #[test]
    fn terms_spec() {
        let spec = AggregateSpec::new(AggregateKind::Terms {
            field: FieldId::Extension,
            top: 50,
            metrics: vec![BucketMetric::Count, BucketMetric::TotalBytes],
            sample: None,
        });
        if let AggregateKind::Terms { field, top, .. } = &spec.kind {
            assert_eq!(*field, FieldId::Extension);
            assert_eq!(*top, 50);
        } else {
            panic!("expected Terms");
        }
    }

    #[test]
    fn calendar_interval_parse() {
        assert_eq!(
            CalendarInterval::parse("month"),
            Some(CalendarInterval::Month)
        );
        assert_eq!(CalendarInterval::parse("M"), Some(CalendarInterval::Month));
        assert_eq!(
            CalendarInterval::parse("yearly"),
            Some(CalendarInterval::Year)
        );
        assert_eq!(
            CalendarInterval::parse("hourly"),
            Some(CalendarInterval::Hour)
        );
        assert_eq!(
            CalendarInterval::parse("weekly"),
            Some(CalendarInterval::Week)
        );
        assert_eq!(
            CalendarInterval::parse("quarterly"),
            Some(CalendarInterval::Quarter)
        );
        assert_eq!(
            CalendarInterval::parse("daily"),
            Some(CalendarInterval::Day)
        );
        assert!(CalendarInterval::parse("millennium").is_none());
    }

    #[test]
    fn all_scalar_metrics() {
        // Ensure all variants are distinct.
        let all = [
            ScalarMetric::Sum,
            ScalarMetric::Min,
            ScalarMetric::Max,
            ScalarMetric::Avg,
            ScalarMetric::ValueCount,
            ScalarMetric::MissingCount,
        ];
        for (i, lhs) in all.iter().enumerate() {
            for (j, rhs) in all.iter().enumerate() {
                assert_eq!(i == j, lhs == rhs);
            }
        }
    }

    #[test]
    fn all_bucket_metrics() {
        let all = [
            BucketMetric::Count,
            BucketMetric::TotalBytes,
            BucketMetric::TotalAllocated,
            BucketMetric::WasteBytes,
            BucketMetric::WastePct,
            BucketMetric::AvgSize,
            BucketMetric::MinSize,
            BucketMetric::MaxSize,
            BucketMetric::ShareOfTotalCount,
            BucketMetric::ShareOfTotalBytes,
        ];
        for (i, lhs) in all.iter().enumerate() {
            for (j, rhs) in all.iter().enumerate() {
                assert_eq!(i == j, lhs == rhs);
            }
        }
    }

    #[test]
    fn range_spec() {
        let spec = AggregateSpec::new(AggregateKind::Range {
            field: FieldId::Size,
            boundaries: vec![1024, 1_048_576, 1_073_741_824],
            metrics: vec![BucketMetric::Count],
        });
        if let AggregateKind::Range { boundaries, .. } = &spec.kind {
            assert_eq!(boundaries.len(), 3);
        } else {
            panic!("expected Range");
        }
    }

    #[test]
    fn missing_and_distinct_specs() {
        let missing = AggregateSpec::new(AggregateKind::Missing {
            field: FieldId::Extension,
        });
        assert!(matches!(missing.kind, AggregateKind::Missing { .. }));

        let distinct = AggregateSpec::new(AggregateKind::Distinct {
            field: FieldId::Type,
        });
        assert!(matches!(distinct.kind, AggregateKind::Distinct { .. }));
    }

    // ── TopHitsSpec tests ────────────────────────────────────────

    #[test]
    fn top_hits_default() {
        let spec = TopHitsSpec::default();
        assert_eq!(spec.count, 2);
        assert_eq!(spec.sort_field, FieldId::Size);
        assert!(spec.sort_desc);
        assert!(spec.projection.is_empty());
    }

    #[test]
    fn top_hits_with_count_clamps() {
        let zero = TopHitsSpec::with_count(0);
        assert_eq!(zero.count, 1, "count=0 clamped to 1");

        let huge = TopHitsSpec::with_count(99);
        assert_eq!(huge.count, MAX_SAMPLE_COUNT, "count=99 clamped to MAX");

        let normal = TopHitsSpec::with_count(3);
        assert_eq!(normal.count, 3);
    }

    #[test]
    fn top_hits_new_full() {
        let spec = TopHitsSpec::new(4, FieldId::Modified, false, vec![
            FieldId::Name,
            FieldId::Size,
        ]);
        assert_eq!(spec.count, 4);
        assert_eq!(spec.sort_field, FieldId::Modified);
        assert!(!spec.sort_desc);
        assert_eq!(spec.projection.len(), 2);
    }

    #[test]
    fn top_hits_effective_projection_default() {
        let spec = TopHitsSpec::default();
        let proj = spec.effective_projection();
        assert_eq!(proj, DEFAULT_PROJECTION);
        assert!(proj.contains(&FieldId::Name));
        assert!(proj.contains(&FieldId::Size));
        assert!(proj.contains(&FieldId::Path));
    }

    #[test]
    fn top_hits_effective_projection_custom() {
        let spec = TopHitsSpec::new(1, FieldId::Size, true, vec![
            FieldId::Name,
            FieldId::Extension,
        ]);
        let proj = spec.effective_projection();
        assert_eq!(proj.len(), 2);
        assert_eq!(proj[0], FieldId::Name);
        assert_eq!(proj[1], FieldId::Extension);
    }

    #[test]
    fn top_hits_validate_ok() {
        let spec = TopHitsSpec::default();
        spec.validate().unwrap();
    }

    #[test]
    fn top_hits_validate_unsortable_field() {
        let spec = TopHitsSpec::new(2, FieldId::Attributes, true, vec![]);
        let result = spec.validate();
        assert!(result.is_err(), "Attributes is not sortable");
        let msg = result.unwrap_err();
        assert!(msg.contains("not sortable"), "error: {msg}");
    }

    #[test]
    fn terms_with_sample() {
        let spec = AggregateSpec::new(AggregateKind::Terms {
            field: FieldId::Type,
            top: 10,
            metrics: vec![BucketMetric::Count],
            sample: Some(TopHitsSpec::with_count(3)),
        });
        if let AggregateKind::Terms { sample, .. } = &spec.kind {
            let sample_spec = sample.as_ref().expect("sample should be Some");
            assert_eq!(sample_spec.count, 3);
            assert_eq!(sample_spec.sort_field, FieldId::Size);
        } else {
            panic!("expected Terms");
        }
    }

    #[test]
    fn duplicates_with_top_hits_sample() {
        let spec = AggregateSpec::new(AggregateKind::Duplicates {
            keys: vec![FieldId::Size, FieldId::Name],
            verify: DuplicateVerify::None,
            top: 50,
            sample: Some(TopHitsSpec::new(2, FieldId::Modified, false, vec![])),
            max_groups: 100_000,
        });
        if let AggregateKind::Duplicates { sample, .. } = &spec.kind {
            let sample_spec = sample.as_ref().expect("sample should be Some");
            assert_eq!(sample_spec.count, 2);
            assert_eq!(sample_spec.sort_field, FieldId::Modified);
            assert!(!sample_spec.sort_desc);
        } else {
            panic!("expected Duplicates");
        }
    }

    #[test]
    fn rollup_with_sample() {
        let spec = AggregateSpec::new(AggregateKind::Rollup {
            mode: RollupMode::Drive,
            top: 5,
            metrics: vec![BucketMetric::Count],
            sample: Some(TopHitsSpec::default()),
            sub: None,
        });
        if let AggregateKind::Rollup { sample, .. } = &spec.kind {
            assert!(sample.is_some());
        } else {
            panic!("expected Rollup");
        }
    }
}
