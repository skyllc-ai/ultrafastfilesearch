// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Wire → core aggregate-spec conversion.
//!
//! Split out of `aggregation.rs` so the runtime path
//! (`run_aggregations` and the duplicate-verifier glue) and the wire
//! decoder can be read independently.  The decoder has no dependency
//! on [`IndexManager`] state — every match arm is a pure
//! [`AggregateSpecWire`] → [`AggregateSpec`] mapping — so isolating
//! it makes the transport contract obvious without changing any
//! call site.
//!
//! Public surface is unchanged: callers still write
//! `IndexManager::convert_wire_spec(ws)`.

use uffs_core::aggregate::TopHitsSpec;

use crate::index::IndexManager;

impl IndexManager {
    /// Convert a wire-protocol [`AggregateSpecWire`] into one or more
    /// core [`AggregateSpec`]s.
    ///
    /// Presets expand to multiple specs; all other kinds produce
    /// exactly one.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string when the wire spec is
    /// missing a required field, names an unknown preset / calendar
    /// / kind, or fails the inner `parse_agg_spec` (for `kind: "raw"`).
    ///
    /// [`AggregateSpec`]: uffs_core::aggregate::spec::AggregateSpec
    /// [`AggregateSpecWire`]: uffs_client::protocol::AggregateSpecWire
    #[expect(
        clippy::too_many_lines,
        reason = "straightforward match arms — one per wire kind"
    )]
    pub(crate) fn convert_wire_spec(
        ws: &uffs_client::protocol::AggregateSpecWire,
    ) -> Result<Vec<uffs_core::aggregate::spec::AggregateSpec>, String> {
        use uffs_core::aggregate::parser::parse_agg_spec;
        use uffs_core::aggregate::presets::AggregatePreset;
        use uffs_core::aggregate::spec::{
            AggregateKind, AggregateSpec, CalendarInterval, DuplicateVerify, RollupMode,
        };
        use uffs_core::search::field::FieldId;

        let make = |kind: AggregateKind| -> Vec<AggregateSpec> {
            let mut spec = AggregateSpec::new(kind);
            spec.label.clone_from(&ws.label);
            vec![spec]
        };

        match ws.kind.as_str() {
            "preset" => {
                let name = ws
                    .preset
                    .as_deref()
                    .ok_or_else(|| "preset kind requires 'preset' field".to_owned())?;
                let preset = AggregatePreset::parse(name)
                    .ok_or_else(|| format!("unknown preset: `{name}`"))?;
                Ok(preset.expand())
            }
            "count" => Ok(make(AggregateKind::Count)),
            "stats" => {
                let field = require_field(ws)?;
                let metrics = parse_scalar_metrics(&ws.metrics);
                Ok(make(AggregateKind::Stats { field, metrics }))
            }
            "terms" | "facet" => {
                let field = require_field(ws)?;
                let top = ws.top.unwrap_or(20);
                let metrics = parse_bucket_metrics(&ws.metrics);
                Ok(make(AggregateKind::Terms {
                    field,
                    top,
                    metrics,
                    sample: build_sample(ws),
                }))
            }
            "histogram" | "hist" => {
                let field = require_field(ws)?;
                let interval = ws.interval.unwrap_or(1_048_576);
                let metrics = parse_bucket_metrics(&ws.metrics);
                Ok(make(AggregateKind::Histogram {
                    field,
                    interval,
                    metrics,
                }))
            }
            "date_histogram" | "datehist" => {
                let field = require_field(ws)?;
                let cal_str = ws.calendar.as_deref().unwrap_or("month");
                let calendar = CalendarInterval::parse(cal_str)
                    .ok_or_else(|| format!("unknown calendar interval: `{cal_str}`"))?;
                let metrics = parse_bucket_metrics(&ws.metrics);
                Ok(make(AggregateKind::DateHistogram {
                    field,
                    calendar,
                    metrics,
                }))
            }
            "range" => {
                let field = require_field(ws)?;
                let metrics = parse_bucket_metrics(&ws.metrics);
                Ok(make(AggregateKind::Range {
                    field,
                    boundaries: ws.boundaries.clone(),
                    metrics,
                }))
            }
            "missing" => {
                let field = require_field(ws)?;
                Ok(make(AggregateKind::Missing { field }))
            }
            "distinct" => {
                let field = require_field(ws)?;
                Ok(make(AggregateKind::Distinct { field }))
            }
            "rollup" => {
                let mode_str = ws.field.as_deref().unwrap_or("path");
                let mode = match mode_str {
                    "drive" => RollupMode::Drive,
                    "ancestor" | "drilldown" => {
                        // Use interval field as the record index.
                        let record_idx = u32::try_from(ws.interval.unwrap_or(0)).unwrap_or(0);
                        RollupMode::Ancestor { record_idx }
                    }
                    _ => {
                        let depth = u32::try_from(ws.interval.unwrap_or(1)).unwrap_or(1);
                        RollupMode::Path { depth }
                    }
                };
                let top = ws.top.unwrap_or(30);
                let metrics = parse_bucket_metrics(&ws.metrics);
                Ok(make(AggregateKind::Rollup {
                    mode,
                    top,
                    metrics,
                    sample: build_sample(ws),
                    sub: None, // TODO: wire sub-agg from wire type
                }))
            }
            "duplicates" | "dups" => {
                let parsed_keys: Vec<FieldId> = ws
                    .metrics
                    .iter()
                    .filter_map(|raw| FieldId::parse(raw))
                    .collect();
                let keys = if parsed_keys.is_empty() {
                    vec![FieldId::Size, FieldId::Name]
                } else {
                    parsed_keys
                };
                let top = ws.top.unwrap_or(100);
                let verify = match ws.verify.as_deref() {
                    Some("first_bytes") => DuplicateVerify::FirstBytes {
                        count: ws.verify_bytes.unwrap_or(4096),
                    },
                    Some("sha256") => DuplicateVerify::Sha256,
                    _ => DuplicateVerify::None,
                };
                Ok(make(AggregateKind::Duplicates {
                    keys,
                    verify,
                    top,
                    sample: Some(build_sample(ws).unwrap_or_else(|| TopHitsSpec::with_count(2))),
                    max_groups: 500_000,
                }))
            }
            "raw" => {
                let syntax = ws
                    .label
                    .as_deref()
                    .ok_or_else(|| "raw kind requires syntax in 'label' field".to_owned())?;
                let spec = parse_agg_spec(syntax)?;
                Ok(vec![spec])
            }
            other => Err(format!("unknown aggregate kind: `{other}`")),
        }
    }
}

/// Parse the wire spec's `field` slot into a [`FieldId`], producing a
/// human-readable error when it is absent or names an unknown column.
fn require_field(
    ws: &uffs_client::protocol::AggregateSpecWire,
) -> Result<uffs_core::search::field::FieldId, String> {
    let name = ws
        .field
        .as_deref()
        .ok_or_else(|| "missing 'field'".to_owned())?;
    uffs_core::search::field::FieldId::parse(name).ok_or_else(|| format!("unknown field: `{name}`"))
}

/// Parse wire metric strings to [`BucketMetric`]s.
///
/// Empty input falls back to the default `[Count, TotalBytes]` pair so
/// `terms` / `histogram` / `range` / `rollup` / `date_histogram`
/// always emit at least the two metrics every UFFS dashboard relies on.
fn parse_bucket_metrics(wire: &[String]) -> Vec<uffs_core::aggregate::spec::BucketMetric> {
    use uffs_core::aggregate::spec::BucketMetric;
    if wire.is_empty() {
        return vec![BucketMetric::Count, BucketMetric::TotalBytes];
    }
    wire.iter()
        .filter_map(|metric| match metric.as_str() {
            "count" => Some(BucketMetric::Count),
            "total_bytes" | "bytes" | "size" => Some(BucketMetric::TotalBytes),
            "total_allocated" | "allocated" => Some(BucketMetric::TotalAllocated),
            "waste_bytes" | "waste" => Some(BucketMetric::WasteBytes),
            "waste_pct" | "waste_percent" => Some(BucketMetric::WastePct),
            "avg_size" | "avg" => Some(BucketMetric::AvgSize),
            "min_size" | "min" => Some(BucketMetric::MinSize),
            "max_size" | "max" => Some(BucketMetric::MaxSize),
            "share_count" | "share_of_count" => Some(BucketMetric::ShareOfTotalCount),
            "share_bytes" | "share_of_bytes" => Some(BucketMetric::ShareOfTotalBytes),
            _ => None,
        })
        .collect()
}

/// Parse wire metric strings to [`ScalarMetric`]s.
///
/// Empty input expands to `[Sum, Min, Max, Avg]`, matching the
/// behaviour every `stats` aggregation has shipped with since v0.5.
fn parse_scalar_metrics(wire: &[String]) -> Vec<uffs_core::aggregate::spec::ScalarMetric> {
    use uffs_core::aggregate::spec::ScalarMetric;
    if wire.is_empty() {
        return vec![
            ScalarMetric::Sum,
            ScalarMetric::Min,
            ScalarMetric::Max,
            ScalarMetric::Avg,
        ];
    }
    wire.iter()
        .filter_map(|metric| match metric.as_str() {
            "sum" => Some(ScalarMetric::Sum),
            "min" => Some(ScalarMetric::Min),
            "max" => Some(ScalarMetric::Max),
            "avg" | "mean" => Some(ScalarMetric::Avg),
            "value_count" | "count" => Some(ScalarMetric::ValueCount),
            "missing_count" | "missing" => Some(ScalarMetric::MissingCount),
            _ => None,
        })
        .collect()
}

/// Build `Option<TopHitsSpec>` from the wire spec's sample fields.
fn build_sample(ws: &uffs_client::protocol::AggregateSpecWire) -> Option<TopHitsSpec> {
    use uffs_core::search::field::FieldId;

    ws.sample.map(|count| {
        let mut th = TopHitsSpec::with_count(count);
        if let Some(field) = &ws.sample_sort
            && let Some(fid) = FieldId::parse(field)
        {
            th.sort_field = fid;
        }
        if let Some(desc) = ws.sample_desc {
            th.sort_desc = desc;
        }
        th
    })
}
