// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for filter application, parsing, and derived filters (part 2).
//!
//! Exception: continuation of filters test suite; cohesion with tests.rs
//! requires shared helpers and tightly coupled assertions.

use uffs_text::case_fold::CaseFold;

use super::*;

#[test]
fn parse_attr_require_system_files_preset() {
    let bits = parse_attr_require("system-files");
    // system-files → hidden (0x2) + system (0x4) = 6
    assert_eq!(bits, 0x2 | 0x4);
}

#[test]
fn parse_attr_exclude_user_files_preset() {
    let bits = parse_attr_exclude("user-files");
    // user-files → !hidden + !system
    assert_eq!(bits, 0x2 | 0x4);
}

#[test]
fn parse_attr_require_compressed_encrypted_preset() {
    let bits = parse_attr_require("compressed-encrypted");
    // compressed (0x800) + encrypted (0x4000)
    assert_eq!(bits, 0x0800 | 0x4000);
}

// ═══════════════════════════════════════════════════════════════════
// hide_ads filter
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_hide_ads_rejects_colon_in_name() {
    let mut names = Vec::new();
    let rec = test_record("file.txt:Zone.Identifier", &mut names);
    let filters = SearchFilters {
        hide_ads: true,
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "ADS names containing ':' should be rejected"
    );
}

#[test]
fn filter_hide_ads_accepts_normal_names() {
    let mut names = Vec::new();
    let rec = test_record("readme.txt", &mut names);
    let filters = SearchFilters {
        hide_ads: true,
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "Normal names without ':' should pass hide_ads"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Name-length filters
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_min_name_len_rejects_short_names() {
    let mut names = Vec::new();
    let rec = test_record("a.txt", &mut names); // 5 chars
    let filters = SearchFilters {
        min_name_len: Some(10),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name 'a.txt' (5 chars) should be rejected by min_name_len=10"
    );
}

#[test]
fn filter_min_name_len_accepts_long_names() {
    let mut names = Vec::new();
    let rec = test_record("long_filename.txt", &mut names); // 17 chars
    let filters = SearchFilters {
        min_name_len: Some(10),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name 'long_filename.txt' (17 chars) should pass min_name_len=10"
    );
}

#[test]
fn filter_max_name_len_rejects_long_names() {
    let mut names = Vec::new();
    let rec = test_record("very_long_filename.txt", &mut names); // 22 chars
    let filters = SearchFilters {
        max_name_len: Some(10),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name 'very_long_filename.txt' (22 chars) should be rejected by max_name_len=10"
    );
}

#[test]
fn filter_max_name_len_accepts_short_names() {
    let mut names = Vec::new();
    let rec = test_record("hi.rs", &mut names); // 5 chars
    let filters = SearchFilters {
        max_name_len: Some(10),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name 'hi.rs' (5 chars) should pass max_name_len=10"
    );
}

#[test]
fn filter_name_len_range() {
    let mut names = Vec::new();
    let rec = test_record("medium.txt", &mut names); // 10 chars
    let filters = SearchFilters {
        min_name_len: Some(5),
        max_name_len: Some(15),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name 'medium.txt' (10 chars) should pass 5..15 range"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Size-on-disk (allocated) filters
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_min_allocated_rejects_small_allocation() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names); // allocated = 1024
    let filters = SearchFilters {
        min_allocated: Some(4096),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "allocated=1024 should be rejected by min_allocated=4096"
    );
}

#[test]
fn filter_min_allocated_accepts_large_allocation() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names); // allocated = 1024
    let filters = SearchFilters {
        min_allocated: Some(512),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "allocated=1024 should pass min_allocated=512"
    );
}

#[test]
fn filter_max_allocated_rejects_large_allocation() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names); // allocated = 1024
    let filters = SearchFilters {
        max_allocated: Some(512),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "allocated=1024 should be rejected by max_allocated=512"
    );
}

#[test]
fn filter_max_allocated_accepts_small_allocation() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names); // allocated = 1024
    let filters = SearchFilters {
        max_allocated: Some(2048),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "allocated=1024 should pass max_allocated=2048"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Tree-size filters
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_min_treesize_rejects_small_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // treesize = 5000
    let filters = SearchFilters {
        min_treesize: Some(10_000),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "treesize=5000 should be rejected by min_treesize=10000"
    );
}

#[test]
fn filter_min_treesize_accepts_large_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // treesize = 5000
    let filters = SearchFilters {
        min_treesize: Some(1000),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "treesize=5000 should pass min_treesize=1000"
    );
}

#[test]
fn filter_max_treesize_rejects_large_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // treesize = 5000
    let filters = SearchFilters {
        max_treesize: Some(1000),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "treesize=5000 should be rejected by max_treesize=1000"
    );
}

#[test]
fn filter_max_treesize_accepts_small_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // treesize = 5000
    let filters = SearchFilters {
        max_treesize: Some(10_000),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "treesize=5000 should pass max_treesize=10000"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Tree-allocated filters
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_min_tree_allocated_rejects_small_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // tree_allocated = 5120
    let filters = SearchFilters {
        min_tree_allocated: Some(10_000),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "tree_allocated=5000 should be rejected by min_tree_allocated=10000"
    );
}

#[test]
fn filter_max_tree_allocated_accepts_small_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // tree_allocated = 5120
    let filters = SearchFilters {
        max_tree_allocated: Some(10_000),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "tree_allocated=5000 should pass max_tree_allocated=10000"
    );
}

#[test]
fn filter_max_tree_allocated_rejects_large_tree() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // tree_allocated = 5120
    let filters = SearchFilters {
        max_tree_allocated: Some(1000),
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "tree_allocated=5000 should be rejected by max_tree_allocated=1000"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Month-of-year filter
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_allowed_months_accepts_matching_month() {
    let mut names = Vec::new();
    // test_record sets modified = 200_000_000 µs = 200 seconds = 1970-01-01 → month
    // 1
    let rec = test_record("file.txt", &mut names);
    let filters = SearchFilters {
        allowed_months: vec![1],
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "month=1 (January) should match modified=200_000_000µs"
    );
}

#[test]
fn filter_allowed_months_rejects_non_matching_month() {
    let mut names = Vec::new();
    // test_record modified = 200_000_000 µs → month 1 (January)
    let rec = test_record("file.txt", &mut names);
    let filters = SearchFilters {
        allowed_months: vec![6, 7, 8], // summer months
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "months [6,7,8] should reject file modified in month 1"
    );
}

#[test]
fn filter_empty_months_means_no_filter() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names);
    let filters = SearchFilters {
        allowed_months: vec![],
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "empty allowed_months should pass all records"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Combined new + old filters
// ═══════════════════════════════════════════════════════════════════

#[test]
fn filter_combined_allocated_plus_size() {
    let mut names = Vec::new();
    let rec = test_record("data.bin", &mut names); // size=1000, allocated=1024
    let filters = SearchFilters {
        min_size: Some(500),
        max_allocated: Some(2048),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "size=1000 >= 500 AND allocated=1024 <= 2048 should pass"
    );
}

#[test]
fn filter_combined_name_len_plus_size() {
    let mut names = Vec::new();
    let rec = test_record("a.txt", &mut names); // name len=5, size=1000
    let filters = SearchFilters {
        min_size: Some(500),
        min_name_len: Some(10), // 5 < 10 → reject
        ..Default::default()
    };
    assert!(
        !filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "name_len=5 < 10 should reject even though size passes"
    );
}

#[test]
fn filter_combined_treesize_plus_descendants() {
    let mut names = Vec::new();
    let rec = test_record("project", &mut names); // treesize=5000, descendants=5
    let filters = SearchFilters {
        min_descendants: Some(5),
        min_treesize: Some(1000),
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "treesize=5000 >= 1000 AND descendants=10 >= 5 should pass"
    );
}

#[test]
fn filter_combined_month_plus_attr() {
    let mut names = Vec::new();
    let rec = test_record("file.txt", &mut names); // flags=0x20 (archive), month=1
    let filters = SearchFilters {
        attr_require: 0x20,      // archive bit
        allowed_months: vec![1], // January
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "archive flag set + month=11 should pass"
    );
}

#[test]
fn filter_combined_all_new_fields() {
    let mut names = Vec::new();
    let rec = test_record("medium.txt", &mut names); // 10 chars, sz=1000, alloc=1024, ts=5000, ta=5000, month=11
    let filters = SearchFilters {
        min_name_len: Some(5),
        max_name_len: Some(20),
        min_allocated: Some(512),
        max_allocated: Some(4096),
        min_treesize: Some(1000),
        max_treesize: Some(10_000),
        min_tree_allocated: Some(1000),
        max_tree_allocated: Some(10_000),
        allowed_months: vec![1], // January (modified=200_000_000µs)
        ..Default::default()
    };
    assert!(
        filters.matches_record(&rec, &names, &mut Vec::new(), CaseFold::default_table()),
        "all new filter fields should pass with matching test record"
    );
}

// ── needs_display_row_filter ─────────────────────────────────────

#[test]
fn needs_display_row_filter_false_when_empty() {
    let filters = SearchFilters::default();
    assert!(
        !filters.needs_display_row_filter(),
        "default filters need no display-row pass"
    );
}

#[test]
fn needs_display_row_filter_true_for_type_filter() {
    let filters = SearchFilters {
        type_filter: Some("code".to_owned()),
        ..Default::default()
    };
    assert!(
        filters.needs_display_row_filter(),
        "type_filter requires display-row pass"
    );
}

#[test]
fn needs_display_row_filter_true_for_path_contains() {
    let filters = SearchFilters {
        path_contains_lower: Some("windows".to_owned()),
        ..Default::default()
    };
    assert!(
        filters.needs_display_row_filter(),
        "path_contains requires display-row pass"
    );
}

#[test]
fn bulkiness_does_not_require_display_row_filter() {
    // Bulkiness is computed from size/allocated fields available on
    // CompactRecord, so it is checked at scan level in matches_record,
    // not as a display-row post-filter.
    let filters = SearchFilters {
        min_bulkiness: Some(200),
        ..Default::default()
    };
    assert!(
        !filters.needs_display_row_filter(),
        "bulkiness should NOT require display-row pass"
    );
}

#[test]
fn path_len_does_not_require_display_row_filter() {
    // path_len is precomputed on CompactRecord, so it is checked at
    // scan level in matches_record, not as a display-row post-filter.
    let filters = SearchFilters {
        min_path_len: Some(100),
        ..Default::default()
    };
    assert!(
        !filters.needs_display_row_filter(),
        "path_len should NOT require display-row pass"
    );
}

// ── apply_search_filters regression tests (T88h–T118) ───────────

/// Regression T89/T91/T93: --type filter must reject non-matching files.
#[test]
fn apply_type_filter_rejects_wrong_extension() {
    let mut rows = vec![
        // .rs is "code"; .jpg is NOT code
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\src\\main.rs".to_owned(),
            100,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\pics\\photo.jpg".to_owned(),
            5000,
            false,
            0,
            0,
            0,
            0x20,
            8192,
            0,
            0,
            0,
        ),
    ];
    let filters = SearchFilters {
        type_filter: Some("code".to_owned()),
        ..Default::default()
    };
    apply_search_filters(&mut rows, &filters);
    assert_eq!(rows.len(), 1, "only .rs (code) should remain");
    assert_eq!(rows.first().expect("rows non-empty").name(), "main.rs");
}

/// WI-4.4 regression: the `DisplayRow` filter path (Path/PathOnly tree-walk and
/// the regex/trigram post-filter pass) must honor `--malformed` /
/// `--well-formed`. Before the fix `is_empty()` ignored `malformed`, so
/// `apply_search_filters` early-returned, and `row_passes_filters` had no
/// malformed arm — the toggle silently no-opped.
#[test]
fn apply_malformed_filter_keeps_only_ill_formed_rows() {
    let bad = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\dir\\\u{fffd}.txt".to_owned(),
        100,
        false,
        0,
        0,
        0,
        0x20,
        4096,
        0,
        0,
        0,
    )
    .with_forensics(true, true, None);
    let good = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\dir\\readme.txt".to_owned(),
        100,
        false,
        0,
        0,
        0,
        0x20,
        4096,
        0,
        0,
        0,
    )
    .with_forensics(false, false, None);

    let mut keep_malformed = vec![bad.clone(), good.clone()];
    apply_search_filters(&mut keep_malformed, &SearchFilters {
        malformed: Some(true),
        ..Default::default()
    });
    assert_eq!(
        keep_malformed.len(),
        1,
        "only the ill-formed row should remain"
    );
    assert!(
        keep_malformed.first().expect("row kept").malformed,
        "kept row must be the malformed one"
    );

    let mut keep_well_formed = vec![bad, good];
    apply_search_filters(&mut keep_well_formed, &SearchFilters {
        malformed: Some(false),
        ..Default::default()
    });
    assert_eq!(
        keep_well_formed.len(),
        1,
        "only the well-formed row should remain"
    );
    assert!(
        !keep_well_formed.first().expect("row kept").malformed,
        "kept row must be the well-formed one"
    );
}

/// Regression T88h/T95: --in-path filter must match resolved path substring.
#[test]
fn apply_path_contains_filters_by_substring() {
    let mut rows = vec![
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\Windows\\System32\\cmd.exe".to_owned(),
            100,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\Users\\hello.exe".to_owned(),
            200,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
    ];
    let filters = SearchFilters {
        path_contains_lower: Some("windows".to_owned()),
        ..Default::default()
    };
    apply_search_filters(&mut rows, &filters);
    assert_eq!(
        rows.len(),
        1,
        "only path containing 'windows' should remain"
    );
    assert!(
        rows.first()
            .expect("rows non-empty")
            .path
            .contains("Windows")
    );
}

/// Regression T98: --min-bulkiness filter must reject rows with low bulkiness.
///
/// Internal bulkiness uses per-million scale: `1_000_000` = 100% (perfectly
/// packed).  A `min_bulkiness` of `2_000_000` means "at least 200%".
#[test]
fn apply_min_bulkiness_rejects_low_ratio() {
    let mut rows = vec![
        // allocated=4096, size=4096 → bulkiness=1_000_000 (100%)
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\tight.bin".to_owned(),
            4096,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
        // allocated=20480, size=4096 → bulkiness=5_000_000 (500%)
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\bloated.bin".to_owned(),
            4096,
            false,
            0,
            0,
            0,
            0x20,
            20480,
            0,
            0,
            0,
        ),
    ];
    let filters = SearchFilters {
        min_bulkiness: Some(2_000_000), // ≥200% on per-million scale
        ..Default::default()
    };
    apply_search_filters(&mut rows, &filters);
    assert_eq!(rows.len(), 1, "only bloated (500%) should pass >=200%");
    assert_eq!(rows.first().expect("rows non-empty").name(), "bloated.bin");
}

/// Regression T106: --min-path-length must reject short paths.
#[test]
fn apply_min_path_len_rejects_short_paths() {
    let short = "C:\\a.txt"; // 8 chars
    let mut long = String::from("C:\\");
    long.push_str(&"x".repeat(200));
    long.push_str(".txt"); // 208 chars
    let mut rows = vec![
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            short.to_owned(),
            100,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            long,
            200,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        ),
    ];
    let filters = SearchFilters {
        min_path_len: Some(200),
        ..Default::default()
    };
    apply_search_filters(&mut rows, &filters);
    assert_eq!(rows.len(), 1, "only path >=200 chars should remain");
    assert!(rows.first().expect("rows non-empty").path.len() >= 200);
}
