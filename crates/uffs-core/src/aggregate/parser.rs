// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Power syntax parser for `--agg` specifications.
//!
//! Parses strings like:
//! - `count`
//! - `stats:size`
//! - `terms:extension,top=50,metrics=count+total_bytes`
//! - `hist:size,interval=1048576`
//! - `datehist:modified,calendar=month`
//! - `range:size,bins=0..1024+1024..1048576+1048576..`
//! - `rollup:path,depth=1,top=30`
//! - `duplicates:size+name,verify=none,top=100,sample=2`
//! - `preset:overview`
//! - `missing:extension`
//! - `distinct:extension`

use super::spec::{
    AggregateKind, AggregateSpec, BucketMetric, CalendarInterval, DuplicateVerify, RollupMode,
    ScalarMetric, TopHitsSpec,
};
use crate::search::field::FieldId;

/// Parse a single `--agg` specification string into an `AggregateSpec`.
///
/// # Errors
///
/// Returns an error string if the spec is malformed.
pub fn parse_agg_spec(input: &str) -> Result<AggregateSpec, String> {
    let trimmed = input.trim();

    // Split on first ':'
    let (kind_str, rest) = trimmed.split_once(':').unwrap_or((trimmed, ""));

    match kind_str {
        "count" => Ok(AggregateSpec::new(AggregateKind::Count)),

        "stats" => parse_stats(rest),

        "terms" | "facet" => parse_terms(rest),

        "hist" | "histogram" => parse_histogram(rest),

        "datehist" | "date_histogram" => parse_date_histogram(rest),

        "range" => parse_range(rest),

        "rollup" => parse_rollup(rest),

        "duplicates" | "dups" => parse_duplicates(rest),

        "preset" => parse_preset(rest),

        "missing" => parse_missing(rest),

        "distinct" => parse_distinct(rest),

        _ => Err(format!("Unknown aggregate kind: `{kind_str}`")),
    }
}

/// Parse key=value options from a comma-separated string.
fn parse_options(input: &str) -> Vec<(&str, &str)> {
    input
        .split(',')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.split_once('='))
        .collect()
}

/// Extract the field and remaining options from "field,opt=val,opt=val".
fn split_field_and_options(input: &str) -> (&str, &str) {
    input.split_once(',').unwrap_or((input, ""))
}

/// Parse a field name to `FieldId`.
fn parse_field(name: &str) -> Result<FieldId, String> {
    FieldId::parse(name).ok_or_else(|| format!("Unknown field: `{name}`"))
}

/// Parse "field,metrics=M+M" → Stats spec.
fn parse_stats(rest: &str) -> Result<AggregateSpec, String> {
    let (field_str, opts_str) = split_field_and_options(rest);
    let field = parse_field(field_str)?;
    let opts = parse_options(opts_str);
    let mut metrics = Vec::new();

    for (key, val) in &opts {
        if *key == "metrics" {
            for metric in val.split('+') {
                metrics.push(parse_scalar_metric(metric)?);
            }
        }
    }

    if metrics.is_empty() {
        metrics = vec![
            ScalarMetric::Sum,
            ScalarMetric::Min,
            ScalarMetric::Max,
            ScalarMetric::Avg,
        ];
    }

    Ok(AggregateSpec::new(AggregateKind::Stats { field, metrics }))
}

/// Parse "field,top=N,metrics=M+M" → Terms spec.
fn parse_terms(rest: &str) -> Result<AggregateSpec, String> {
    let (field_str, opts_str) = split_field_and_options(rest);
    let field = parse_field(field_str)?;
    let opts = parse_options(opts_str);
    let mut top: u16 = 20;
    let mut metrics = Vec::new();
    let mut sample_count: u8 = 0;

    for (key, val) in &opts {
        match *key {
            "top" => {
                top = val
                    .parse()
                    .map_err(|err| format!("Invalid top: `{val}`: {err}"))?;
            }
            "metrics" => {
                for metric in val.split('+') {
                    metrics.push(parse_bucket_metric(metric)?);
                }
            }
            "sample" => {
                sample_count = val
                    .parse()
                    .map_err(|err| format!("Invalid sample: `{val}`: {err}"))?;
            }
            _ => {}
        }
    }

    if metrics.is_empty() {
        metrics = vec![BucketMetric::Count, BucketMetric::TotalBytes];
    }

    let sample = (sample_count > 0).then(|| TopHitsSpec::with_count(sample_count));

    Ok(AggregateSpec::new(AggregateKind::Terms {
        field,
        top,
        metrics,
        sample,
    }))
}

/// Parse "field,interval=N" → Histogram spec.
fn parse_histogram(rest: &str) -> Result<AggregateSpec, String> {
    let (field_str, opts_str) = split_field_and_options(rest);
    let field = parse_field(field_str)?;
    let opts = parse_options(opts_str);
    let mut interval: u64 = 1_048_576; // 1 MB default
    let mut metrics = vec![BucketMetric::Count, BucketMetric::TotalBytes];

    for (key, val) in &opts {
        match *key {
            "interval" => {
                interval = val
                    .parse()
                    .map_err(|err| format!("Invalid interval: `{val}`: {err}"))?;
            }
            "metrics" => {
                metrics.clear();
                for metric in val.split('+') {
                    metrics.push(parse_bucket_metric(metric)?);
                }
            }
            _ => {}
        }
    }

    Ok(AggregateSpec::new(AggregateKind::Histogram {
        field,
        interval,
        metrics,
    }))
}

/// Parse "field,calendar=INTERVAL" → `DateHistogram` spec.
fn parse_date_histogram(rest: &str) -> Result<AggregateSpec, String> {
    let (field_str, opts_str) = split_field_and_options(rest);
    let field = parse_field(field_str)?;
    let opts = parse_options(opts_str);
    let mut calendar = CalendarInterval::Month;
    let mut metrics = vec![BucketMetric::Count, BucketMetric::TotalBytes];

    for (key, val) in &opts {
        match *key {
            "calendar" | "interval" => {
                calendar = CalendarInterval::parse(val)
                    .ok_or_else(|| format!("Invalid calendar interval: `{val}`"))?;
            }
            "metrics" => {
                metrics.clear();
                for metric in val.split('+') {
                    metrics.push(parse_bucket_metric(metric)?);
                }
            }
            _ => {}
        }
    }

    Ok(AggregateSpec::new(AggregateKind::DateHistogram {
        field,
        calendar,
        metrics,
    }))
}

/// Parse "field,bins=A..B+C..D+E.." → Range spec.
fn parse_range(rest: &str) -> Result<AggregateSpec, String> {
    let (field_str, opts_str) = split_field_and_options(rest);
    let field = parse_field(field_str)?;
    let opts = parse_options(opts_str);
    let mut boundaries: Vec<u64> = Vec::new();
    let mut metrics = vec![BucketMetric::Count, BucketMetric::TotalBytes];

    for (key, val) in &opts {
        match *key {
            "bins" | "boundaries" => {
                for boundary_str in val.split('+') {
                    // Each bin can be "A..B" or just "A". Extract the boundary values.
                    for part in boundary_str.split("..") {
                        if !part.is_empty() {
                            let boundary_val: u64 = part.parse().map_err(|err| {
                                format!("Invalid range boundary: `{part}`: {err}")
                            })?;
                            if !boundaries.contains(&boundary_val) {
                                boundaries.push(boundary_val);
                            }
                        }
                    }
                }
                boundaries.sort_unstable();
                boundaries.dedup();
            }
            "metrics" => {
                metrics.clear();
                for metric in val.split('+') {
                    metrics.push(parse_bucket_metric(metric)?);
                }
            }
            _ => {}
        }
    }

    Ok(AggregateSpec::new(AggregateKind::Range {
        field,
        boundaries,
        metrics,
    }))
}

/// Parse "path,depth=N,top=N" or "drive,top=N" or "ancestor,record=N" → Rollup
/// spec.
///
/// Nested sub-aggregation syntax: `rollup:drive,sub=terms:type`
fn parse_rollup(rest: &str) -> Result<AggregateSpec, String> {
    let (mode_str, opts_str) = split_field_and_options(rest);
    let opts = parse_options(opts_str);
    let mut depth: u32 = 1;
    let mut top: u16 = 30;
    let mut metrics = vec![BucketMetric::Count, BucketMetric::TotalBytes];
    let mut sample_count: u8 = 0;
    let mut record_idx: Option<u32> = None;
    let mut sub_spec: Option<Box<AggregateSpec>> = None;

    for (key, val) in &opts {
        match *key {
            "depth" => {
                depth = val
                    .parse()
                    .map_err(|err| format!("Invalid depth: `{val}`: {err}"))?;
            }
            "top" => {
                top = val
                    .parse()
                    .map_err(|err| format!("Invalid top: `{val}`: {err}"))?;
            }
            "record" | "frs" | "ancestor" => {
                record_idx = Some(
                    val.parse()
                        .map_err(|err| format!("Invalid record index: `{val}`: {err}"))?,
                );
            }
            "metrics" => {
                metrics.clear();
                for metric in val.split('+') {
                    metrics.push(parse_bucket_metric(metric)?);
                }
            }
            "sample" => {
                sample_count = val
                    .parse()
                    .map_err(|err| format!("Invalid sample: `{val}`: {err}"))?;
            }
            "sub" => {
                // Parse nested sub-aggregation spec (e.g. "terms:type").
                let inner = parse_agg_spec(val)?;
                sub_spec = Some(Box::new(inner));
            }
            _ => {}
        }
    }

    let mode = match mode_str {
        "drive" => RollupMode::Drive,
        "path" | "folder" | "dir" => RollupMode::Path { depth },
        "ancestor" | "drilldown" => {
            let idx = record_idx
                .ok_or_else(|| "rollup:ancestor requires record=<idx> option".to_owned())?;
            RollupMode::Ancestor { record_idx: idx }
        }
        _ => {
            return Err(format!(
                "Unknown rollup mode: `{mode_str}`. Use 'path', 'drive', or 'ancestor'."
            ));
        }
    };

    let sample = (sample_count > 0).then(|| TopHitsSpec::with_count(sample_count));

    Ok(AggregateSpec::new(AggregateKind::Rollup {
        mode,
        top,
        metrics,
        sample,
        sub: sub_spec,
    }))
}

/// Parse "size+name,verify=MODE,top=N,sample=N" → Duplicates spec.
fn parse_duplicates(rest: &str) -> Result<AggregateSpec, String> {
    let (keys_str, opts_str) = split_field_and_options(rest);
    let opts = parse_options(opts_str);

    let mut keys: Vec<FieldId> = Vec::new();
    for key in keys_str.split('+') {
        keys.push(parse_field(key)?);
    }
    if keys.is_empty() {
        keys = vec![FieldId::Size, FieldId::Name];
    }

    let mut verify = DuplicateVerify::None;
    let mut top: u16 = 100;
    let mut sample_count: u8 = 2;
    let mut max_groups: u32 = 500_000;

    for (key, val) in &opts {
        match *key {
            "verify" => {
                verify = match *val {
                    "none" => DuplicateVerify::None,
                    "first_bytes" | "first" => DuplicateVerify::FirstBytes { count: 4096 },
                    "sha256" | "hash" => DuplicateVerify::Sha256,
                    _ => return Err(format!("Unknown verify mode: `{val}`")),
                };
            }
            "top" => {
                top = val
                    .parse()
                    .map_err(|err| format!("Invalid top: `{val}`: {err}"))?;
            }
            "sample" => {
                sample_count = val
                    .parse()
                    .map_err(|err| format!("Invalid sample: `{val}`: {err}"))?;
            }
            "max_groups" => {
                max_groups = val
                    .parse()
                    .map_err(|err| format!("Invalid max_groups: `{val}`: {err}"))?;
            }
            _ => {}
        }
    }

    let sample = (sample_count > 0).then(|| TopHitsSpec::with_count(sample_count));

    Ok(AggregateSpec::new(AggregateKind::Duplicates {
        keys,
        verify,
        top,
        sample,
        max_groups,
    }))
}

/// Parse "NAME" → Preset expansion.
fn parse_preset(rest: &str) -> Result<AggregateSpec, String> {
    let name = rest.trim();
    if super::presets::AggregatePreset::parse(name).is_none() {
        return Err(format!(
            "Unknown preset: `{name}`. Available: {}",
            super::presets::AggregatePreset::ALL_NAMES.join(", ")
        ));
    }
    // Return a Count as placeholder — the caller should expand the preset.
    // For now, we signal via label that this is a preset.
    Ok(AggregateSpec::with_label(
        AggregateKind::Count,
        format!("__preset__{name}"),
    ))
}

/// Parse "field" → Missing spec.
fn parse_missing(rest: &str) -> Result<AggregateSpec, String> {
    let field = parse_field(rest.trim())?;
    Ok(AggregateSpec::new(AggregateKind::Missing { field }))
}

/// Parse "field" → Distinct spec.
fn parse_distinct(rest: &str) -> Result<AggregateSpec, String> {
    let field = parse_field(rest.trim())?;
    Ok(AggregateSpec::new(AggregateKind::Distinct { field }))
}

/// Parse a scalar metric name.
fn parse_scalar_metric(name: &str) -> Result<ScalarMetric, String> {
    match name {
        "sum" => Ok(ScalarMetric::Sum),
        "min" => Ok(ScalarMetric::Min),
        "max" => Ok(ScalarMetric::Max),
        "avg" | "mean" => Ok(ScalarMetric::Avg),
        "value_count" | "count" => Ok(ScalarMetric::ValueCount),
        "missing_count" | "missing" => Ok(ScalarMetric::MissingCount),
        _ => Err(format!("Unknown scalar metric: `{name}`")),
    }
}

/// Parse a bucket metric name.
fn parse_bucket_metric(name: &str) -> Result<BucketMetric, String> {
    match name {
        "count" => Ok(BucketMetric::Count),
        "total_bytes" | "bytes" | "size" => Ok(BucketMetric::TotalBytes),
        "total_allocated" | "allocated" => Ok(BucketMetric::TotalAllocated),
        "waste_bytes" | "waste" => Ok(BucketMetric::WasteBytes),
        "waste_pct" | "waste_percent" => Ok(BucketMetric::WastePct),
        "avg_size" | "avg" => Ok(BucketMetric::AvgSize),
        "min_size" | "min" => Ok(BucketMetric::MinSize),
        "max_size" | "max" => Ok(BucketMetric::MaxSize),
        "share_count" | "share_of_count" => Ok(BucketMetric::ShareOfTotalCount),
        "share_bytes" | "share_of_bytes" => Ok(BucketMetric::ShareOfTotalBytes),
        _ => Err(format!("Unknown bucket metric: `{name}`")),
    }
}

/// Parse multiple `--agg` specifications and expand presets.
///
/// Returns a flat list of `AggregateSpec`s with all presets expanded.
///
/// # Errors
///
/// Returns errors for any malformed spec.
pub fn parse_and_expand_agg_specs(inputs: &[&str]) -> Result<Vec<AggregateSpec>, String> {
    let mut result = Vec::new();

    for input in inputs {
        let spec = parse_agg_spec(input)?;

        // Check if this is a preset placeholder.
        if let Some(label) = &spec.label
            && let Some(name) = label.strip_prefix("__preset__")
            && let Some(preset) = super::presets::AggregatePreset::parse(name)
        {
            result.extend(preset.expand());
            continue;
        }

        result.push(spec);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_count() {
        let spec = parse_agg_spec("count").unwrap();
        assert!(matches!(spec.kind, AggregateKind::Count));
    }

    #[test]
    fn parse_stats_size() {
        let spec = parse_agg_spec("stats:size").unwrap();
        if let AggregateKind::Stats { field, metrics } = &spec.kind {
            assert_eq!(*field, FieldId::Size);
            assert_eq!(metrics.len(), 4); // default: sum/min/max/avg
        } else {
            panic!("expected Stats");
        }
    }

    #[test]
    fn parse_terms_extension_with_top() {
        let spec = parse_agg_spec("terms:extension,top=100").unwrap();
        if let AggregateKind::Terms { field, top, .. } = &spec.kind {
            assert_eq!(*field, FieldId::Extension);
            assert_eq!(*top, 100);
        } else {
            panic!("expected Terms");
        }
    }

    #[test]
    fn parse_facet_alias() {
        let spec = parse_agg_spec("facet:extension").unwrap();
        assert!(matches!(spec.kind, AggregateKind::Terms { .. }));
    }

    #[test]
    fn parse_date_histogram() {
        let spec = parse_agg_spec("datehist:modified,calendar=month").unwrap();
        if let AggregateKind::DateHistogram {
            field, calendar, ..
        } = &spec.kind
        {
            assert_eq!(*field, FieldId::Modified);
            assert_eq!(*calendar, CalendarInterval::Month);
        } else {
            panic!("expected DateHistogram");
        }
    }

    #[test]
    fn parse_range_with_bins() {
        let spec = parse_agg_spec("range:size,bins=1024+1048576+1073741824").unwrap();
        if let AggregateKind::Range { boundaries, .. } = &spec.kind {
            assert_eq!(boundaries.len(), 3);
        } else {
            panic!("expected Range");
        }
    }

    #[test]
    fn parse_rollup_path() {
        let spec = parse_agg_spec("rollup:path,depth=2,top=20").unwrap();
        if let AggregateKind::Rollup { mode, top, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Path { depth: 2 });
            assert_eq!(*top, 20);
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_duplicates() {
        let spec = parse_agg_spec("duplicates:size+name,verify=none,top=50").unwrap();
        if let AggregateKind::Duplicates { keys, top, .. } = &spec.kind {
            assert_eq!(keys.len(), 2);
            assert_eq!(*top, 50);
        } else {
            panic!("expected Duplicates");
        }
    }

    #[test]
    fn parse_missing() {
        let spec = parse_agg_spec("missing:extension").unwrap();
        assert!(matches!(spec.kind, AggregateKind::Missing { .. }));
    }

    #[test]
    fn parse_distinct() {
        let spec = parse_agg_spec("distinct:extension").unwrap();
        assert!(matches!(spec.kind, AggregateKind::Distinct { .. }));
    }

    #[test]
    fn parse_preset() {
        let spec = parse_agg_spec("preset:overview").unwrap();
        assert!(spec.label.as_deref().unwrap().starts_with("__preset__"));
    }

    #[test]
    fn parse_and_expand_preset() {
        let specs = parse_and_expand_agg_specs(&["preset:overview"]).unwrap();
        assert!(specs.len() >= 5); // overview has 5+ specs
    }

    #[test]
    fn parse_unknown_kind() {
        let result = parse_agg_spec("foobar:thing");
        result.unwrap_err();
    }

    #[test]
    fn parse_histogram() {
        let spec = parse_agg_spec("hist:size,interval=1024").unwrap();
        if let AggregateKind::Histogram {
            field, interval, ..
        } = &spec.kind
        {
            assert_eq!(*field, FieldId::Size);
            assert_eq!(*interval, 1024);
        } else {
            panic!("expected Histogram");
        }
    }

    // ── Stage 2 gap-fill tests ────────────────────────────────────

    #[test]
    fn parse_terms_with_sample() {
        let spec = parse_agg_spec("terms:extension,top=10,sample=3").unwrap();
        if let AggregateKind::Terms {
            field, top, sample, ..
        } = &spec.kind
        {
            assert_eq!(*field, FieldId::Extension);
            assert_eq!(*top, 10);
            let sample_spec = sample.as_ref().expect("sample should be Some");
            assert_eq!(sample_spec.count, 3);
        } else {
            panic!("expected Terms");
        }
    }

    #[test]
    fn parse_terms_without_sample() {
        let spec = parse_agg_spec("terms:extension,top=5").unwrap();
        if let AggregateKind::Terms { sample, .. } = &spec.kind {
            assert!(sample.is_none(), "no sample= → None");
        } else {
            panic!("expected Terms");
        }
    }

    #[test]
    fn parse_rollup_drive() {
        let spec = parse_agg_spec("rollup:drive,top=10").unwrap();
        if let AggregateKind::Rollup { mode, top, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Drive);
            assert_eq!(*top, 10);
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_with_sample() {
        let spec = parse_agg_spec("rollup:path,depth=2,sample=2").unwrap();
        if let AggregateKind::Rollup { mode, sample, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Path { depth: 2 });
            let sample_spec = sample.as_ref().expect("sample should be Some");
            assert_eq!(sample_spec.count, 2);
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_duplicates_with_sample() {
        let spec = parse_agg_spec("duplicates:size+name,sample=5,top=50").unwrap();
        if let AggregateKind::Duplicates {
            keys, top, sample, ..
        } = &spec.kind
        {
            assert_eq!(keys.len(), 2);
            assert_eq!(*top, 50);
            let sample_spec = sample.as_ref().expect("sample should be Some");
            assert_eq!(sample_spec.count, 5);
        } else {
            panic!("expected Duplicates");
        }
    }

    #[test]
    fn parse_rollup_folder_alias() {
        let spec = parse_agg_spec("rollup:folder,depth=3").unwrap();
        if let AggregateKind::Rollup { mode, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Path { depth: 3 });
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_dir_alias() {
        let spec = parse_agg_spec("rollup:dir").unwrap();
        if let AggregateKind::Rollup { mode, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Path { depth: 1 });
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_ancestor_mode() {
        let spec = parse_agg_spec("rollup:ancestor,record=42,top=10").unwrap();
        if let AggregateKind::Rollup { mode, top, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Ancestor { record_idx: 42 });
            assert_eq!(*top, 10);
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_ancestor_frs_alias() {
        let spec = parse_agg_spec("rollup:ancestor,frs=100").unwrap();
        if let AggregateKind::Rollup { mode, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Ancestor { record_idx: 100 });
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_ancestor_missing_record_errors() {
        let err = parse_agg_spec("rollup:ancestor,top=10");
        assert!(err.is_err(), "ancestor without record= should fail");
    }

    #[test]
    fn parse_rollup_drilldown_alias() {
        let spec = parse_agg_spec("rollup:drilldown,record=5").unwrap();
        if let AggregateKind::Rollup { mode, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Ancestor { record_idx: 5 });
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_with_nested_sub() {
        let spec = parse_agg_spec("rollup:drive,sub=terms:extension").unwrap();
        if let AggregateKind::Rollup { mode, sub, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Drive);
            let sub_spec = sub.as_ref().expect("sub should be present");
            assert!(
                matches!(&sub_spec.kind, AggregateKind::Terms { .. }),
                "sub should be a terms aggregation"
            );
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_rollup_no_sub_by_default() {
        let spec = parse_agg_spec("rollup:drive,top=5").unwrap();
        if let AggregateKind::Rollup { sub, .. } = &spec.kind {
            assert!(sub.is_none(), "sub should be None when not specified");
        } else {
            panic!("expected Rollup");
        }
    }

    #[test]
    fn parse_nested_rollup_path_with_terms_type() {
        let spec = parse_agg_spec("rollup:path,depth=1,sub=terms:type,top=20").unwrap();
        if let AggregateKind::Rollup { mode, sub, top, .. } = &spec.kind {
            assert_eq!(*mode, RollupMode::Path { depth: 1 });
            assert_eq!(*top, 20);
            let sub_spec = sub.as_ref().expect("sub should be present");
            assert!(matches!(&sub_spec.kind, AggregateKind::Terms { .. }));
        } else {
            panic!("expected Rollup");
        }
    }
    #[test]
    fn parse_terms_sample_zero_is_none() {
        let spec = parse_agg_spec("terms:extension,sample=0").unwrap();
        if let AggregateKind::Terms { sample, .. } = &spec.kind {
            assert!(sample.is_none(), "sample=0 should produce None");
        } else {
            panic!("expected Terms");
        }
    }
}
