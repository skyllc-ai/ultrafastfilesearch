//! Tests for output column parsing and formatting behavior.

use uffs_polars::{Column, DataFrame};

use super::*;

#[test]
fn test_parse_column() {
    assert_eq!(OutputColumn::parse("path"), Some(OutputColumn::Path));
    assert_eq!(OutputColumn::parse("PATH"), Some(OutputColumn::Path));
    assert_eq!(OutputColumn::parse("size"), Some(OutputColumn::Size));
    assert_eq!(OutputColumn::parse("unknown"), None);
}

#[test]
fn test_parse_columns_all() {
    assert!(OutputConfig::parse_columns("all").is_none());
    assert!(OutputConfig::parse_columns("ALL").is_none());
}

#[test]
fn test_parse_columns_list() {
    let cols = OutputConfig::parse_columns("path,name,size").expect("should parse");
    assert_eq!(cols.len(), 3);
    assert_eq!(cols.first(), Some(&OutputColumn::Path));
    assert_eq!(cols.get(1), Some(&OutputColumn::Name));
    assert_eq!(cols.get(2), Some(&OutputColumn::Size));
}

#[test]
fn test_parse_separator_special() {
    // Original values
    assert_eq!(OutputConfig::parse_separator("TAB"), "\t");
    assert_eq!(OutputConfig::parse_separator("tab"), "\t");
    assert_eq!(OutputConfig::parse_separator("NEWLINE"), "\n");
    assert_eq!(OutputConfig::parse_separator(";"), ";");
    // CPP compatibility values
    assert_eq!(OutputConfig::parse_separator("NEW LINE"), "\n");
    assert_eq!(OutputConfig::parse_separator("SPACE"), " ");
    assert_eq!(OutputConfig::parse_separator("RETURN"), "\r");
    assert_eq!(OutputConfig::parse_separator("DOUBLE"), "\"");
    assert_eq!(OutputConfig::parse_separator("SINGLE"), "'");
    assert_eq!(OutputConfig::parse_separator("NULL"), "\0");
}

#[test]
fn test_parse_column_aliases() {
    // Short aliases for CPP compatibility
    assert_eq!(OutputColumn::parse("r"), Some(OutputColumn::ReadOnly));
    assert_eq!(OutputColumn::parse("a"), Some(OutputColumn::Archive));
    assert_eq!(OutputColumn::parse("s"), Some(OutputColumn::System));
    assert_eq!(OutputColumn::parse("h"), Some(OutputColumn::Hidden));
    assert_eq!(OutputColumn::parse("o"), Some(OutputColumn::Offline));
    // CPP name aliases
    assert_eq!(OutputColumn::parse("written"), Some(OutputColumn::Modified));
    assert_eq!(
        OutputColumn::parse("notcontent"),
        Some(OutputColumn::NotIndexed)
    );
    assert_eq!(OutputColumn::parse("directory"), Some(OutputColumn::Type));
    // CPP typo support
    assert_eq!(
        OutputColumn::parse("decendents"),
        Some(OutputColumn::Descendants)
    );
}

#[test]
fn test_output_config_builder() {
    let config = OutputConfig::new()
        .with_columns("path,name")
        .with_separator(";")
        .with_quote("'")
        .with_header(false)
        .with_pos("+")
        .with_neg("-");

    assert!(config.columns.is_some());
    assert_eq!(config.separator, ";");
    assert_eq!(config.quote, "'");
    assert!(!config.header);
    assert_eq!(config.pos, "+");
    assert_eq!(config.neg, "-");
}

#[test]
fn test_df_column_mapping() {
    assert_eq!(OutputColumn::Path.df_column(), "path");
    assert_eq!(OutputColumn::SizeOnDisk.df_column(), "allocated_size");
    assert_eq!(OutputColumn::AttributeValue.df_column(), "flags");
}

#[test]
fn test_display_name() {
    assert_eq!(OutputColumn::Path.display_name(), "Path");
    assert_eq!(OutputColumn::SizeOnDisk.display_name(), "Size on Disk");
    assert_eq!(
        OutputColumn::NotIndexed.display_name(),
        "Not content indexed file"
    );
}

#[test]
fn test_needs_descendants() {
    let config_no_desc = OutputConfig::new().with_columns("path,name,size");
    assert!(!config_no_desc.needs_descendants());

    let config_with_desc = OutputConfig::new().with_columns("path,descendants,size");
    assert!(config_with_desc.needs_descendants());

    // "all" columns returns None, so needs_descendants should be false
    let config_all = OutputConfig::new().with_columns("all");
    assert!(!config_all.needs_descendants());
}

#[test]
fn test_add_descendants_column() {
    // Create a test DataFrame with directory structure:
    // root (5) -> Users (100) -> john (101) -> file.txt (102)
    //                         -> Documents (103) -> doc.pdf (104)
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[5_u64, 100, 101, 102, 103, 104]),
        Column::new("parent_frs".into(), &[0_u64, 5, 100, 101, 100, 103]),
        Column::new(
            "is_directory".into(),
            &[true, true, true, false, true, false],
        ),
        Column::new("size".into(), &[0_u64, 0, 0, 1000, 0, 50000]),
        Column::new(
            "allocated_size".into(),
            &[4096_u64, 4096, 4096, 4096, 4096, 53248],
        ),
    ])
    .unwrap();

    let result = add_descendants_column(&df).unwrap();

    // Check that descendants column was added
    let desc_col = result.column("descendants").unwrap().u64().unwrap();

    // Descendants = direct children + all their descendants (recursive)
    // root (5): children=[Users(100)] -> 1 + descendants(100) = 1 + 4 = 5
    // Users (100): children=[john(101), Documents(103)] -> 2 + desc(101) +
    // desc(103) = 2 + 1 + 1 = 4 john (101): children=[file.txt(102)] -> 1 +
    // 0 = 1 file.txt (102): 0 (file)
    // Documents (103): children=[doc.pdf(104)] -> 1 + 0 = 1
    // doc.pdf (104): 0 (file)
    assert_eq!(desc_col.get(0), Some(5)); // root -> all 5 items below
    assert_eq!(desc_col.get(1), Some(4)); // Users -> john, file.txt, Documents, doc.pdf
    assert_eq!(desc_col.get(2), Some(1)); // john -> file.txt
    assert_eq!(desc_col.get(3), Some(0)); // file.txt (file)
    assert_eq!(desc_col.get(4), Some(1)); // Documents -> doc.pdf
    assert_eq!(desc_col.get(5), Some(0)); // doc.pdf (file)
}
