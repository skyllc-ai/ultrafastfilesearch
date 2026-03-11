use uffs_polars::Column;

use super::*;

// Use a test-specific Result type that works with CoreError
type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

#[test]
fn test_parse_single_extension() {
    let filter = ExtensionFilter::parse("jpg").unwrap();
    assert!(filter.matches("photo.jpg"));
    assert!(filter.matches("PHOTO.JPG"));
    assert!(!filter.matches("photo.png"));
}

#[test]
fn test_parse_multiple_extensions() {
    let filter = ExtensionFilter::parse("jpg,png,gif").unwrap();
    assert!(filter.matches("photo.jpg"));
    assert!(filter.matches("image.png"));
    assert!(filter.matches("anim.gif"));
    assert!(!filter.matches("doc.pdf"));
}

#[test]
fn test_parse_with_dots() {
    let filter = ExtensionFilter::parse(".jpg,.png").unwrap();
    assert!(filter.matches("photo.jpg"));
    assert!(filter.matches("image.png"));
}

#[test]
fn test_pictures_collection() {
    let filter = ExtensionFilter::parse("pictures").unwrap();
    assert!(filter.matches("photo.jpg"));
    assert!(filter.matches("image.png"));
    assert!(filter.matches("icon.ico"));
    assert!(!filter.matches("video.mp4"));
}

#[test]
fn test_documents_collection() {
    let filter = ExtensionFilter::parse("documents").unwrap();
    assert!(filter.matches("report.pdf"));
    assert!(filter.matches("letter.docx"));
    assert!(filter.matches("data.xlsx"));
    assert!(!filter.matches("photo.jpg"));
}

#[test]
fn test_mixed_collection_and_extensions() {
    let filter = ExtensionFilter::parse("pictures,mp4,pdf").unwrap();
    assert!(filter.matches("photo.jpg"));
    assert!(filter.matches("video.mp4"));
    assert!(filter.matches("doc.pdf"));
    assert!(!filter.matches("song.mp3"));
}

#[test]
fn test_empty_error() {
    assert!(ExtensionFilter::parse("").is_err());
    assert!(ExtensionFilter::parse("   ").is_err());
}

#[test]
fn test_to_regex() {
    let filter = ExtensionFilter::parse("jpg,png").unwrap();
    let regex = filter.to_regex();
    assert!(regex.contains("jpg"));
    assert!(regex.contains("png"));
    assert!(regex.starts_with(r"\."));
    assert!(regex.ends_with(")$"));
}

#[test]
fn test_no_extension_file() {
    let filter = ExtensionFilter::parse("txt").unwrap();
    assert!(!filter.matches("README"));
    assert!(!filter.matches("Makefile"));
}

// ═══════════════════════════════════════════════════════════════════════
// ExtensionIndex tests
// ═══════════════════════════════════════════════════════════════════════

fn create_ext_test_df() -> DataFrame {
    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2, 3, 4, 5, 6]),
        Column::new(
            "name".into(),
            &[
                "photo.jpg",
                "document.txt",
                "image.jpg",
                "README",
                "script.py",
                "data.txt",
            ],
        ),
    ])
    .unwrap()
}

#[test]
fn test_extension_index_build() -> TestResult {
    let df = create_ext_test_df();
    let index = ExtensionIndex::build(&df)?;

    assert_eq!(index.total_files(), 6);
    assert_eq!(index.unique_extensions(), 3); // jpg, txt, py
    Ok(())
}

#[test]
fn test_extension_index_get() -> TestResult {
    let df = create_ext_test_df();
    let index = ExtensionIndex::build(&df)?;

    let jpg_files = index.get("jpg").unwrap();
    assert_eq!(jpg_files.len(), 2);
    assert!(jpg_files.contains(&1));
    assert!(jpg_files.contains(&3));

    let txt_files = index.get("txt").unwrap();
    assert_eq!(txt_files.len(), 2);
    assert!(txt_files.contains(&2));
    assert!(txt_files.contains(&6));

    assert!(index.get("pdf").is_none());
    Ok(())
}

#[test]
fn test_extension_index_case_insensitive() -> TestResult {
    let df = create_ext_test_df();
    let index = ExtensionIndex::build(&df)?;

    // Should work with any case
    assert!(index.get("JPG").is_some());
    assert!(index.get("Jpg").is_some());
    assert!(index.get("jpg").is_some());
    Ok(())
}

#[test]
fn test_extension_index_stats() -> TestResult {
    let df = create_ext_test_df();
    let index = ExtensionIndex::build(&df)?;

    let stats = index.stats();
    assert_eq!(stats.total_files, 6);
    assert_eq!(stats.files_with_extension, 5); // README has no extension
    assert_eq!(stats.unique_extensions, 3);
    assert_eq!(stats.max_extension_count, 2); // jpg and txt both have 2
    Ok(())
}

#[test]
fn test_extension_index_hidden_files() -> TestResult {
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2, 3]),
        Column::new("name".into(), &[".gitignore", ".bashrc", "file.txt"]),
    ])?;

    let index = ExtensionIndex::build(&df)?;

    // Hidden files should not be indexed as extensions
    assert!(index.get("gitignore").is_none());
    assert!(index.get("bashrc").is_none());
    assert!(index.get("txt").is_some());
    Ok(())
}

// =========================================================================
// Extension Column Tests
// =========================================================================

#[test]
fn test_add_ext_column() -> TestResult {
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2, 3, 4, 5]),
        Column::new(
            "name".into(),
            &[
                "photo.jpg",
                "document.txt",
                "README",
                ".gitignore",
                "archive.tar.gz",
            ],
        ),
    ])?;

    let result = add_ext_column(df)?;

    // Check that ext column was added
    assert!(has_ext_column(&result));

    // Verify the ext column values
    let ext_col = result.column("ext")?.str()?;
    assert_eq!(ext_col.get(0), Some("jpg"));
    assert_eq!(ext_col.get(1), Some("txt"));
    assert!(ext_col.get(2).is_none()); // README has no extension
    assert!(ext_col.get(3).is_none()); // .gitignore is hidden file
    assert_eq!(ext_col.get(4), Some("gz")); // tar.gz -> gz

    Ok(())
}

#[test]
fn test_has_ext_column() -> TestResult {
    let df_without = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64]),
        Column::new("name".into(), &["file.txt"]),
    ])?;

    assert!(!has_ext_column(&df_without));

    let df_with = add_ext_column(df_without)?;
    assert!(has_ext_column(&df_with));

    Ok(())
}

#[test]
fn test_ext_expr_lowercase() -> TestResult {
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2]),
        Column::new("name".into(), &["Photo.JPG", "Document.TXT"]),
    ])?;

    let result = add_ext_column(df)?;
    let ext_col = result.column("ext")?.str()?;

    // Extensions should be lowercase
    assert_eq!(ext_col.get(0), Some("jpg"));
    assert_eq!(ext_col.get(1), Some("txt"));

    Ok(())
}
