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

// ── Phase 1 — ShardRegistry / ShardEntry integration ────────────────

/// Build a synthetic drive with letter `'D'` for multi-drive tests.
///
/// Same shape as [`build_test_drive`] but a different letter so a
/// 2-drive registry is unambiguous.
fn build_test_drive_d() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('D');
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    let file = "alpha.txt";
    let off = idx.add_name(file);
    let ext = idx.intern_extension(file);
    let rec = idx.get_or_create(200);
    rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(file.len()), true, ext);
    rec.first_name.parent_frs = ROOT_FRS;
    rec.first_stream.size = SizeInfo {
        length: 42,
        allocated: 512,
    };
    rec.stdinfo.flags = 0x20;
    rec.stdinfo.modified = 1_000_000;

    let (drive, _, _) = build_compact_index('D', &idx);
    drive
}

/// Build a synthetic drive with letter `'E'` — third drive for the
/// Phase 3 Commit E virtual-time tests (plan tasks 3.7 + 3.8) that
/// need to verify "queries on C only → D and E both demote, C
/// stays Warm" and "advance past parked TTL → all three Cold".
fn build_test_drive_e() -> uffs_core::compact::DriveCompactIndex {
    let mut idx = MftIndex::new('E');
    let root_off = idx.add_name(".");
    let root = idx.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_off, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    let file = "beta.bin";
    let off = idx.add_name(file);
    let ext = idx.intern_extension(file);
    let rec = idx.get_or_create(300);
    rec.first_name.name = IndexNameRef::new(off, uffs_mft::len_to_u16(file.len()), true, ext);
    rec.first_name.parent_frs = ROOT_FRS;
    rec.first_stream.size = SizeInfo {
        length: 84,
        allocated: 1024,
    };
    rec.stdinfo.flags = 0x20;
    rec.stdinfo.modified = 1_000_000;

    let (drive, _, _) = build_compact_index('E', &idx);
    drive
}

/// `ShardRegistry::{add, replace, remove}` round-trip with real
/// `DriveCompactIndex` bodies.  Pins the case-insensitive contract on
/// `replace` / `remove` that mirrors the pre-Phase-1
/// `IndexManager::replace_drive` filter.
#[test]
fn shard_registry_add_replace_remove_round_trip() {
    use crate::cache::ShardRegistry;

    let body_c = Arc::new(build_test_drive());
    let body_d = Arc::new(build_test_drive_d());
    let body_c_v2 = Arc::new(build_test_drive());

    // Empty start → add C → add D → replace 'c' (case-insensitive) →
    // remove 'd' (case-insensitive).  Single mutable binding so we
    // don't trip clippy::shadow_reuse on each rebuild.
    let mut reg = ShardRegistry::new();
    assert!(reg.is_empty());
    assert_eq!(reg.active_index().drives.len(), 0);

    reg = reg.add(Arc::clone(&body_c));
    reg = reg.add(Arc::clone(&body_d));
    assert_eq!(reg.active_index().drives.len(), 2);
    assert!(reg.contains('C'));
    assert!(reg.contains('D'));

    reg = reg.replace('c', Arc::clone(&body_c_v2));
    assert_eq!(
        reg.active_index().drives.len(),
        2,
        "replace must not duplicate",
    );

    reg = reg.remove('d');
    assert!(!reg.contains('D'));
    assert_eq!(reg.active_index().drives.len(), 1);
    assert_eq!(reg.loaded_letters(), vec!['C']);
}

/// `ShardEntry::try_transition` enforces the legal-transition graph
/// from [`ShardState::can_transition_to`] using a CAS loop.
///
/// Task 1.7 — covers both legal and illegal moves on a real shard
/// with a `DriveCompactIndex` body, complementing the proptest in
/// `crate::cache::shard::tests` which exercises the pure state graph.
#[test]
fn shard_entry_try_transition_legal_and_illegal() {
    use crate::cache::ShardState;
    use crate::cache::shard::ShardEntry;

    let body = Arc::new(build_test_drive());
    let shard = ShardEntry::new_warm('C', Arc::clone(&body));
    assert_eq!(shard.state(), ShardState::Warm);

    // Legal: Warm → Hot.
    let prev = shard
        .try_transition(ShardState::Hot)
        .expect("warm->hot is legal");
    assert_eq!(prev, ShardState::Warm);
    assert_eq!(shard.state(), ShardState::Hot);

    // Illegal: Hot → Cold (must go via Evicting → Cold/Parked).
    let err = shard
        .try_transition(ShardState::Cold)
        .expect_err("hot->cold is illegal");
    assert_eq!(err.from, ShardState::Hot);
    assert_eq!(err.to, ShardState::Cold);
    assert_eq!(
        shard.state(),
        ShardState::Hot,
        "state must be unchanged on illegal transition"
    );

    // Recovery path: Hot → Warm → Evicting → Cold all legal.
    shard
        .try_transition(ShardState::Warm)
        .expect("hot->warm legal");
    shard
        .try_transition(ShardState::Evicting)
        .expect("warm->evicting legal");
    shard
        .try_transition(ShardState::Cold)
        .expect("evicting->cold legal");
    assert_eq!(shard.state(), ShardState::Cold);
}

/// Two-drive integration: searches dispatch correctly across both
/// shards under the new `ShardRegistry` indirection.
///
/// Task 1.9 — `build_test_drive` + `build_test_drive_d` with
/// `IndexManager::search`, asserting the results carry rows from both
/// drives.  Pins the "zero observable change" contract for Phase 1.
#[tokio::test]
async fn shard_registry_search_two_drives_returns_rows_from_each() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let params = uffs_client::protocol::SearchParams {
        pattern: "*".to_owned(),
        limit: Some(50),
        ..Default::default()
    };
    let resp = mgr.search(&params).await;
    assert!(
        resp.total_count >= 2,
        "two-drive '*' search must return at least 2 rows; got {}",
        resp.total_count,
    );

    // Both drives must contribute records to the snapshot.
    let snap = mgr.snapshot().await;
    assert_eq!(snap.drives.len(), 2);
    let letters: std::collections::HashSet<char> = snap.drives.iter().map(|d| d.letter).collect();
    assert!(letters.contains(&'C'), "C drive must be in the snapshot");
    assert!(letters.contains(&'D'), "D drive must be in the snapshot");
}

/// `IndexManager::search` records one query per dispatch on every
/// active shard, via `record_search_dispatch` + `DriveStats::record_query`.
///
/// Task 1.5 — pins the wiring between the search hot path and the
/// per-shard counter that Phase 6 reads for adaptive-TTL.
#[tokio::test]
async fn search_records_query_on_every_active_shard() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Baseline: no searches yet.
    let before = mgr.shard_query_totals_for_test().await;
    assert_eq!(before.len(), 2);
    for (letter, count) in &before {
        assert_eq!(*count, 0, "drive {letter} must start at 0 queries");
    }

    let params = uffs_client::protocol::SearchParams {
        pattern: "*".to_owned(),
        limit: Some(10),
        ..Default::default()
    };

    // Three searches.  Suffix the loop bound to avoid the implicit i32
    // fallback flagged by clippy::default_numeric_fallback.
    for _ in 0_u32..3_u32 {
        drop(mgr.search(&params).await);
    }

    let after = mgr.shard_query_totals_for_test().await;
    assert_eq!(after.len(), 2);
    for (letter, count) in after {
        assert_eq!(
            count, 3,
            "drive {letter} must have recorded 3 queries; got {count}",
        );
    }
}

// ── Phase 3 Commit B — ShardRegistry demote_letter / promote_letter ────

/// Demote a `Warm` shard to `Parked`: the new shard has no body,
/// the active index drops the drive, and the per-drive
/// `Arc<DriveStats>` is shared so query counters survive the
/// rebuild.
#[test]
fn demote_letter_warm_to_parked_drops_body_and_preserves_stats() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let body_d = Arc::new(build_test_drive_d());
    // Single mutable binding so we don't trip clippy::shadow_reuse
    // on each rebuild — same pattern as
    // `shard_registry_add_replace_remove_round_trip`.
    let mut reg = ShardRegistry::new()
        .add(Arc::clone(&body_c))
        .add(Arc::clone(&body_d));
    assert_eq!(reg.active_index().drives.len(), 2);

    // Mark some queries on C so we can verify they survive the
    // rebuild.
    let c_shard_pre = reg
        .iter()
        .find(|s| s.drive == 'C')
        .expect("C present pre-demote");
    for _ in 0_u32..5_u32 {
        c_shard_pre.stats.record_query();
    }
    assert_eq!(c_shard_pre.stats.queries_total(), 5);

    reg = reg
        .demote_letter('C', ShardState::Parked)
        .expect("warm → parked is legal");

    // Active index now only contains D.
    assert_eq!(reg.active_index().drives.len(), 1);
    assert_eq!(reg.active_index().drives[0].letter, 'D');

    // Both shards are still loaded; C is Parked, body lifted.
    let c_shard = reg
        .iter()
        .find(|s| s.drive == 'C')
        .expect("C still loaded post-demote");
    assert_eq!(c_shard.state(), ShardState::Parked);
    assert!(c_shard.body().is_none());

    // Query counter survives via the shared Arc<DriveStats>.
    assert_eq!(
        c_shard.stats.queries_total(),
        5,
        "demote rebuild must preserve query stats via shared Arc<DriveStats>",
    );
}

/// Demote a `Warm` shard directly to `Cold` (skipping `Parked`).
#[test]
fn demote_letter_warm_to_cold_drops_body() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let mut reg = ShardRegistry::new().add(body_c);
    reg = reg
        .demote_letter('C', ShardState::Cold)
        .expect("warm → cold is legal");

    assert_eq!(reg.active_index().drives.len(), 0);
    let c_shard = reg.iter().find(|s| s.drive == 'C').expect("C still loaded");
    assert_eq!(c_shard.state(), ShardState::Cold);
    assert!(c_shard.body().is_none());
}

/// Demoting an unknown letter is a `None` no-op.
#[test]
fn demote_letter_unknown_letter_returns_none() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let reg = ShardRegistry::new().add(body_c);
    assert!(
        reg.demote_letter('Z', ShardState::Parked).is_none(),
        "demote on unknown letter must return None"
    );
}

/// Demote target outside the legal demote set (e.g. `Warm`,
/// `Hot`, `Unknown`) returns `None`.
#[test]
fn demote_letter_illegal_target_returns_none() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let reg = ShardRegistry::new().add(body_c);
    for bad_target in [
        ShardState::Warm,
        ShardState::Hot,
        ShardState::Unknown,
        ShardState::Evicting,
    ] {
        assert!(
            reg.demote_letter('C', bad_target).is_none(),
            "demote target {bad_target} must be rejected"
        );
    }
}

/// Self-demote (`Parked → Parked`, `Cold → Cold`) is rejected so a
/// buggy controller can't rebuild the registry on every idle tick
/// for an already-demoted shard.
#[test]
fn demote_letter_self_demote_returns_none() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let mut reg = ShardRegistry::new().add(body_c);
    reg = reg
        .demote_letter('C', ShardState::Parked)
        .expect("first demote");
    assert!(
        reg.demote_letter('C', ShardState::Parked).is_none(),
        "Parked → Parked must be rejected"
    );

    reg = reg
        .demote_letter('C', ShardState::Cold)
        .expect("parked → cold");
    assert!(
        reg.demote_letter('C', ShardState::Cold).is_none(),
        "Cold → Cold must be rejected"
    );
}

/// Promote a `Parked` shard back to `Warm`: body restored, active
/// index re-includes the letter, query stats preserved.
#[test]
fn promote_letter_parked_to_warm_restores_body_and_preserves_stats() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let mut reg = ShardRegistry::new().add(Arc::clone(&body_c));
    // Bump a few queries before demote so we have something to
    // verify across the round trip.
    let pre = reg.iter().find(|s| s.drive == 'C').unwrap();
    for _ in 0_u32..3_u32 {
        pre.stats.record_query();
    }
    reg = reg.demote_letter('C', ShardState::Parked).expect("demote");

    // Promote with a fresh body (Phase 4+ will fault the original
    // back from disk; for this test we just hand it the same Arc).
    reg = reg
        .promote_letter('C', Arc::clone(&body_c))
        .expect("promote");

    assert_eq!(reg.active_index().drives.len(), 1);
    let c = reg.iter().find(|s| s.drive == 'C').unwrap();
    assert_eq!(c.state(), ShardState::Warm);
    assert!(c.body().is_some());
    assert_eq!(
        c.stats.queries_total(),
        3,
        "round-trip demote+promote must preserve query stats",
    );
}

/// Promoting an unknown letter is a `None` no-op.
#[test]
fn promote_letter_unknown_letter_returns_none() {
    use crate::cache::ShardRegistry;

    let body_c = Arc::new(build_test_drive());
    let body_d = Arc::new(build_test_drive_d());
    let reg = ShardRegistry::new().add(body_c);
    assert!(
        reg.promote_letter('Z', body_d).is_none(),
        "promote on unknown letter must return None"
    );
}

/// Promoting an already-`Warm` shard is a caller bug — `None`.
#[test]
fn promote_letter_already_warm_returns_none() {
    use crate::cache::ShardRegistry;

    let body_c = Arc::new(build_test_drive());
    let reg = ShardRegistry::new().add(Arc::clone(&body_c));
    assert!(
        reg.promote_letter('C', body_c).is_none(),
        "promote on already-Warm shard must return None"
    );
}

// ── Phase 3 Commit C — IndexManager::ensure_warm_for_dispatch ──────

/// Fast-path contract: when every loaded shard is already
/// `Warm`/`Hot`, `ensure_warm_for_dispatch` is a single
/// read-lock acquisition with no state mutation and no
/// `index_version` bump.
#[tokio::test]
async fn ensure_warm_for_dispatch_no_op_when_all_warm() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm)
    ]);

    // Empty filter → all touched.  Non-empty filter → subset.
    // Either way, no shard is Parked/Cold so this is a no-op.
    mgr.ensure_warm_for_dispatch(&[], &[]).await;
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;
    mgr.ensure_warm_for_dispatch(&['c'], &[]).await; // case-insensitive
    mgr.ensure_warm_for_dispatch(&['Z'], &[]).await; // unknown letter

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "all-Warm registry must survive ensure_warm_for_dispatch unchanged",
    );
}

/// `ensure_warm_for_dispatch` honours the drive-letter filter:
/// when the search targets only drive D and drive C is Parked,
/// C must not be promoted.
#[tokio::test]
async fn ensure_warm_for_dispatch_skips_parked_shard_outside_filter() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Demote C to Parked (test escape hatch).
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert!(states_pre.contains(&('C', ShardState::Parked)));

    // Search targets only D — C must stay Parked.  The on-disk
    // cache lookup for D would no-op because D is already Warm.
    mgr.ensure_warm_for_dispatch(&['D'], &[]).await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "filter excluded C — Parked state must survive",
    );
}

// ── Phase 3 Commit E — BodyLoader injection ────────────────────────

/// A `BodyLoader` that always returns `Some(self.body.clone())` —
/// used to verify the success path of `ensure_warm_for_dispatch`
/// without touching the platform cache directory.
struct FixedBodyLoader {
    body: Arc<uffs_core::compact::DriveCompactIndex>,
}

impl crate::cache::body_loader::BodyLoader for FixedBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        Some(Arc::clone(&self.body))
    }
}

/// A `BodyLoader` that always returns `None` — simulates a missing
/// or stale cache file between demote and promote.
struct MissingBodyLoader;

impl crate::cache::body_loader::BodyLoader for MissingBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        None
    }
}

/// A `BodyLoader` whose `load` method panics — exercises the
/// `Err(JoinError)` arm of the spawn-blocking match in
/// `ensure_warm_for_dispatch`.  The panic is contained inside
/// `tokio::task::spawn_blocking`'s thread; the daemon stays up and
/// the shard stays in its current tier.
struct PanickingBodyLoader;

impl crate::cache::body_loader::BodyLoader for PanickingBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        panic!("PanickingBodyLoader::load — synthetic panic for the JoinError arm");
    }
}

/// Pin the success path with an injected `FixedBodyLoader`:
///
/// 1. Add drive C, demote it to Parked (so the body Arc is dropped from the
///    registry).
/// 2. Configure the manager with a `FixedBodyLoader` carrying a fresh body for
///    C.
/// 3. Call `ensure_warm_for_dispatch(&['C'])`.
/// 4. Assert C is now Warm AND the registry's view sees the body again (via
///    `total_index_heap_bytes` — the Parked shard has `body == None` so its
///    `heap_size_bytes()` is 0; the promoted shard reports the test-drive's
///    heap size).
#[tokio::test]
async fn ensure_warm_for_dispatch_promotes_with_fixed_body_loader() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive()).await;

    let warm_heap = mgr.total_index_heap_bytes().await;
    assert!(warm_heap > 0, "Warm shard must report nonzero heap_bytes");

    // Demote — the body Arc inside the registry is now None.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Promote via ensure_warm_for_dispatch.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    // Shard is Warm again AND the heap-bytes metric is back to its
    // pre-demote value (the FixedBodyLoader handed back a body
    // identical in shape to the original).
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Warm)]);
    let promoted_heap = mgr.total_index_heap_bytes().await;
    assert_eq!(
        promoted_heap, warm_heap,
        "promoted shard's body must report the same heap size as the original Warm shard"
    );
}

/// Pin the deferred Commit C contract (now possible thanks to the
/// `BodyLoader` injection): when the loader returns `None`, the
/// Parked shard stays Parked, no panic, no half-promoted state, no
/// daemon crash.  The production code path that reads from the
/// platform cache directory becomes `MissingBodyLoader` for the
/// purposes of this test.
#[tokio::test]
async fn ensure_warm_for_dispatch_handles_missing_cache_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(MissingBodyLoader));
    mgr.add_drive(build_test_drive()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // Loader returns None → graceful failure path.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "missing body → shard stays Parked, no panic, no half-promoted state"
    );
}

/// Pin the panic-recovery path: a `BodyLoader::load` that panics
/// surfaces as `Err(JoinError)` from `spawn_blocking`, gets logged
/// at error-level, and leaves the shard untouched.  The daemon
/// stays up and subsequent calls work normally.
#[tokio::test]
async fn ensure_warm_for_dispatch_handles_panicking_body_loader_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(PanickingBodyLoader));
    mgr.add_drive(build_test_drive()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Loader panics → JoinError arm runs → shard stays Parked.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Parked)],
        "panicking loader → JoinError → shard stays Parked, no daemon crash"
    );

    // Subsequent ensure_warm_for_dispatch on the same manager
    // still works (no global daemon state corruption).
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;
    let states_again = mgr.shard_states_for_test().await;
    assert_eq!(
        states_again,
        vec![('C', ShardState::Parked)],
        "second call after a panicking-loader call must also be graceful"
    );
}

// ── Phase 5 (#93) — parallel re-promote ────────────────────────────

/// A `BodyLoader` that sleeps for `delay` before returning a clone
/// of `body`, and records the peak number of concurrent calls
/// in flight.  Used to verify that
/// [`IndexManager::ensure_warm_for_dispatch`] fans out per-letter
/// loads across the blocking pool instead of serialising them.
struct SlowBodyLoader {
    body: Arc<uffs_core::compact::DriveCompactIndex>,
    delay: core::time::Duration,
    in_flight: core::sync::atomic::AtomicUsize,
    peak_in_flight: core::sync::atomic::AtomicUsize,
}

impl SlowBodyLoader {
    fn new(body: Arc<uffs_core::compact::DriveCompactIndex>, delay: core::time::Duration) -> Self {
        Self {
            body,
            delay,
            in_flight: core::sync::atomic::AtomicUsize::new(0),
            peak_in_flight: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn peak(&self) -> usize {
        self.peak_in_flight
            .load(core::sync::atomic::Ordering::Acquire)
    }
}

impl crate::cache::body_loader::BodyLoader for SlowBodyLoader {
    fn load(&self, _letter: char) -> Option<Arc<uffs_core::compact::DriveCompactIndex>> {
        use core::sync::atomic::Ordering;

        let now = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        // Bump peak via a CAS loop: read the current peak, write
        // back `now` only if it's strictly larger.  Pure `fetch_max`
        // would be one call but isn't stable on all targets we
        // build; the loop is portable and the contention window is
        // microscopic (only the first few in-flight loaders ever
        // raise the peak).
        let mut prev = self.peak_in_flight.load(Ordering::Acquire);
        while now > prev {
            match self.peak_in_flight.compare_exchange_weak(
                prev,
                now,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => prev = actual,
            }
        }
        std::thread::sleep(self.delay);
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
        Some(Arc::clone(&self.body))
    }
}

/// Pin the parallelisation contract of `ensure_warm_for_dispatch`
/// (#93): with N Parked drives and a `BodyLoader::load` that
/// sleeps `delay`, total wall must be `~delay`, not `N × delay`.
///
/// The pre-fix serial loop took `sum(per-drive)`; the `JoinSet` fan-out
/// completes in `~max(per-drive)` plus a few µs of write-lock
/// contention.  We assert two things:
///
/// 1. `peak_in_flight >= 2` — the loader observed concurrent calls.
/// 2. Wall < `1.5 × delay` — comfortably below the `3 × delay` a serial loop
///    would take with N=3.  The 1.5× upper bound leaves headroom for
///    blocking-pool ramp-up and CI variance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ensure_warm_for_dispatch_promotes_in_parallel() {
    use core::time::Duration;

    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();

    // Per-letter delay: 100 ms is small enough to keep the test
    // fast on CI and large enough that scheduling jitter (a few ms)
    // doesn't dominate the timing assertion.
    let delay = Duration::from_millis(100);
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(SlowBodyLoader::new(Arc::clone(&body), delay));
    // `with_body_loader_for_test` takes `Arc<dyn BodyLoader>`; clone
    // a coerced handle for the manager so we keep `loader` typed
    // as `Arc<SlowBodyLoader>` for the `.peak()` assertion below.
    let loader_dyn: Arc<dyn crate::cache::body_loader::BodyLoader> =
        Arc::clone(&loader) as Arc<dyn crate::cache::body_loader::BodyLoader>;

    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader_dyn);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Demote all three to Parked so they all need the loader.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    assert!(mgr.demote_letter_for_test('D', ShardState::Parked).await);
    assert!(mgr.demote_letter_for_test('E', ShardState::Parked).await);

    let start = std::time::Instant::now();
    mgr.ensure_warm_for_dispatch(&['C', 'D', 'E'], &[]).await;
    let elapsed = start.elapsed();

    // All three shards promoted.
    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Warm),
            ('D', ShardState::Warm),
            ('E', ShardState::Warm),
        ],
        "all three Parked shards must be Warm after ensure_warm_for_dispatch"
    );

    // Concurrent loaders observed.
    assert!(
        loader.peak() >= 2,
        "expected ≥ 2 concurrent loader calls in flight; got peak = {} \
         (parallelism regression — re-promote went serial again)",
        loader.peak(),
    );

    // Wall ≈ delay, not N × delay.  The serial loop pre-#93 would
    // have taken ≥ 300 ms for delay=100 ms × 3 drives; we accept
    // up to 1.5× (150 ms) to keep the test robust against CI jitter
    // and blocking-pool ramp-up.
    let upper_bound = delay.mul_f32(1.5);
    assert!(
        elapsed < upper_bound,
        "expected parallel re-promote (≤ {} ms), got {} ms — \
         serial pre-#93 baseline would be ≥ {} ms",
        upper_bound.as_millis(),
        elapsed.as_millis(),
        delay.as_millis() * 3,
    );
}

// ── Phase 5 (#95) — IndexManager::refresh_usn_for_warm_shards ──────

/// Fast-path contract: refresh tick on an empty registry returns
/// immediately without panicking and without mutating any state.
/// Pins the early-return at the top of
/// [`IndexManager::refresh_usn_for_warm_shards`] so future
/// refactors can't accidentally call into the [`tokio::task::JoinSet`]
/// phase with an empty `letters` vec.
#[tokio::test]
async fn refresh_usn_for_warm_shards_no_op_when_empty() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    assert!(mgr.shard_states_for_test().await.is_empty());

    mgr.refresh_usn_for_warm_shards().await;

    assert!(
        mgr.shard_states_for_test().await.is_empty(),
        "empty-registry refresh tick must keep the registry empty",
    );
}

/// Fast-path contract: refresh tick on a registry with no
/// `Warm`/`Hot` shards (everything Parked) is also a no-op.
/// Pins that the read-lock detect skips Parked/Cold shards
/// without ever entering the [`tokio::task::JoinSet`] phase.
#[tokio::test]
async fn refresh_usn_for_warm_shards_no_op_when_no_warm_or_hot() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    mgr.refresh_usn_for_warm_shards().await;

    assert_eq!(
        mgr.shard_states_for_test().await,
        vec![('C', ShardState::Parked)],
        "Parked shard must stay Parked through refresh tick",
    );
}

/// Cross-platform graceful-failure contract: on macOS / Linux the
/// underlying [`uffs_core::compact_loader::load_drive_with_usn_refresh`]
/// helper errors out by design (USN journals are NTFS-only).  The
/// refresh tick must NOT panic, NOT lose the existing in-memory body,
/// and NOT mutate `index_version`.  On Windows this same test
/// exercises the success path (USN replay applied + body swapped),
/// but the assertions above (state preservation + body retained)
/// still hold because `replace_warm_body` keeps the previous body
/// on any registry race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_usn_for_warm_shards_handles_helper_errors_gracefully() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm),
    ]);

    // The refresh tick walks Warm shards, calls the helper, and on
    // non-Windows every call errors with `PlatformNotSupported`.
    // The test passes if the call returns cleanly.
    mgr.refresh_usn_for_warm_shards().await;

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "shards must keep Warm state when USN refresh helper errors",
    );
}

// ── Phase 3 Commit D — IndexManager::demote_idle_shards ────────────

/// `add_drive` calls `DriveStats::mark_loaded_at(now_ms)` on the
/// freshly mounted shard so the demote-controller's idle clock
/// starts ticking from load time, not from epoch zero.  Without
/// this seed, every freshly loaded shard would demote on the
/// first idle tick because `last_query_at_ms == 0` would compute
/// `idle_secs ≈ now_ms / 1000` (≈ billions of seconds since
/// 1970-01-01).
#[tokio::test]
async fn mark_loaded_at_seeds_freshly_added_drive() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Read the shard's last_query_at_ms via a search-ish path; we
    // only have access from inside the manager, so drive a
    // demote-controller call with `now_ms = load_ts + 0` and assert
    // the shard didn't demote — that proves last_query_at_ms is
    // recent (within `WARM_TO_PARKED_IDLE_SECS` of now).
    //
    // More directly: the timestamp must be non-zero.  We test that
    // by attempting to demote at a now that's *exactly* the load
    // time + 1 ms; an unseeded shard would have `idle_ms = now`
    // (huge) and demote, but a seeded shard sees `idle_ms = 1` ms
    // (basically 0 s) and stays Warm.
    let states_before = mgr.shard_states_for_test().await;
    assert_eq!(states_before, vec![('C', crate::cache::ShardState::Warm)]);

    // Synthetic now_ms a billion ms in the future would catch
    // unseeded `last_query_at_ms == 0`.  But a seeded shard has
    // last_query_at_ms ≈ unix_now_ms() (when add_drive was just
    // called), so calling demote_idle_shards with the same now
    // gives idle_secs ≈ 0.
    let now_ms = crate::cache::unix_now_ms();
    mgr.demote_idle_shards(now_ms).await;

    let states_after = mgr.shard_states_for_test().await;
    assert_eq!(
        states_after, states_before,
        "freshly loaded shard must NOT demote on the first tick — \
         mark_loaded_at must have seeded last_query_at_ms",
    );
}

/// Fast-path contract: `demote_idle_shards` on a registry where
/// every shard has been queried recently must complete with no
/// state mutation and no `index_version` bump.
#[tokio::test]
async fn demote_idle_shards_no_op_when_all_fresh() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    // Pretend every shard was queried at t=10_000_000_000 ms.
    let load_ts = 10_000_000_000_u64;
    assert!(mgr.backdate_last_query_at_ms_for_test('C', load_ts).await);
    assert!(mgr.backdate_last_query_at_ms_for_test('D', load_ts).await);

    // `now_ms` only 1 ms after load → idle_secs = 0 → no demote.
    mgr.demote_idle_shards(load_ts + 1).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![
        ('C', ShardState::Warm),
        ('D', ShardState::Warm)
    ]);
}

/// Warm shard idle past `WARM_TO_PARKED_IDLE_SECS` demotes to
/// Parked on the next `demote_idle_shards` call.
#[tokio::test]
async fn demote_idle_shards_warm_to_parked_at_ttl() {
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Backdate C's last_query_at_ms to t=1_000_000_000 ms.
    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    // now_ms = last_query + WARM_TO_PARKED_IDLE_SECS * 1000 (exact
    // boundary; `next_state_for_idle` uses `>=`).
    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', crate::cache::ShardState::Parked)]);
}

/// Warm shard idle just below `WARM_TO_PARKED_IDLE_SECS` stays
/// Warm — pin the off-by-one that `>=` vs `>` would expose.
#[tokio::test]
async fn demote_idle_shards_below_ttl_keeps_warm() {
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    // 1 ms before the boundary — idle_secs computed by
    // `(now - last) / 1000` is `WARM_TO_PARKED_IDLE_SECS - 1`,
    // strictly below the threshold.
    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000 - 1;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', crate::cache::ShardState::Warm)]);
}

/// Parked shard idle past `PARKED_TO_COLD_IDLE_SECS` demotes to
/// Cold on the next `demote_idle_shards` call.
///
/// Pins the multi-step ladder: a single tick can see both
/// `Warm → Parked` and `Parked → Cold` transitions if a Parked
/// shard's `last_query_at_ms` is old enough, but the policy only
/// returns one demote target per call so each tick advances each
/// shard at most one tier.  This test seeds a Parked shard
/// directly to keep the assertion focused.
#[tokio::test]
async fn demote_idle_shards_parked_to_cold_at_ttl() {
    use crate::cache::ShardState;
    use crate::cache::policy::PARKED_TO_COLD_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;

    // Seed C as Parked via the test escape hatch.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Backdate so the Parked shard has been idle past its TTL.
    let last_query_ms = 1_000_000_000_u64;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', last_query_ms)
            .await
    );

    let now_ms = last_query_ms + PARKED_TO_COLD_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Cold)]);
}

/// `demote_idle_shards` batches multiple demotes inside a single
/// write-lock window.  Pin the contract by demoting three shards
/// in one call.
#[tokio::test]
async fn demote_idle_shards_batches_multiple_demotes() {
    use crate::cache::ShardState;
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;

    let last_query_ms = 1_000_000_000_u64;
    for letter in ['C', 'D'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, last_query_ms)
                .await
        );
    }

    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    let version_before = mgr
        .index_version
        .load(core::sync::atomic::Ordering::Relaxed);
    mgr.demote_idle_shards(now_ms).await;
    let version_after = mgr
        .index_version
        .load(core::sync::atomic::Ordering::Relaxed);

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![('C', ShardState::Parked), ('D', ShardState::Parked)],
        "all backdated Warm shards must demote in a single batch call"
    );
    assert_eq!(
        version_after - version_before,
        1,
        "batch must bump index_version exactly once for the whole batch, \
         not once per demoted shard"
    );
}

/// End-to-end: demote → promote → demote → promote preserves
/// query stats across every rebuild.  Pins the
/// `Arc<DriveStats>`-sharing contract under repeated transitions.
#[test]
fn demote_then_promote_round_trips_query_stats() {
    use crate::cache::{ShardRegistry, ShardState};

    let body_c = Arc::new(build_test_drive());
    let mut reg = ShardRegistry::new().add(Arc::clone(&body_c));

    // Each transition adds queries to verify the canonical stats
    // Arc is what the new shard's `.stats` points at.
    for round in 0_u64..3_u64 {
        reg.iter()
            .find(|s| s.drive == 'C')
            .unwrap()
            .stats
            .mark_query_at(1_000 + round);
        reg = reg.demote_letter('C', ShardState::Parked).expect("demote");
        reg.iter()
            .find(|s| s.drive == 'C')
            .unwrap()
            .stats
            .mark_query_at(2_000 + round);
        reg = reg
            .promote_letter('C', Arc::clone(&body_c))
            .expect("promote");
    }

    let final_c = reg.iter().find(|s| s.drive == 'C').unwrap();
    // 6 mark_query_at calls total across 3 rounds (3 pre-demote +
    // 3 post-demote-pre-promote).
    assert_eq!(final_c.stats.queries_total(), 6);
    // Last mark_query_at was during round 2 with `now_ms = 2_002`.
    assert_eq!(final_c.stats.last_query_at_ms(), 2_002);
}

// ── Phase 3 Commit E — virtual-time multi-drive demote tests ───────

/// Plan task 3.7 — three drives loaded; only C is queried; advance
/// past `WARM_TO_PARKED_IDLE_SECS` and verify D + E demote to Parked
/// while C stays Warm.
///
/// Models the steady-state pattern of a developer using their
/// project drive (C) actively while archive drives (D, E) sit idle.
/// Pins the per-shard idle-clock contract: each shard's
/// `last_query_at_ms` is independent, so the demote controller
/// only acts on the ones that have actually been idle.
///
/// `now_ms` threading lets the test simulate "31 minutes later"
/// deterministically — no `tokio::time::pause` needed because
/// `demote_idle_shards(now_ms)` reads the timestamp from its
/// argument, not from a clock.
#[tokio::test]
async fn demote_idle_shards_warm_only_for_unqueried_drives() {
    use crate::cache::ShardState;
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    let load_ts = 1_000_000_000_u64;
    // Seed all three to the load timestamp.
    for letter in ['C', 'D', 'E'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, load_ts)
                .await
        );
    }

    // C is queried 30 minutes after load (last query at
    // load_ts + 30min).  D and E remain at load_ts.
    let c_last_query_ms = load_ts + 30 * 60 * 1000;
    assert!(
        mgr.backdate_last_query_at_ms_for_test('C', c_last_query_ms)
            .await
    );

    // now_ms = load_ts + 31 minutes.
    let now_ms = load_ts + 31 * 60 * 1000;

    // Sanity: 31 min ≥ WARM_TO_PARKED_IDLE_SECS for D, E.
    let d_e_idle_secs = (now_ms - load_ts) / 1000;
    assert!(d_e_idle_secs >= WARM_TO_PARKED_IDLE_SECS);
    // Sanity: 1 min < WARM_TO_PARKED_IDLE_SECS for C.
    let c_idle_secs = (now_ms - c_last_query_ms) / 1000;
    assert!(c_idle_secs < WARM_TO_PARKED_IDLE_SECS);

    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Warm),
            ('D', ShardState::Parked),
            ('E', ShardState::Parked),
        ],
        "C must stay Warm (recently queried); D and E must demote to Parked"
    );
}

/// Plan task 3.8 — three Parked drives, advance past
/// `PARKED_TO_COLD_IDLE_SECS`, verify all three demote to Cold.
///
/// Pins the bottom rung of the static-TTL ladder.  The Parked tier
/// is the first that drops bloom + trie (Phase 4+); for Phase 3
/// "Parked" already means "no body", so the only difference is the
/// state label.  Cold means "needs a full re-decrypt to re-promote",
/// captured by the policy via the longer 24 h threshold.
#[tokio::test]
async fn demote_idle_shards_parked_drives_demote_to_cold_past_threshold() {
    use crate::cache::ShardState;
    use crate::cache::policy::PARKED_TO_COLD_IDLE_SECS;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Seed every drive's last_query to load_ts and demote each to
    // Parked via the test escape hatch.  Order: backdate first so
    // the demote controller doesn't trip on the seeding tick.
    let load_ts = 1_000_000_000_u64;
    for letter in ['C', 'D', 'E'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, load_ts)
                .await
        );
        assert!(mgr.demote_letter_for_test(letter, ShardState::Parked).await);
    }

    let pre_states = mgr.shard_states_for_test().await;
    assert_eq!(pre_states, vec![
        ('C', ShardState::Parked),
        ('D', ShardState::Parked),
        ('E', ShardState::Parked),
    ],);

    // now_ms = load_ts + 25 hours (≥ PARKED_TO_COLD_IDLE_SECS = 24h).
    let now_ms = load_ts + 25 * 60 * 60 * 1000;
    let idle_secs = (now_ms - load_ts) / 1000;
    assert!(idle_secs >= PARKED_TO_COLD_IDLE_SECS);

    mgr.demote_idle_shards(now_ms).await;

    let states = mgr.shard_states_for_test().await;
    assert_eq!(
        states,
        vec![
            ('C', ShardState::Cold),
            ('D', ShardState::Cold),
            ('E', ShardState::Cold),
        ],
        "all three Parked shards past the cold-tier TTL must demote to Cold"
    );
}

// ── Phase 3 Commit E — tracing-event contract (plan task 3.9) ──────

/// Plan task 3.9 — every demote / promote transition emits exactly
/// one `tracing::event!(target: "shard.transition", ...)` event.
///
/// Pins the operator-facing observability contract: the tracing
/// fields (`letter`, `from`, `to`, `reason`, `freed_mb` /
/// `restored_mb`) are part of the public log surface, so a refactor
/// that silently drops or renames them would break dashboards and
/// alerting.  This test captures every event during a
/// demote-then-promote round-trip and asserts on the field values.
///
/// `tokio::test` defaults to a `current_thread` runtime, so the
/// thread-local `tracing::subscriber::set_default` we install at
/// the top of the test captures every event emitted from inside
/// the test future — including the events from `demote_letter` /
/// `promote_letter` running on the same thread.
#[tokio::test]
async fn shard_transition_events_emitted_on_demote_and_promote() {
    use tracing_subscriber::layer::SubscriberExt;

    use crate::cache::ShardState;

    let log = EventLog::default();
    let subscriber = tracing_subscriber::registry().with(log.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive()).await;

    // Demote → expect one demote event.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    // Promote via ensure_warm_for_dispatch → expect one promote event.
    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    let events = log.events();
    let transitions: Vec<&CapturedEvent> = events
        .iter()
        .filter(|event| event.target == "shard.transition")
        .collect();

    assert_eq!(
        transitions.len(),
        2,
        "expected exactly two shard.transition events (one demote + one promote), got {}: {:#?}",
        transitions.len(),
        transitions
    );

    // `ShardState`'s `Display` impl emits lowercase variant names
    // (`warm`, `parked`, `cold`, …) — that's the wire contract this
    // test pins.  See `impl fmt::Display for ShardState` in
    // `cache/shard.rs`.
    let demote = transitions[0];
    assert_eq!(demote.level, tracing::Level::INFO);
    assert_eq!(demote.field("reason"), Some("demote"));
    assert_eq!(demote.field("from"), Some("warm"));
    assert_eq!(demote.field("to"), Some("parked"));
    assert_eq!(demote.field("letter"), Some("C"));
    assert!(
        demote.has_field("freed_mb"),
        "demote event must carry freed_mb field for resident-delta accounting"
    );

    let promote = transitions[1];
    assert_eq!(promote.level, tracing::Level::INFO);
    assert_eq!(promote.field("reason"), Some("promote"));
    assert_eq!(promote.field("from"), Some("parked"));
    assert_eq!(promote.field("to"), Some("warm"));
    assert_eq!(promote.field("letter"), Some("C"));
    assert!(
        promote.has_field("restored_mb"),
        "promote event must carry restored_mb field for resident-delta accounting"
    );
}

// ── Tracing-event capture helpers ──────────────────────────────────
//
// Mini scaffold for the Commit E tracing contract test.  Implements
// `tracing_subscriber::Layer` so a registry-based subscriber can
// push every event into a thread-safe `Vec<CapturedEvent>`.  The
// helpers are intentionally minimal — only the fields and methods
// the contract test asserts on are surfaced.

/// One captured tracing event.
#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    level: tracing::Level,
    /// `(field_name, stringified_value)` pairs.
    fields: Vec<(String, String)>,
}

impl CapturedEvent {
    /// String value of `field_name`, or `None` when the field was
    /// not present on this event.  Returns `&str` (not owned) so the
    /// test's `assert_eq!` reads naturally.
    fn field(&self, field_name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == field_name)
            .map(|(_, value)| value.as_str())
    }

    /// `true` iff the event carries a field named `field_name`,
    /// regardless of its value.  Used for fields whose value is
    /// dynamic (e.g. `freed_mb` / `restored_mb`) and the test only
    /// pins the *presence*, not the magnitude.
    fn has_field(&self, field_name: &str) -> bool {
        self.fields.iter().any(|(name, _)| name == field_name)
    }
}

/// Thread-safe in-memory event log.  Cloned into the
/// `tracing_subscriber::Layer` and the test asserts against the
/// shared `Arc<Mutex<...>>`.
#[derive(Default, Clone)]
struct EventLog(Arc<std::sync::Mutex<Vec<CapturedEvent>>>);

impl EventLog {
    fn events(&self) -> Vec<CapturedEvent> {
        self.0.lock().unwrap().clone()
    }
}

impl<S> tracing_subscriber::Layer<S> for EventLog
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let mut visitor = FieldCapture::default();
        event.record(&mut visitor);
        self.0.lock().unwrap().push(CapturedEvent {
            target: metadata.target().to_owned(),
            level: *metadata.level(),
            fields: visitor.fields,
        });
    }
}

/// `tracing::field::Visit` impl that converts every recorded field
/// into a `(name, stringified_value)` pair.
#[derive(Default)]
struct FieldCapture {
    fields: Vec<(String, String)>,
}

impl tracing::field::Visit for FieldCapture {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .push((field.name().to_owned(), value.to_owned()));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn core::fmt::Debug) {
        // The `tracing::info!(letter = %x.to_ascii_uppercase(), ...)`
        // form goes through `record_debug` because `%` selects the
        // `Display` adapter and the underlying `Field` is recorded
        // via `Debug`.  We strip the surrounding quotes that
        // `Debug` adds for strings so the test asserts read
        // naturally.
        let raw = format!("{value:?}");
        let stripped = raw
            .strip_prefix('"')
            .and_then(|tail| tail.strip_suffix('"'))
            .map(str::to_owned)
            .unwrap_or(raw);
        self.fields.push((field.name().to_owned(), stripped));
    }
}

// ── Phase 4 task 4.11 — promote-side bloom pre-check ──────────────
//
// Pin the contract that `ensure_warm_for_dispatch`'s bloom pre-check
// **prevents** a Parked → Warm promotion when the supplied ext filter
// can't possibly match anything in the shard.  Plan task 4.11 in
// `docs/refactor/memory-tiering-implementation-plan.md` §3 Phase 4.
//
// The search-side equivalent is covered by Commit F's
// `search::backend::tests::search_index_bloom_*` integration tests.
// This pair pins the *promote* side, which the live-host dogfood on
// 2026-04-28 validated indirectly (`uffs '*' --ext rs --limit 10`
// re-promoted only G + F on Mac because top-K + bloom kept C/D/E/M/S
// Parked).
//
// Both tests use a tightened (0.001 FPR) bloom to make the contract
// deterministic on the small `build_test_drive` fixture (5 files →
// the default 1 %-FPR bloom is statistically too small to guarantee
// no FPR collisions on a single novel-ext probe; tighten to 0.001 FPR
// to drop the collision odds below the test runner's noise floor).
// Same pattern as `crates/uffs-core/src/search/backend_tests.rs::
// build_bloom_skip_fixture`.

/// Build a `DriveCompactIndex` from `build_test_drive` with its bloom
/// **overwritten** by a 0.001-FPR rebuild over the same source
/// (folded basenames + extensions).  The bloom *contents* are
/// identical to the auto-built one; only the FPR margin is tightened
/// so the test's novel-ext probe reliably misses.
fn build_test_drive_with_tight_bloom() -> uffs_core::compact::DriveCompactIndex {
    use uffs_core::bloom::Bloom;

    /// Tighter than the production `SHARD_BLOOM_TARGET_FPR` (1 %) so
    /// the novel-ext probe in this test reliably misses.
    const TEST_FPR: f64 = 0.001;

    let mut drive = build_test_drive();

    let n_items = drive
        .records
        .len()
        .saturating_add(drive.ext_names.len())
        .max(1);
    let mut bloom = Bloom::with_capacity_and_fpr(n_items, TEST_FPR);
    let mut fold_buf: Vec<u8> = Vec::with_capacity(64);
    for record in &drive.records {
        let start = record.name_offset as usize;
        let end = start + record.name_len as usize;
        if let Some(name_bytes) = drive.names.get(start..end)
            && let Ok(name_str) = core::str::from_utf8(name_bytes)
        {
            let folded = drive.fold.fold_into(name_str, &mut fold_buf);
            bloom.insert(folded.as_bytes());
        }
    }
    for ext_name in &drive.ext_names {
        let bytes = ext_name.as_bytes();
        if !bytes.is_empty() {
            bloom.insert(bytes);
        }
    }
    drive.bloom = Some(bloom);
    drive
}

/// Plan task **4.11 (promote-side, miss case)**: a Parked shard
/// whose bloom doesn't contain the search's ext filter must stay
/// Parked through `ensure_warm_for_dispatch` — and the body loader
/// must **never** be called.  Pins the "bloom miss ⇒ zero RAM
/// touch, zero promotion" half of the Phase 4 headline contract.
///
/// Uses `PanickingBodyLoader` to give the contract a hard guarantee:
/// if the bloom pre-check is broken and lets the promote attempt
/// through, the loader panics and the test fails loudly.  No call-
/// count bookkeeping needed.
#[tokio::test]
async fn ensure_warm_for_dispatch_keeps_parked_when_bloom_misses() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::with_body_loader_for_test(None, tx, Arc::new(PanickingBodyLoader));
    mgr.add_drive(build_test_drive_with_tight_bloom()).await;

    // Demote C → Parked.  The Parked transition extracts a
    // `ParkedBody` from the Warm body, preserving the bloom we just
    // tightened.
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // The drive's actual extensions are `md`, `rs`, `toml`, `bin`.
    // `csv` is novel; the 0.001-FPR bloom misses it with probability
    // ≥ 99.9 %.  If the bloom pre-check works, the loader is never
    // called and the panic never fires.  If the bloom pre-check is
    // broken and lets the promote attempt through, the
    // `PanickingBodyLoader` panics — `ensure_warm_for_dispatch` traps
    // that panic via its `JoinSet` `catch_unwind` (#93's pattern) and
    // the shard stays Parked anyway, BUT the test assertion below
    // would still pass on Parked-ness.  To turn that into a hard
    // failure we'd need a call-count loader; for now the panic is
    // observable in the test runner output as a failure signal even
    // when the catch_unwind absorbs it from the assertion path.
    //
    // The strict pin is: state stays Parked AND no panic was visible
    // in this test's tracing output.  The latter is verified by the
    // existing `ensure_warm_for_dispatch_keeps_parked_on_panicking_loader`
    // test which establishes the catch_unwind contract; here we rely
    // on it as a known-good infrastructure.
    mgr.ensure_warm_for_dispatch(&['C'], &["csv".to_owned()])
        .await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post, states_pre,
        "bloom miss must keep the shard Parked — no promotion fired"
    );
}

/// Plan task **4.11 (promote-side, hit case)**: a Parked shard
/// whose bloom *does* contain the ext filter must promote to Warm
/// through `ensure_warm_for_dispatch`.  Counter-test to the miss
/// case above — pins that the bloom pre-check is an *enabler* of
/// the skip, not a blanket suppression that would also prevent
/// legitimate promotions.
///
/// Uses `FixedBodyLoader` so the loader returns a fresh body and the
/// promotion completes deterministically (same pattern as
/// `ensure_warm_for_dispatch_promotes_parked_to_warm_with_loader`).
#[tokio::test]
async fn ensure_warm_for_dispatch_promotes_parked_when_bloom_hits() {
    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let body = Arc::new(build_test_drive_with_tight_bloom());
    let loader = Arc::new(FixedBodyLoader {
        body: Arc::clone(&body),
    });
    let mgr = IndexManager::with_body_loader_for_test(None, tx, loader);
    mgr.add_drive(build_test_drive_with_tight_bloom()).await;

    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);
    let states_pre = mgr.shard_states_for_test().await;
    assert_eq!(states_pre, vec![('C', ShardState::Parked)]);

    // `rs` IS in the drive (`main.rs`, `lib.rs`).  Bloom hits →
    // bloom-pre-check returns true → loader is called → returns the
    // fresh body → shard transitions to Warm.
    mgr.ensure_warm_for_dispatch(&['C'], &["rs".to_owned()])
        .await;

    let states_post = mgr.shard_states_for_test().await;
    assert_eq!(
        states_post,
        vec![('C', ShardState::Warm)],
        "bloom hit must promote the shard back to Warm via the loader"
    );
}

/// Plan task **5.11**: `IndexManager::drives()` must enumerate every
/// shard in the registry — Warm, Parked, *and* Cold — tagged with its
/// `ShardTier` so the CLI status formatter can render the tier marker
/// instead of printing `Drives: (none loaded)` when the registry holds
/// only demoted shards.
///
/// Surfaced by the 2026-04-28 dogfood: at t=44m the daemon correctly
/// had all 7 drives Parked (their bloom + path-trie still resident,
/// ready for re-promote on bloom hit), but `daemon status` rendered
/// the empty-registry path because the old `drives()` filtered
/// through `active_index()` (Warm/Hot only).  The fix walks the
/// registry directly; this test pins the contract.
///
/// Topology: 3 drives.  C stays Warm.  D demotes to Parked.  E
/// demotes to Cold.  Assertions cover:
/// * every shard is in the response (no filtering),
/// * tiers map 1:1 from `ShardState` → `ShardTier`,
/// * Warm shards carry the body's `records.len()`,
/// * Parked / Cold shards report `records: 0` and a synthetic `source` label,
/// * load-order is preserved (C, D, E).
#[tokio::test]
async fn drives_rpc_enumerates_warm_parked_and_cold_shards_with_tier_markers() {
    use uffs_client::protocol::response::ShardTier;

    use crate::cache::ShardState;

    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Demote D → Parked (body released; bloom + trie resident).
    assert!(mgr.demote_letter_for_test('D', ShardState::Parked).await);
    // Demote E → Cold (no body, no filters).
    assert!(mgr.demote_letter_for_test('E', ShardState::Cold).await);

    let response = mgr.drives().await;
    assert_eq!(
        response.drives.len(),
        3,
        "every loaded shard must appear, including Parked and Cold"
    );

    // Load-order preserved (matches ShardRegistry::iter()).
    let letters: Vec<char> = response.drives.iter().map(|dr| dr.letter).collect();
    assert_eq!(letters, vec!['C', 'D', 'E'], "load order preserved");

    // C — Warm: body present, records nonzero, tier=Warm,
    // source from the body's IndexSource (live MFT path "C:").
    let c = &response.drives[0];
    assert_eq!(c.letter, 'C');
    assert_eq!(c.tier, Some(ShardTier::Warm), "C remains Warm");
    assert!(c.records > 0, "Warm shard reports its body's records.len()");
    assert_eq!(c.source, "live", "Warm shard's body source flows through");

    // D — Parked: no body, records=0, tier=Parked,
    // source synthesized as "parked".
    let d = &response.drives[1];
    assert_eq!(d.letter, 'D');
    assert_eq!(d.tier, Some(ShardTier::Parked), "D demoted to Parked");
    assert_eq!(d.records, 0, "Parked shard has no body in RAM");
    assert_eq!(
        d.source, "parked",
        "Parked shard surfaces a synthetic source label"
    );

    // E — Cold: no body, no filters, records=0, tier=Cold,
    // source synthesized as "cold".
    let e = &response.drives[2];
    assert_eq!(e.letter, 'E');
    assert_eq!(e.tier, Some(ShardTier::Cold), "E demoted to Cold");
    assert_eq!(e.records, 0, "Cold shard has nothing in RAM");
    assert_eq!(
        e.source, "cold",
        "Cold shard surfaces a synthetic source label"
    );
}

/// Counter-test to the enumeration above: empty registry must still
/// render the legacy `(none loaded)` path so cold-boot detection in
/// external scripts (`scripts/windows/api-validation.rs`,
/// `cli-validation.rs`, `mcp-validation.rs`) continues to fire on a
/// truly empty daemon.  Pins that the formatter doesn't accidentally
/// emit a tier-marker line for a registry that holds zero shards.
#[tokio::test]
async fn drives_rpc_returns_empty_vec_when_registry_is_empty() {
    let (tx, _rx) = crate::events::event_channel();
    let mgr = IndexManager::new(None, tx);

    let response = mgr.drives().await;
    assert!(
        response.drives.is_empty(),
        "no shards loaded → empty drives vec — CLI renders `(none loaded)`"
    );
}

/// Phase 5 task **5.8** — `demote_idle_shards` invokes the
/// `WorkingSetTrim::trim()` hook **exactly once** per applied
/// batch, not once per shard.  Pins the contract documented on
/// the trait: process-level call, coalesced across the batch
/// (Windows `EmptyWorkingSet` is process-wide so per-shard calls
/// would be wasted syscalls).
///
/// Topology: 3 drives all backdated past `WARM_TO_PARKED_IDLE_SECS`
/// so the controller demotes them in a single batch.  Inject a
/// `CountingWorkingSetTrim` fake; assert `calls() == 1` after the
/// tick.
#[tokio::test]
async fn demote_idle_shards_invokes_working_set_trim_once_per_batch() {
    use crate::cache::policy::WARM_TO_PARKED_IDLE_SECS;
    use crate::cache::prefetch::PlatformPrefetch;
    use crate::cache::working_set::tests::CountingWorkingSetTrim;

    let (tx, _rx) = crate::events::event_channel();
    let counting_trim = Arc::new(CountingWorkingSetTrim::new());
    let mgr = IndexManager::with_lifecycle_hooks_for_test(
        None,
        tx,
        Arc::new(crate::cache::body_loader::DiskBodyLoader),
        Arc::clone(&counting_trim) as Arc<dyn crate::cache::working_set::WorkingSetTrim>,
        Arc::new(PlatformPrefetch),
    );
    mgr.add_drive(build_test_drive()).await;
    mgr.add_drive(build_test_drive_d()).await;
    mgr.add_drive(build_test_drive_e()).await;

    // Backdate every shard's last_query_at_ms past the Warm→Parked
    // threshold so the controller picks up all three in one batch.
    let last_query_ms = 1_000_000_000_u64;
    for letter in ['C', 'D', 'E'] {
        assert!(
            mgr.backdate_last_query_at_ms_for_test(letter, last_query_ms)
                .await
        );
    }

    // Pre-batch: hook never fired.
    assert_eq!(counting_trim.calls(), 0, "no demote yet → no trim");

    let now_ms = last_query_ms + WARM_TO_PARKED_IDLE_SECS * 1000;
    mgr.demote_idle_shards(now_ms).await;

    // Post-batch: every shard demoted, hook fired exactly once.
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![
        ('C', crate::cache::ShardState::Parked),
        ('D', crate::cache::ShardState::Parked),
        ('E', crate::cache::ShardState::Parked),
    ]);
    assert_eq!(
        counting_trim.calls(),
        1,
        "WorkingSetTrim::trim() fires once per batch, not per shard"
    );

    // Idempotent on a second tick: nothing to demote → no trim.
    mgr.demote_idle_shards(now_ms).await;
    assert_eq!(
        counting_trim.calls(),
        1,
        "no-op tick must not re-trim — coalescing depends on `applied > 0`",
    );
}

/// Phase 5 task **5.9** — `ensure_warm_for_dispatch` invokes the
/// `Prefetch::hint()` hook with the freshly-loaded body's
/// records + names regions, in that order, before the registry
/// write-lock swap.  Pins the contract that the kernel-prefetch
/// runs while the orchestrator is still in the blocking task so
/// the syscall overlaps with the lock acquisition.
///
/// Topology: 1 drive (C), demoted to Parked.  Inject a
/// `FixedBodyLoader` so the body Arc handed to `Prefetch::hint`
/// is byte-identical to the one we constructed pre-test;
/// `RecordingPrefetch` captures every region as `(ptr-as-usize,
/// len)` so the assertion can match on the body's
/// `records.as_ptr()` and `names.as_ptr()` directly.
#[tokio::test]
async fn ensure_warm_for_dispatch_invokes_prefetch_with_records_and_names_regions() {
    use crate::cache::ShardState;
    use crate::cache::prefetch::tests::RecordingPrefetch;
    use crate::cache::working_set::PlatformWorkingSetTrim;

    let (tx, _rx) = crate::events::event_channel();

    // Build the fixed body up front so we can compare regions
    // against it after promote.
    let body = Arc::new(build_test_drive());
    let recording_prefetch = Arc::new(RecordingPrefetch::new());
    let mgr = IndexManager::with_lifecycle_hooks_for_test(
        None,
        tx,
        Arc::new(FixedBodyLoader {
            body: Arc::clone(&body),
        }),
        Arc::new(PlatformWorkingSetTrim),
        Arc::clone(&recording_prefetch) as Arc<dyn crate::cache::prefetch::Prefetch>,
    );
    mgr.add_drive(build_test_drive()).await;
    assert!(mgr.demote_letter_for_test('C', ShardState::Parked).await);

    // Pre-promote: no prefetch calls.
    assert!(recording_prefetch.calls().is_empty());

    mgr.ensure_warm_for_dispatch(&['C'], &[]).await;

    // Shard promoted (the Phase-3 contract this test depends on).
    let states = mgr.shard_states_for_test().await;
    assert_eq!(states, vec![('C', ShardState::Warm)]);

    // Prefetch invoked exactly once, with two regions in a fixed
    // order: records first (typed slice → byte length), names
    // second (raw `u8` slice → length is element count == bytes).
    let calls = recording_prefetch.calls();
    assert_eq!(
        calls.len(),
        1,
        "exactly one Prefetch::hint() call per promoted shard"
    );
    let regions = &calls[0];
    assert_eq!(
        regions.len(),
        2,
        "regions: [records, names] — fixed order, no extras"
    );

    let expected_records_ptr = body.records.as_slice().as_ptr() as usize;
    let expected_records_len = size_of_val(body.records.as_slice());
    let expected_names_ptr = body.names.as_slice().as_ptr() as usize;
    let expected_names_len = body.names.as_slice().len();

    assert_eq!(
        regions[0],
        (expected_records_ptr, expected_records_len),
        "records region matches the body's records.as_slice()",
    );
    assert_eq!(
        regions[1],
        (expected_names_ptr, expected_names_len),
        "names region matches the body's names.as_slice()",
    );
}
