//! Tests for direct `MftIndex` search.

use regex::Regex;

use super::*;

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_any() {
    let pattern = compile_index_pattern("*").unwrap();
    assert!(pattern.matches("anything", true));
    assert!(pattern.matches("", true));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_exact() {
    let pattern = compile_index_pattern("foo.txt").unwrap();
    assert!(pattern.matches("foo.txt", true));
    assert!(!pattern.matches("FOO.TXT", true));
    assert!(pattern.matches("FOO.TXT", false));
    assert!(!pattern.matches("foo.txt.bak", true));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_prefix() {
    let pattern = compile_index_pattern("foo*").unwrap();
    assert!(pattern.matches("foo", true));
    assert!(pattern.matches("foobar", true));
    assert!(!pattern.matches("barfoo", true));
    assert!(pattern.matches("FOOBAR", false));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_suffix() {
    let pattern = compile_index_pattern("*.txt").unwrap();
    assert!(pattern.matches("foo.txt", true));
    assert!(pattern.matches(".txt", true));
    assert!(!pattern.matches("foo.txt.bak", true));
    assert!(pattern.matches("FOO.TXT", false));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_contains() {
    let pattern = compile_index_pattern("*needle*").unwrap();
    assert!(pattern.matches("needle", true));
    assert!(pattern.matches("haystackneedlehaystack", true));
    assert!(!pattern.matches("haystack", true));
    assert!(pattern.matches("NEEDLE", false));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_pattern_prefix_suffix() {
    let pattern = compile_index_pattern("foo*bar").unwrap();
    assert!(pattern.matches("foobar", true));
    assert!(pattern.matches("foo123bar", true));
    assert!(!pattern.matches("foobarbaz", true));
    assert!(!pattern.matches("bazfoobar", true));
}

#[test]
fn test_extensions() {
    let pattern = compile_extensions(&["rs", "toml"]);
    assert!(pattern.matches("main.rs", true));
    assert!(pattern.matches("Cargo.toml", true));
    assert!(!pattern.matches("main.py", true));
    assert!(pattern.matches("MAIN.RS", false));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_extension_index_integration() {
    use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

    let mut index = MftIndex::new('C');
    let root_name_offset = index.add_name(".");
    let root = index.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_name_offset, 1, true, 0);
    root.first_name.parent_frs = ROOT_FRS;

    let files = [
        ("readme.txt", 1000),
        ("notes.txt", 2000),
        ("data.csv", 3000),
        ("script.py", 4000),
        ("config.json", 5000),
        ("test.txt", 6000),
    ];

    for (i, (name, size)) in files.iter().enumerate() {
        let frs = (i + 100) as u64;
        let offset = index.add_name(name);
        let ext_id = index.intern_extension(name);

        let rec = index.get_or_create(frs);
        rec.first_name.name =
            IndexNameRef::new(offset, u16::try_from(name.len()).unwrap(), true, ext_id);
        rec.first_name.parent_frs = ROOT_FRS;
        rec.first_stream.size = SizeInfo {
            length: *size,
            allocated: *size,
        };
        index.extensions.record_file(ext_id, *size);
    }

    index.build_extension_index();

    let pattern = compile_index_pattern("*.txt").unwrap();
    let results: Vec<_> = IndexQuery::new(&index).with_pattern(pattern).collect();

    assert_eq!(results.len(), 3, "Should find 3 .txt files");

    let names: Vec<String> = results.iter().map(|rec| rec.name.clone()).collect();
    assert!(names.contains(&"readme.txt".to_owned()));
    assert!(names.contains(&"notes.txt".to_owned()));
    assert!(names.contains(&"test.txt".to_owned()));

    let total_size: u64 = results.iter().map(|rec| rec.size).sum();
    assert_eq!(total_size, 1000 + 2000 + 6000);
}

#[test]
fn test_query_mode_from_str() {
    assert_eq!(QueryMode::from_str_opt("auto"), Some(QueryMode::Auto));
    assert_eq!(QueryMode::from_str_opt("hybrid"), Some(QueryMode::Auto));
    assert_eq!(
        QueryMode::from_str_opt("index"),
        Some(QueryMode::ForceIndex)
    );
    assert_eq!(QueryMode::from_str_opt("fast"), Some(QueryMode::ForceIndex));
    assert_eq!(
        QueryMode::from_str_opt("dataframe"),
        Some(QueryMode::ForceDataFrame)
    );
    assert_eq!(
        QueryMode::from_str_opt("df"),
        Some(QueryMode::ForceDataFrame)
    );
    assert_eq!(
        QueryMode::from_str_opt("polars"),
        Some(QueryMode::ForceDataFrame)
    );
    assert_eq!(QueryMode::from_str_opt("invalid"), None);
}

#[test]
fn test_query_mode_display() {
    assert_eq!(QueryMode::Auto.to_string(), "auto");
    assert_eq!(QueryMode::ForceIndex.to_string(), "index");
    assert_eq!(QueryMode::ForceDataFrame.to_string(), "dataframe");
}

#[test]
fn test_query_features_requires_dataframe() {
    let empty = QueryFeatures::empty();
    assert!(!empty.requires_dataframe());

    let with_sql = QueryFeatures::empty().with(QueryFeatures::SQL);
    assert!(with_sql.requires_dataframe());
    assert!(with_sql.has(QueryFeatures::SQL));
    assert!(!with_sql.has(QueryFeatures::AGGREGATION));

    let with_agg = QueryFeatures::empty().with(QueryFeatures::AGGREGATION);
    assert!(with_agg.requires_dataframe());

    let combined = QueryFeatures::empty()
        .with(QueryFeatures::SQL)
        .with(QueryFeatures::SORTING);
    assert!(combined.requires_dataframe());
    assert!(combined.has(QueryFeatures::SQL));
    assert!(combined.has(QueryFeatures::SORTING));
    assert!(!combined.has(QueryFeatures::JOIN));
}

#[test]
#[expect(clippy::unwrap_used, reason = "test code — unwrap on controlled data")]
fn test_analyze_pattern_complexity() {
    let any = compile_index_pattern("*").unwrap();
    assert_eq!(analyze_pattern_complexity(&any), QueryComplexity::Simple);

    let suffix = compile_index_pattern("*.rs").unwrap();
    assert_eq!(analyze_pattern_complexity(&suffix), QueryComplexity::Simple);

    let regex = IndexPattern::Regex {
        regex: Regex::new(".*").unwrap(),
        regex_lower: Regex::new("(?i).*").unwrap(),
    };
    assert_eq!(analyze_pattern_complexity(&regex), QueryComplexity::Simple);
}
