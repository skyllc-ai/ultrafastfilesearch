//! Tests for direct `MftIndex` search.

use regex::Regex;
use uffs_mft::index::{
    IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex, NO_ENTRY, ROOT_FRS, SizeInfo,
};

use super::*;

type TestError = Box<dyn core::error::Error>;
type TestResult = Result<(), TestError>;

fn push_name_ref(index: &mut MftIndex, name: &str) -> Result<IndexNameRef, TestError> {
    let offset = index.add_name(name);
    Ok(IndexNameRef::new(
        offset,
        u16::try_from(name.len())?,
        name.is_ascii(),
        IndexNameRef::NO_EXTENSION,
    ))
}

fn push_file_name_ref(index: &mut MftIndex, name: &str) -> Result<IndexNameRef, TestError> {
    let offset = index.add_name(name);
    let extension_id = index.intern_extension(name);
    Ok(IndexNameRef::new(
        offset,
        u16::try_from(name.len())?,
        name.is_ascii(),
        extension_id,
    ))
}

fn build_index_query_fixture() -> Result<MftIndex, TestError> {
    let mut index = MftIndex::new('C');

    let root_name = push_name_ref(&mut index, ".")?;
    let root = index.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = root_name;
    root.first_name.parent_frs = ROOT_FRS;

    let docs_frs = 100_u64;
    let docs_name = push_name_ref(&mut index, "Docs")?;
    let docs = index.get_or_create(docs_frs);
    docs.stdinfo.set_directory(true);
    docs.first_name.name = docs_name;
    docs.first_name.parent_frs = ROOT_FRS;

    let links_frs = 101_u64;
    let links_name = push_name_ref(&mut index, "Links")?;
    let links = index.get_or_create(links_frs);
    links.stdinfo.set_directory(true);
    links.first_name.name = links_name;
    links.first_name.parent_frs = ROOT_FRS;

    let primary_name = push_file_name_ref(&mut index, "alpha.txt")?;
    let hard_link_name = push_file_name_ref(&mut index, "alpha_link.txt")?;
    let ads_name = push_name_ref(&mut index, "meta")?;

    let hard_link_idx = u32::try_from(index.links.len())?;
    index.links.push(LinkInfo {
        next_entry: NO_ENTRY,
        name: hard_link_name,
        parent_frs: links_frs,
    });

    let ads_idx = u32::try_from(index.streams.len())?;
    index.streams.push(IndexStreamInfo {
        size: SizeInfo {
            length: 5,
            allocated: 5,
        },
        next_entry: NO_ENTRY,
        name: ads_name,
        flags: 8_u8 << 2_u32,
    });

    let alpha = index.get_or_create(200);
    alpha.first_name.name = primary_name;
    alpha.first_name.parent_frs = docs_frs;
    alpha.first_name.next_entry = hard_link_idx;
    alpha.name_count = 2;
    alpha.first_stream.size = SizeInfo {
        length: 120,
        allocated: 128,
    };
    alpha.first_stream.next_entry = ads_idx;
    alpha.first_stream.flags = 8_u8 << 2_u32;
    alpha.stream_count = 2;
    alpha.total_stream_count = 2;

    let beta_name = push_file_name_ref(&mut index, "beta.bin")?;
    let beta = index.get_or_create(201);
    beta.first_name.name = beta_name;
    beta.first_name.parent_frs = docs_frs;
    beta.first_stream.size = SizeInfo {
        length: 20,
        allocated: 64,
    };
    beta.first_stream.flags = 8_u8 << 2_u32;

    Ok(index)
}

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
fn test_index_query_count_applies_record_filters() -> TestResult {
    let index = build_index_query_fixture()?;

    assert_eq!(
        IndexQuery::new(&index).files_only().min_size(100).count(),
        1
    );
    assert_eq!(IndexQuery::new(&index).files_only().max_size(50).count(), 1);

    Ok(())
}

#[test]
fn test_index_query_collect_respects_name_and_stream_expansion_toggles() -> TestResult {
    let index = build_index_query_fixture()?;

    let expanded = IndexQuery::new(&index)
        .glob("alpha*")
        .files_only()
        .collect();
    assert_eq!(expanded.len(), 4);
    assert_eq!(
        expanded
            .iter()
            .filter(|result| result.name_index == 0)
            .count(),
        2
    );
    assert_eq!(
        expanded
            .iter()
            .filter(|result| result.name_index == 1)
            .count(),
        2
    );
    assert_eq!(
        expanded
            .iter()
            .filter(|result| result.stream_index == 0)
            .count(),
        2
    );
    assert_eq!(
        expanded
            .iter()
            .filter(|result| result.stream_index == 1)
            .count(),
        2
    );

    let no_name_expansion = IndexQuery::new(&index)
        .glob("alpha*")
        .files_only()
        .with_expand_names(false)
        .collect();
    assert_eq!(no_name_expansion.len(), 2);
    assert!(
        no_name_expansion
            .iter()
            .all(|result| result.name_index == 0)
    );

    let no_stream_expansion = IndexQuery::new(&index)
        .glob("alpha*")
        .files_only()
        .with_expand_streams(false)
        .collect();
    assert_eq!(no_stream_expansion.len(), 2);
    assert!(
        no_stream_expansion
            .iter()
            .all(|result| result.stream_index == 0)
    );

    let no_expansion = IndexQuery::new(&index)
        .glob("alpha*")
        .files_only()
        .with_expand_names(false)
        .with_expand_streams(false)
        .collect();
    assert_eq!(no_expansion.len(), 1);
    assert_eq!(
        no_expansion.first().map(|result| result.name.as_str()),
        Some("alpha.txt")
    );

    Ok(())
}

#[test]
fn test_index_query_collect_resolves_paths_for_hard_links_and_ads() -> TestResult {
    let index = build_index_query_fixture()?;

    let results = IndexQuery::new(&index)
        .glob("alpha*")
        .files_only()
        .resolve_paths()
        .collect();

    let paths: Vec<String> = results
        .iter()
        .filter_map(|result| result.path.clone())
        .collect();
    assert_eq!(paths.len(), 4);
    assert!(paths.contains(&r"C:\Docs\alpha.txt".to_owned()));
    assert!(paths.contains(&r"C:\Docs\alpha.txt:meta".to_owned()));
    assert!(paths.contains(&r"C:\Links\alpha_link.txt".to_owned()));
    assert!(paths.contains(&r"C:\Links\alpha_link.txt:meta".to_owned()));

    Ok(())
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
