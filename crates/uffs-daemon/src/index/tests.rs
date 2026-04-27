// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Integration tests for aggregation and predicate conversion.
//! Exception: `file_size_policy` — aggregation test suite, shared fixture
//! requires cohesion.

#![expect(
    clippy::indexing_slicing,
    clippy::min_ident_chars,
    clippy::std_instead_of_alloc,
    reason = "test code — relaxed linting for test clarity"
)]

use std::sync::Arc;

use uffs_client::protocol::AggregateSpecWire;
use uffs_core::aggregate::AggregateFilter;
use uffs_core::aggregate::spec::AggregateKind;
use uffs_core::compact::build_compact_index;
use uffs_core::search::backend::DriveIndex;
use uffs_core::search::field::FieldId;
use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

use super::IndexManager;
use super::aggregation::AggregationRequest;

/// Build a synthetic drive with root + 1 dir + 5 files of varied
/// sizes/extensions.
fn build_test_drive() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('C');

    // Root directory
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    // Subdirectory "Projects"
    let dir_name = "Projects";
    let dir_off = idx.add_name(dir_name);
    let dir_ext = idx.intern_extension(dir_name);
    let dir = idx.get_or_create(100);
    dir.stdinfo.set_directory(true);
    dir.stdinfo.flags = 0x10; // directory flag
    dir.first_name.name =
        IndexNameRef::new(dir_off, uffs_mft::len_to_u16(dir_name.len()), true, dir_ext);
    dir.first_name.parent_frs = ROOT_FRS;

    // Files with different extensions and sizes
    let files: &[(&str, u64, u64, u64)] = &[
        ("readme.md", 101, 500, 512),
        ("main.rs", 102, 2000, 4096),
        ("lib.rs", 103, 3000, 4096),
        ("config.toml", 104, 100, 512),
        ("data.bin", 105, 10_000, 16_384),
    ];

    for &(name, frs, size, allocated) in files {
        let off = idx.add_name(name);
        let ext = idx.intern_extension(name);
        let rec = idx.get_or_create(frs);
        rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(name.len()), true, ext);
        rec.first_name.parent_frs = 100; // under Projects
        rec.first_stream.size = SizeInfo {
            length: size,
            allocated,
        };
        rec.stdinfo.flags = 0x20; // archive
        rec.stdinfo.modified = 1_000_000;
    }

    let (drive, _, _) = build_compact_index('C', &idx);
    drive
}

fn test_index() -> DriveIndex {
    DriveIndex {
        drives: vec![Arc::new(build_test_drive())],
    }
}

fn spec(kind: &str) -> AggregateSpecWire {
    AggregateSpecWire {
        kind: kind.to_owned(),
        ..AggregateSpecWire::default()
    }
}

// ── Preset round-trip ────────────────────────────────────────────

#[test]
fn preset_overview_returns_multiple_results() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        preset: Some("overview".to_owned()),
        ..spec("preset")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    // overview preset expands to count + stats + terms etc.
    assert!(
        results.len() >= 3,
        "overview should produce ≥3 results, got {}",
        results.len()
    );
    // First result is typically count
    let count = results.iter().find(|r| r.kind == "count").unwrap();
    // 5 files + 1 dir + root = 7 records total
    assert!(count.value.unwrap() >= 5, "count should be ≥5 files");
}

// ── Count ────────────────────────────────────────────────────────

#[test]
fn count_returns_total_records() {
    let index = test_index();
    let (results, _matched) = IndexManager::run_aggregations(
        &index,
        None,
        &[spec("count")],
        AggregationRequest::default(),
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "count");
    // root + Projects dir + 5 files = 7
    assert_eq!(results[0].value, Some(7));
}

// ── Stats ────────────────────────────────────────────────────────

#[test]
fn stats_size_returns_metrics() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("size".to_owned()),
        ..spec("stats")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "stats");
    let stats = results[0].stats.as_ref().unwrap();
    assert!(stats.count > 0);
    // Total of 500+2000+3000+100+10000 = 15600 for files; dirs have size 0
    assert!(stats.sum > 0);
    assert!(stats.min <= stats.max);
}

// ── Terms ────────────────────────────────────────────────────────

#[test]
fn terms_extension_returns_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "buckets");
    // We have rs, md, toml, bin extensions
    assert!(
        results[0].buckets.len() >= 3,
        "expected ≥3 ext buckets, got {}",
        results[0].buckets.len()
    );
    // "rs" should have 2 files (main.rs, lib.rs)
    let rs_bucket = results[0].buckets.iter().find(|b| b.key == "rs");
    assert!(rs_bucket.is_some(), "should have 'rs' bucket");
    assert_eq!(rs_bucket.unwrap().count, 2);
}

// ── Histogram ────────────────────────────────────────────────────

#[test]
fn histogram_size_returns_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("size".to_owned()),
        ..spec("histogram")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "buckets");
    // Should have at least 1 bucket covering the file sizes
    assert!(!results[0].buckets.is_empty());
}

// ── Date Histogram ───────────────────────────────────────────────

#[test]
fn date_histogram_returns_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("modified".to_owned()),
        calendar: Some("month".to_owned()),
        ..spec("datehist")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "buckets");
}

// ── Missing ──────────────────────────────────────────────────────

#[test]
fn missing_extension_counts_records_without_ext() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        ..spec("missing")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "missing");
    // Root "." and dir "Projects" have no extension → ≥2 missing
    assert!(results[0].value.unwrap() >= 2);
}

// ── Distinct ─────────────────────────────────────────────────────

#[test]
fn distinct_extension_counts_unique_values() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        ..spec("distinct")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "distinct");
    // rs, md, toml, bin → 4 distinct extensions
    assert!(results[0].value.unwrap() >= 4);
}

// ── Rollup ───────────────────────────────────────────────────────

#[test]
fn rollup_drive_returns_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("drive".to_owned()),
        top: Some(10),
        ..spec("rollup")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    // Rollup → buckets or rollup kind
    assert!(!results[0].buckets.is_empty() || results[0].value.is_some());
}

// ── Duplicates ───────────────────────────────────────────────────

#[test]
fn duplicates_returns_result() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        top: Some(10),
        ..spec("duplicates")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    // Should return exactly 1 result (even if 0 duplicates)
    assert_eq!(results.len(), 1);
}

// ── Raw power syntax ─────────────────────────────────────────────

#[test]
fn raw_power_syntax_terms_works() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        label: Some("terms:extension,top=5".to_owned()),
        ..spec("raw")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "buckets");
    assert!(!results[0].buckets.is_empty());
}

// ── Error handling ───────────────────────────────────────────────

#[test]
fn unknown_kind_skipped_gracefully() {
    let index = test_index();
    let specs = [spec("bogus_kind")];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(results.is_empty(), "unknown kind should produce no results");
}

#[test]
fn missing_field_skipped_gracefully() {
    let index = test_index();
    // stats requires a field but none provided
    let specs = [spec("stats")];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(
        results.is_empty(),
        "missing field should produce no results"
    );
}

// ── Multiple specs in one call ───────────────────────────────────

#[test]
fn multiple_specs_return_multiple_results() {
    let index = test_index();
    let specs = [
        spec("count"),
        AggregateSpecWire {
            field: Some("size".to_owned()),
            ..spec("stats")
        },
        AggregateSpecWire {
            field: Some("extension".to_owned()),
            top: Some(5),
            ..spec("terms")
        },
    ];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert_eq!(results.len(), 3, "should return one result per spec");
    assert_eq!(results[0].kind, "count");
    assert_eq!(results[1].kind, "stats");
    assert_eq!(results[2].kind, "buckets");
}

// ── S1H.2: uffs stats daemon-path parity ─────────────────────────

#[test]
fn stats_overview_preset_wire_roundtrip() {
    // Simulate the exact wire spec that `uffs stats` (no path)
    // sends to the daemon, and verify it produces correct results.
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "preset".to_owned(),
        preset: Some("overview".to_owned()),
        ..AggregateSpecWire::default()
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());

    // Overview preset expands to multiple results.
    assert!(
        results.len() >= 3,
        "overview should produce ≥3 results, got {}",
        results.len()
    );

    // Must include a count result.
    let count = results.iter().find(|r| r.kind == "count").unwrap();
    assert_eq!(count.value, Some(7)); // root + dir + 5 files

    // Must include a stats result with valid metrics.
    let stats = results.iter().find(|r| r.kind == "stats").unwrap();
    let s = stats.stats.as_ref().unwrap();
    assert!(s.count > 0);
    assert!(s.sum > 0);

    // Must include a buckets result (extension or type terms).
    let has_buckets = results.iter().any(|r| r.kind == "buckets");
    assert!(has_buckets, "overview should include bucket results");
}

// ── S2G.13: terms with sample=2 produces sample_rows + drilldown ──

#[test]
fn terms_with_sample_produces_sample_rows_and_drilldown() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "terms".to_owned(),
        field: Some("extension".to_owned()),
        top: Some(5),
        sample: Some(2),
        sample_sort: None,
        sample_desc: None,
        ..spec("terms")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(!results.is_empty(), "should have results");

    let bucket_result = results
        .iter()
        .find(|r| r.kind == "buckets")
        .expect("should have a buckets result");
    assert!(
        !bucket_result.buckets.is_empty(),
        "should have at least one bucket"
    );

    // At least one bucket should have sample_rows (our synthetic index
    // has files with extensions, so TopHits should find records).
    let has_samples = bucket_result
        .buckets
        .iter()
        .any(|b| !b.sample_rows.is_empty());
    assert!(
        has_samples,
        "at least one bucket should have sample rows with sample=2"
    );

    // Verify sample row constraints.
    for b in &bucket_result.buckets {
        assert!(
            b.sample_rows.len() <= 2,
            "sample rows should be bounded by sample=2, got {}",
            b.sample_rows.len()
        );
    }

    // Every bucket should have a drilldown predicate for the bucket key.
    for b in &bucket_result.buckets {
        assert!(
            !b.drilldown.is_empty(),
            "bucket '{}' should have drilldown predicates",
            b.key
        );
        let has_key_pred = b.drilldown.iter().any(|d| d.field == "extension");
        assert!(
            has_key_pred,
            "bucket '{}' should have an extension drilldown predicate",
            b.key
        );
    }
}

#[test]
fn terms_without_sample_has_empty_sample_rows() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "terms".to_owned(),
        field: Some("extension".to_owned()),
        top: Some(5),
        ..spec("terms")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    let bucket_result = results
        .iter()
        .find(|r| r.kind == "buckets")
        .expect("should have buckets");
    // No sample was requested → all sample_rows should be empty.
    for b in &bucket_result.buckets {
        assert!(
            b.sample_rows.is_empty(),
            "bucket '{}' should not have sample rows without sample spec",
            b.key
        );
    }
}

// ── Stage 2 gap-fill: daemon integration tests ────────────────────

#[test]
fn rollup_drive_via_wire() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "rollup".to_owned(),
        field: Some("drive".to_owned()),
        top: Some(10),
        ..spec("rollup")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(!results.is_empty(), "rollup:drive should return results");
    let result = results
        .iter()
        .find(|r| r.kind == "rollup")
        .expect("should have a rollup result");
    assert!(
        !result.buckets.is_empty(),
        "rollup:drive should have buckets"
    );
}

#[test]
fn rollup_path_with_sample_via_wire() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "rollup".to_owned(),
        field: Some("path".to_owned()),
        top: Some(5),
        sample: Some(2),
        ..spec("rollup")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(
        !results.is_empty(),
        "rollup:path with sample should return results"
    );
    let result = results
        .iter()
        .find(|r| r.kind == "rollup")
        .expect("should have a rollup result");
    // Sample rows should be bounded.
    for b in &result.buckets {
        assert!(
            b.sample_rows.len() <= 2,
            "rollup bucket '{}' should have ≤2 sample rows, got {}",
            b.key,
            b.sample_rows.len()
        );
    }
}

#[test]
fn convert_wire_spec_terms_with_sample_fields() {
    let ws = AggregateSpecWire {
        kind: "terms".to_owned(),
        field: Some("extension".to_owned()),
        top: Some(5),
        sample: Some(3),
        sample_sort: Some("size".to_owned()),
        sample_desc: Some(true),
        ..spec("terms")
    };
    let converted = IndexManager::convert_wire_spec(&ws).unwrap();
    assert_eq!(converted.len(), 1);
    assert!(
        matches!(&converted[0].kind, AggregateKind::Terms { .. }),
        "expected Terms variant"
    );
    if let AggregateKind::Terms { sample, .. } = &converted[0].kind {
        let top_hits = sample.as_ref().expect("sample should be Some");
        assert_eq!(top_hits.count, 3);
        assert_eq!(top_hits.sort_field, FieldId::Size);
        assert!(top_hits.sort_desc, "sort_desc should be true");
    }
}

#[test]
fn query_predicates_forwarded_to_drilldown() {
    use uffs_core::aggregate::finalize::{DrilldownPredicate, DrilldownValue};
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "terms".to_owned(),
        field: Some("extension".to_owned()),
        top: Some(3),
        sample: Some(1),
        ..spec("terms")
    }];
    let predicates = vec![DrilldownPredicate {
        field: "name".to_owned(),
        op: "glob".to_owned(),
        value: DrilldownValue::String("*.rs".to_owned()),
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            query_predicates: predicates,
            ..AggregationRequest::default()
        });
    let result = results
        .iter()
        .find(|r| r.kind == "buckets")
        .expect("should have buckets");
    // Each bucket's drilldown should include the query predicate for "name".
    for b in &result.buckets {
        let has_name_pred = b
            .drilldown
            .iter()
            .any(|d| d.field == "name" && d.op == "glob");
        assert!(
            has_name_pred,
            "bucket '{}' should have query predicate for 'name' in drilldown",
            b.key
        );
    }
}

#[test]
fn raw_power_syntax_rollup_drive() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "raw".to_owned(),
        label: Some("rollup:drive,top=5".to_owned()),
        ..spec("raw")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(
        !results.is_empty(),
        "raw rollup:drive should return results"
    );
}

#[test]
fn raw_power_syntax_hist_size() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "raw".to_owned(),
        label: Some("hist:size,interval=1048576".to_owned()),
        ..spec("raw")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(!results.is_empty(), "raw hist:size should return results");
}

#[test]
fn raw_power_syntax_stats_size() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        kind: "raw".to_owned(),
        label: Some("stats:size".to_owned()),
        ..spec("raw")
    }];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    assert!(!results.is_empty(), "raw stats:size should return results");
    let stats = results.iter().find(|r| r.kind == "stats");
    assert!(stats.is_some(), "should have a stats result");
}

// ── Cursor pagination ─────────────────────────────────────────────

#[test]
fn page_size_paginates_terms_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];
    // Request page_size=2 → first page should have ≤2 buckets.
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            agg_page_size: Some(2),
            ..AggregationRequest::default()
        });
    let terms = results.iter().find(|r| r.kind == "buckets").unwrap();
    assert!(
        terms.buckets.len() <= 2,
        "expected ≤2 buckets, got {}",
        terms.buckets.len()
    );
    // With 4 extensions and page_size=2, next_cursor should be present.
    assert!(
        terms.next_cursor.is_some(),
        "expected next_cursor for first page of 4 extensions with page_size=2"
    );
}

#[test]
fn cursor_returns_next_page() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];

    // First page.
    let (page1, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            agg_page_size: Some(2),
            ..AggregationRequest::default()
        });
    let terms1 = page1.iter().find(|r| r.kind == "buckets").unwrap();
    let cursor = terms1
        .next_cursor
        .as_deref()
        .expect("first page should have next_cursor");

    // Second page using cursor from first.
    let (page2, _matched_page2) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            agg_cursor: Some(cursor),
            agg_page_size: Some(2),
            ..AggregationRequest::default()
        });
    let terms2 = page2.iter().find(|r| r.kind == "buckets").unwrap();

    // Second page should have different keys than first page.
    let keys1: Vec<&str> = terms1.buckets.iter().map(|b| b.key.as_str()).collect();
    let keys2: Vec<&str> = terms2.buckets.iter().map(|b| b.key.as_str()).collect();
    assert!(
        !keys2.is_empty(),
        "second page should have at least 1 bucket"
    );
    for key in &keys2 {
        assert!(
            !keys1.contains(key),
            "second page key `{key}` should not appear in first page"
        );
    }
}

#[test]
fn page_size_does_not_affect_non_bucket_results() {
    let index = test_index();
    let specs = [spec("count")];
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            agg_page_size: Some(2),
            ..AggregationRequest::default()
        });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, "count");
    // Count results should not have next_cursor.
    assert!(
        results[0].next_cursor.is_none(),
        "count results should never have next_cursor"
    );
}

#[test]
fn no_pagination_returns_all_buckets() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];
    // Without pagination, all buckets returned.
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest::default());
    let terms = results.iter().find(|r| r.kind == "buckets").unwrap();
    assert!(
        terms.buckets.len() >= 4,
        "expected ≥4 extension buckets without pagination, got {}",
        terms.buckets.len()
    );
    assert!(
        terms.next_cursor.is_none(),
        "no pagination should mean no next_cursor"
    );
}

#[test]
fn last_page_has_no_next_cursor() {
    let index = test_index();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];
    // Page size of 100 is bigger than our 4 extensions → single page.
    let (results, _matched) =
        IndexManager::run_aggregations(&index, None, &specs, AggregationRequest {
            agg_page_size: Some(100),
            ..AggregationRequest::default()
        });
    let terms = results.iter().find(|r| r.kind == "buckets").unwrap();
    assert!(
        terms.next_cursor.is_none(),
        "page_size larger than total buckets should produce no next_cursor"
    );
}

// ── Aggregate cache integration ──────────────────────────────────

#[test]
fn cache_hit_on_identical_second_call() {
    use uffs_core::aggregate::AggregateCache;

    let index = test_index();
    let cache = AggregateCache::default_ttl();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];

    // First call populates the cache (miss).
    let (first, _) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest::default());
    let stats_after_first = cache.stats();
    assert_eq!(stats_after_first.misses, 1, "first call should miss");
    assert_eq!(stats_after_first.hits, 0, "first call cannot hit");
    assert_eq!(stats_after_first.entries, 1, "miss should populate cache");

    // Second call with identical inputs must hit.
    let (second, _) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest::default());
    let stats_after_second = cache.stats();
    assert_eq!(stats_after_second.hits, 1, "second call must hit");
    assert_eq!(
        stats_after_second.misses, 1,
        "miss count must not increase on a hit"
    );

    // Response shape is identical.
    assert_eq!(
        first.len(),
        second.len(),
        "hit must return same number of results as miss"
    );
    let first_buckets = first.iter().find(|r| r.kind == "buckets").unwrap();
    let second_buckets = second.iter().find(|r| r.kind == "buckets").unwrap();
    assert_eq!(
        first_buckets.buckets.len(),
        second_buckets.buckets.len(),
        "hit must return same bucket count as miss"
    );
}

#[test]
fn cache_miss_when_filter_differs() {
    use uffs_core::aggregate::AggregateCache;

    let index = test_index();
    let cache = AggregateCache::default_ttl();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];

    // Populate cache with the unfiltered query.
    let (_initial, _): (Vec<_>, u64) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest::default());
    assert_eq!(cache.stats().entries, 1);

    // Identical query with a different filter must NOT hit.
    let narrower = AggregateFilter {
        extensions: vec!["rs".to_owned()],
        ..AggregateFilter::default()
    };
    let (_narrow, _): (Vec<_>, u64) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest {
            record_filter: narrower,
            ..AggregationRequest::default()
        });
    let stats = cache.stats();
    assert_eq!(
        stats.hits, 0,
        "different filter must be treated as a different key"
    );
    assert_eq!(stats.misses, 2, "should log a second miss");
    assert_eq!(stats.entries, 2, "second miss should create a second entry");
}

#[test]
fn cache_invalidated_by_index_version_bump() {
    use uffs_core::aggregate::AggregateCache;

    let index = test_index();
    let cache = AggregateCache::default_ttl();
    let specs = [AggregateSpecWire {
        field: Some("extension".to_owned()),
        top: Some(10),
        ..spec("terms")
    }];

    // Populate cache.
    let (_seed, _): (Vec<_>, u64) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest::default());
    assert_eq!(cache.stats().entries, 1);

    // Simulate a drive mutation: version bump → cache invalidation.
    cache.set_index_version(1);
    assert!(
        cache.is_empty(),
        "set_index_version must drop stale entries"
    );

    // Second call is a miss again because the previous entry was
    // invalidated by the version change.
    let (_post_bump, _): (Vec<_>, u64) =
        IndexManager::run_aggregations(&index, Some(&cache), &specs, AggregationRequest::default());
    let stats = cache.stats();
    assert_eq!(
        stats.hits, 0,
        "call after version bump must not see a cached entry"
    );
    assert!(stats.misses >= 2, "post-bump call must be recorded as miss");
}

// ── auto-concurrency target ────────────────────────────────────────────────
//
// Lock in the `max(2, (cpus × 26) / (drives × 10))` formula for a handful of
// representative box shapes.  If anyone ever wants to retune the factor,
// these assertions will fail loudly and the developer has to update both the
// formula *and* the docstring (which quotes the 24/7 = 8 measurement).

#[test]
fn auto_concurrency_target_24x7_landed_on_8() {
    // The whole point of the 2.6× factor: the user's 24-core / 7-drive
    // Windows box's empirical sweet spot (v0.5.45 sweep, 2026-04-18).
    assert_eq!(IndexManager::auto_concurrency_target(24, 7), 8);
}

#[test]
fn auto_concurrency_target_common_box_shapes() {
    // (cpus, drives, expected_permits) — derived from
    //   max(2, floor((cpus × 26) / (drives × 10)))
    let cases: &[(usize, usize, usize)] = &[
        // Laptops & desktops.
        (4, 1, 10),  //  4 × 26 /  10 = 10   (single-drive 4-core)
        (8, 1, 20),  //  8 × 26 /  10 = 20   (single-drive 8-core)
        (8, 2, 10),  //  8 × 26 /  20 = 10
        (12, 2, 15), // 12 × 26 /  20 = 15
        (16, 2, 20), // 16 × 26 /  20 = 20
        // Developer workstations (the Mac box).
        (16, 7, 5), // 16 × 26 /  70 =  5
        // The calibration target: user's Windows box.
        (24, 7, 8), // 24 × 26 /  70 =  8   ← landed on 8 by design
        // Big storage servers.
        (32, 14, 5), // 32 × 26 / 140 =  5
        (64, 2, 83), // 64 × 26 /  20 = 83
        // Edge cases.
        (1, 1, 2),    //  1 × 26 /  10 =  2 (floor of 2 kicks in at ceil)
        (2, 1, 5),    //  2 × 26 /  10 =  5
        (24, 100, 2), // 24 × 26 /1000 =  0 → floor to 2
    ];
    for &(cpus, drives, expected) in cases {
        assert_eq!(
            IndexManager::auto_concurrency_target(cpus, drives),
            expected,
            "auto_concurrency_target({cpus}, {drives}) should be {expected}"
        );
    }
}

#[test]
fn auto_concurrency_target_zero_drives_treated_as_one() {
    // Drives count of zero is clamped up to 1 so the daemon can still
    // admit queries during the pre-load window (there is nothing to
    // scan yet, so a permit is essentially free).
    assert_eq!(
        IndexManager::auto_concurrency_target(16, 0),
        IndexManager::auto_concurrency_target(16, 1),
        "drives = 0 must be treated identically to drives = 1"
    );
}

#[test]
fn auto_concurrency_target_floor_prevents_zero_permits() {
    // Pathological shape: many drives, few CPUs.  Raw ratio rounds to
    // zero — the floor of 2 must still apply so the daemon can make
    // progress.
    assert_eq!(IndexManager::auto_concurrency_target(2, 64), 2);
    assert_eq!(IndexManager::auto_concurrency_target(1, 200), 2);
}

// ── Phase 3.1 NUL fast path: include_rows gate ──────────────────────

/// Helper: construct a minimal [`IndexManager`] with the synthetic
/// test drive loaded.  Uses `tokio::test` so the async `add_drive` can
/// swap the internal snapshot pointer.
async fn test_manager_with_drive() -> IndexManager {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr
}

/// Regression: when `include_rows = false`, [`IndexManager::search`]
/// must return a response with empty `rows`, empty `projected_rows`,
/// and `paths_blob = None` — while still populating `total_count` so
/// callers can display "N results suppressed" if they want to.
///
/// This is the daemon-side half of the Phase 3.1 NUL fast path: the
/// CLI injects `--no-output` when stdout points to the null device,
/// which sets this flag, and the daemon skips row materialisation +
/// `paths_blob` packing + IPC transfer.
#[tokio::test]
async fn search_with_include_rows_false_suppresses_rows_but_counts() {
    use uffs_client::protocol::response::SearchPayload;

    let mgr = test_manager_with_drive().await;

    let params = uffs_client::protocol::SearchParams {
        pattern: "*".to_owned(),
        include_rows: false,
        ..uffs_client::protocol::SearchParams::default()
    };

    let response = mgr.search(&params).await;

    // `include_rows = false` must leave the payload as `Empty` —
    // any other variant (InlineRows, blob, shmem) would mean the
    // daemon allocated and populated rows despite the caller opting
    // out, which is the exact overhead the flag is meant to skip.
    assert!(
        matches!(response.payload, SearchPayload::Empty),
        "include_rows=false must produce SearchPayload::Empty; got {:?}",
        response.payload
    );
    assert!(
        response.total_count > 0,
        "total_count must reflect the matched record count regardless of include_rows; got {}",
        response.total_count
    );
}

/// Control: with `include_rows = true` (the default), the same query
/// returns non-empty rows.  Pins that the gate in
/// `IndexManager::search` does not accidentally suppress the
/// non-suppressed case.
#[tokio::test]
async fn search_with_include_rows_true_returns_rows() {
    use uffs_client::protocol::response::SearchPayload;

    let mgr = test_manager_with_drive().await;

    let params = uffs_client::protocol::SearchParams {
        pattern: "*".to_owned(),
        include_rows: true,
        ..uffs_client::protocol::SearchParams::default()
    };

    let response = mgr.search(&params).await;

    // The happy path delivers `InlineRows` — the small-manager
    // fixture never breaches the `SHMEM_THRESHOLD` (100 K rows) or
    // `PATHS_BLOB_SHMEM_THRESHOLD` (512 KB) boundaries, and the `*`
    // pattern with default projection is not path-only so no
    // `InlineBlob` fast-path fires either.
    let total_count = response.total_count;
    let SearchPayload::InlineRows(rows) = response.payload else {
        panic!(
            "include_rows=true on a small fixture must deliver \
             InlineRows; got a non-rows payload variant"
        );
    };
    assert!(
        !rows.is_empty(),
        "include_rows=true must return matched rows; got 0"
    );
    assert_eq!(
        rows.len() as u64,
        total_count,
        "rows.len() must equal total_count when no limit is set and include_rows=true"
    );
}

// ── Zero-drive shutdown guard (prevents the Ready-with-no-data zombie) ──

/// Regression pin for the zero-drive guard in
/// `crate::run_daemon`'s `load_task`.  The guard keys off
/// `IndexManager::loaded_drive_letters().await.is_empty()` — if that
/// signal ever started reporting a non-empty vec for a fresh manager
/// (e.g. by accidentally seeding a placeholder drive), the guard
/// would silently stop firing and the zombie-daemon bug would
/// reappear.  This test pins the invariant the guard relies on.
///
/// The end-to-end check — that `run_daemon` actually calls
/// `request_shutdown` when every MFT parse fails — is covered by
/// `scripts/windows/api-validation.rs` which spins up a real daemon
/// with an empty `data_dir` and now observes it exit cleanly on
/// macOS/Linux instead of lingering in `Ready` with zero drives.
#[tokio::test]
async fn fresh_index_manager_reports_no_loaded_drives() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    let letters = mgr.loaded_drive_letters().await;
    assert!(
        letters.is_empty(),
        "a fresh IndexManager must report zero loaded drives — the run_daemon \
         zero-drive shutdown guard relies on this signal.  got: {letters:?}",
    );
}

// ── Pure helpers extracted from the load / refresh paths ─────────

/// `infer_drive_letter` keys MFT-snapshot file paths to a canonical
/// drive letter so the hot-load path can short-circuit when that
/// drive is already loaded.  The contract is:
///
/// * first ASCII-alphabetic character of the file stem,
/// * uppercased,
/// * `'X'` fallback for non-conforming names so callers always have a stable
///   handle to log against.
#[test]
fn infer_drive_letter_pins_canonical_mapping() {
    use std::path::Path;

    // Standard captures: `<letter>_mft.iocp`.
    assert_eq!(
        IndexManager::infer_drive_letter(Path::new("G_mft.iocp")),
        'G'
    );
    assert_eq!(
        IndexManager::infer_drive_letter(Path::new("c_mft.iocp")),
        'C'
    );

    // Lone letter, no extension.
    assert_eq!(IndexManager::infer_drive_letter(Path::new("d")), 'D');

    // Path with directory components — only the file stem matters.
    assert_eq!(
        IndexManager::infer_drive_letter(Path::new("data_dir/drive_e/E_mft.iocp")),
        'E'
    );

    // Non-conforming names fall back to 'X' rather than panicking.
    assert_eq!(
        IndexManager::infer_drive_letter(Path::new("1bad_name")),
        'X'
    );
    assert_eq!(
        IndexManager::infer_drive_letter(Path::new("_underscore.iocp")),
        'X'
    );

    // Empty path components default to 'X' (file_name() returns None).
    assert_eq!(IndexManager::infer_drive_letter(Path::new("")), 'X');
}

/// `is_live_drive_marker` distinguishes a cached source whose
/// recorded path is the bare drive marker (e.g. `"C:"`) from a real
/// on-disk MFT snapshot.  The threshold is `len <= 2` so a stray
/// trailing backslash on Windows still counts as a real path.
#[test]
fn is_live_drive_marker_recognises_cached_volume_marker() {
    use std::path::Path;

    // The two canonical live-drive markers used by the cache layer.
    assert!(IndexManager::is_live_drive_marker(Path::new("C:")));
    assert!(IndexManager::is_live_drive_marker(Path::new("d:")));
    // Single-char shorthand also classifies as live.
    assert!(IndexManager::is_live_drive_marker(Path::new("D")));

    // Anything ≥ 3 bytes is treated as an on-disk snapshot.
    assert!(!IndexManager::is_live_drive_marker(Path::new("C:\\")));
    assert!(!IndexManager::is_live_drive_marker(Path::new(
        "C:\\snap\\C_mft.iocp"
    )));
    assert!(!IndexManager::is_live_drive_marker(Path::new(
        "./C_mft.iocp"
    )));
}

// ── Phase 0 telemetry: status() surfaces RSS + mimalloc committed ──

/// `IndexManager::status` populates both `rss_bytes` and
/// `mimalloc_committed_bytes` via [`crate::telemetry::mem_snapshot`]
/// — the two new fields landed by Phase 0 of the memory-tiering
/// work.  This pin guards against a future refactor accidentally
/// dropping the wiring (which would silently zero the telemetry
/// dataset the rest of the tiering work measures itself against).
#[tokio::test]
async fn status_populates_rss_and_mimalloc_committed() {
    let mgr = test_manager_with_drive().await;
    let status = mgr.status(0).await;

    let rss = status
        .rss_bytes
        .expect("Phase 0: status must surface rss_bytes via mem_snapshot");
    assert!(
        rss > 0,
        "rss_bytes must be positive in a live test process; got {rss}"
    );

    // Committed bytes is `Option<u64>` on the wire because mimalloc's
    // `current_commit` can underflow on macOS under heavy allocation
    // churn (observed during v0.5.77 baseline capture); the daemon's
    // `sanity_clamp_committed` rejects those readings, surfacing
    // `None` instead of a `~u64::MAX` value.  Test asserts the bound
    // only when `Some` is present so it stays meaningful on every
    // platform without forcing macOS to lie.
    if let Some(committed) = status.mimalloc_committed_bytes {
        assert!(
            committed < u64::MAX / 2,
            "mimalloc_committed_bytes looks like an underflow: {committed}"
        );
    }
}

/// `IndexManager::total_index_heap_bytes` returns 0 for a fresh
/// manager (no drives loaded).  Pins the empty-state contract the
/// `mem.snapshot` heartbeat relies on so the first event after
/// startup carries a real number rather than a panic.
#[tokio::test]
async fn total_index_heap_bytes_zero_for_empty_manager() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    assert_eq!(mgr.total_index_heap_bytes().await, 0);
}

/// After [`IndexManager::add_drive`] the heap total is non-zero
/// and matches the sum reported via the per-drive breakdown in
/// `status().drive_memory`.  Pins the contract that the heartbeat
/// path and the JSON-RPC `status` path agree on the same number.
#[tokio::test]
async fn total_index_heap_bytes_matches_status_breakdown() {
    let mgr = test_manager_with_drive().await;
    let total = mgr.total_index_heap_bytes().await;
    assert!(
        total > 0,
        "loaded drive must report a positive heap; got {total}"
    );

    let status = mgr.status(0).await;
    let summed: u64 = status.drive_memory.iter().map(|dm| dm.heap_bytes).sum();
    assert_eq!(
        total, summed,
        "total_index_heap_bytes ({total}) must equal sum of \
         drive_memory.heap_bytes ({summed})"
    );
    assert_eq!(
        status.index_heap_bytes,
        Some(total),
        "status.index_heap_bytes must equal total_index_heap_bytes",
    );
}
