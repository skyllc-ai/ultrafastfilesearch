// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Derived search-field helpers shared by daemon projection/filter logic.

use super::backend::DisplayRow;
use crate::compact::CompactRecord;
use crate::extensions::collections;

/// Bulkiness fixed-point scale (`1.0 == 1_000_000`).
pub const BULKINESS_SCALE: u64 = 1_000_000;

/// Executable file extensions.
const EXECUTABLES: &[&str] = &["exe", "msi", "bat", "cmd", "ps1", "com", "scr"];
/// Script / interpreted file extensions (not compiled code).
const SCRIPTS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "csh", "ksh", "awk", "sed", "lua", "pl", "pm", "tcl",
];
/// Web / markup file extensions.
const WEB: &[&str] = &[
    "html", "htm", "css", "scss", "less", "sass", "jsx", "tsx", "vue", "svelte", "wasm", "xhtml",
];
/// Font file extensions.
const FONTS: &[&str] = &["ttf", "otf", "woff", "woff2", "eot", "fon"];
/// Database file extensions.
const DATABASES: &[&str] = &[
    "db", "sqlite", "sqlite3", "mdb", "accdb", "sql", "ldf", "mdf", "ndf", "dbf",
];
/// Configuration file extensions.
const CONFIGS: &[&str] = &[
    "ini",
    "cfg",
    "conf",
    "yaml",
    "yml",
    "toml",
    "json",
    "xml",
    "env",
    "properties",
    "reg",
    "inf",
    "plist",
];
/// Log file extensions.
const LOGS: &[&str] = &["log", "out", "err", "trace", "evt", "evtx"];
/// Backup/temporary file extensions.
const BACKUPS: &[&str] = &["bak", "old", "orig", "swp", "tmp", "temp", "~"];
/// Disk image / virtual disk extensions.
const DISK_IMAGES: &[&str] = &[
    "vmdk", "vhd", "vhdx", "vdi", "qcow2", "img", "wim", "iso", "dmg",
];
/// Data / serialization file extensions.
const DATA: &[&str] = &[
    "csv", "tsv", "parquet", "avro", "arrow", "ndjson", "jsonl", "dat", "sav", "hdf5",
];
/// CAD / 3D model file extensions.
const CAD: &[&str] = &[
    "dwg", "dxf", "step", "stp", "stl", "obj", "fbx", "blend", "3ds", "gltf", "glb",
];
/// Shortcut / link file extensions.
const SHORTCUTS: &[&str] = &["lnk", "url", "desktop", "webloc"];
/// System / driver file extensions.
const SYSTEM: &[&str] = &["sys", "dll", "drv", "ocx", "cpl", "ax", "mui"];
/// Certificate / security file extensions.
const CERTS: &[&str] = &[
    "pem", "crt", "cer", "der", "pfx", "p12", "key", "csr", "jks",
];
/// E-book file extensions.
const EBOOKS: &[&str] = &["epub", "mobi", "azw", "azw3", "djvu", "cbr", "cbz"];

/// Return the lowercase extension without a leading dot.
#[must_use]
pub fn extension_from_name(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    name.get(dot + 1..)
}

/// Return whether a name is a reserved NTFS metafile (`$MFT`, `$LogFile`,
/// the `$Extend` family, …).
///
/// Delegates to [`crate::compact::is_ntfs_metafile_name`], the single source of
/// truth.  An ordinary `$`-prefixed file (`$Recycle.Bin`, `$PatchCache`, the
/// `WinSxS` `$$_*.cdf-ms` filemaps) is NOT a metafile and returns `false`.
#[must_use]
pub fn is_system_name(name: &str) -> bool {
    crate::compact::is_ntfs_metafile_name(name)
}

/// Semantic type/category name for a row.
#[must_use]
pub fn semantic_type_for_row(row: &DisplayRow) -> &'static str {
    if row.is_directory {
        return "directory";
    }

    let Some(ext) = extension_from_name(row.name()) else {
        return "file";
    };
    let ext_lower = ext.to_ascii_lowercase();
    semantic_type_from_extension(&ext_lower)
}

/// All known type categories for CLI `--type` help / validation.
pub const ALL_TYPE_CATEGORIES: &[&str] = &[
    "archive",
    "audio",
    "backup",
    "cad",
    "cert",
    "code",
    "config",
    "data",
    "database",
    "directory",
    "disk",
    "document",
    "ebook",
    "executable",
    "file",
    "font",
    "log",
    "other",
    "picture",
    "script",
    "shortcut",
    "system",
    "video",
    "web",
];

/// Numeric type IDs for aggregation bucketing (round-trips with
/// [`semantic_type_name_from_id`]).
///
/// The order matches [`SEMANTIC_TYPE_NAMES`].
#[must_use]
pub(crate) fn semantic_type_id_from_extension(ext: &str) -> u64 {
    // Must stay in sync with SEMANTIC_TYPE_NAMES.
    if collections::DOCUMENTS.contains(&ext) {
        1
    } else if collections::PICTURES.contains(&ext) {
        2
    } else if collections::VIDEOS.contains(&ext) {
        3
    } else if collections::MUSIC.contains(&ext) {
        4
    } else if collections::ARCHIVES.contains(&ext) {
        5
    } else if collections::CODE.contains(&ext) {
        6
    } else if EXECUTABLES.contains(&ext) {
        7
    } else if SCRIPTS.contains(&ext) {
        8
    } else if WEB.contains(&ext) {
        9
    } else if FONTS.contains(&ext) {
        10
    } else if DATABASES.contains(&ext) {
        11
    } else if CONFIGS.contains(&ext) {
        12
    } else if LOGS.contains(&ext) {
        13
    } else if BACKUPS.contains(&ext) {
        14
    } else if DISK_IMAGES.contains(&ext) {
        15
    } else if DATA.contains(&ext) {
        16
    } else if CAD.contains(&ext) {
        17
    } else if SHORTCUTS.contains(&ext) {
        18
    } else if SYSTEM.contains(&ext) {
        19
    } else if CERTS.contains(&ext) {
        20
    } else if EBOOKS.contains(&ext) {
        21
    } else {
        0 // "other" / no extension
    }
}

/// Ordered names matching the IDs from [`semantic_type_id_from_extension`].
///
/// Index 0 = "other", index 22 = "directory", index 23 = "file".
pub(crate) const SEMANTIC_TYPE_NAMES: &[&str] = &[
    "other",      // 0
    "document",   // 1
    "picture",    // 2
    "video",      // 3
    "audio",      // 4
    "archive",    // 5
    "code",       // 6
    "executable", // 7
    "script",     // 8
    "web",        // 9
    "font",       // 10
    "database",   // 11
    "config",     // 12
    "log",        // 13
    "backup",     // 14
    "disk",       // 15
    "data",       // 16
    "cad",        // 17
    "shortcut",   // 18
    "system",     // 19
    "cert",       // 20
    "ebook",      // 21
    "directory",  // 22
    "file",       // 23
];

/// ID for directory entries (not extension-based).
pub(crate) const SEMANTIC_TYPE_ID_DIRECTORY: u64 = 22;

/// ID for files with no extension.
pub(crate) const SEMANTIC_TYPE_ID_FILE: u64 = 23;

/// Resolve a numeric type ID back to a category name.
#[must_use]
pub(crate) fn semantic_type_name_from_id(id: u64) -> &'static str {
    SEMANTIC_TYPE_NAMES
        .get(usize::try_from(id).unwrap_or(usize::MAX))
        .copied()
        .unwrap_or("other")
}

/// Semantic type/category name for an extension.
#[must_use]
pub fn semantic_type_from_extension(ext: &str) -> &'static str {
    if collections::DOCUMENTS.contains(&ext) {
        "document"
    } else if collections::PICTURES.contains(&ext) {
        "picture"
    } else if collections::VIDEOS.contains(&ext) {
        "video"
    } else if collections::MUSIC.contains(&ext) {
        "audio"
    } else if collections::ARCHIVES.contains(&ext) {
        "archive"
    } else if collections::CODE.contains(&ext) {
        "code"
    } else if EXECUTABLES.contains(&ext) {
        "executable"
    } else if SCRIPTS.contains(&ext) {
        "script"
    } else if WEB.contains(&ext) {
        "web"
    } else if FONTS.contains(&ext) {
        "font"
    } else if DATABASES.contains(&ext) {
        "database"
    } else if CONFIGS.contains(&ext) {
        "config"
    } else if LOGS.contains(&ext) {
        "log"
    } else if BACKUPS.contains(&ext) {
        "backup"
    } else if DISK_IMAGES.contains(&ext) {
        "disk"
    } else if DATA.contains(&ext) {
        "data"
    } else if CAD.contains(&ext) {
        "cad"
    } else if SHORTCUTS.contains(&ext) {
        "shortcut"
    } else if SYSTEM.contains(&ext) {
        "system"
    } else if CERTS.contains(&ext) {
        "cert"
    } else if EBOOKS.contains(&ext) {
        "ebook"
    } else {
        "other"
    }
}

/// Return the extension list for a semantic type category (if
/// extension-mappable).
///
/// Types like `"directory"`, `"file"`, and `"other"` return `None` because
/// they are not defined by a fixed set of extensions.
#[must_use]
pub(crate) fn extensions_for_type(type_name: &str) -> Option<&'static [&'static str]> {
    match type_name {
        "document" => Some(collections::DOCUMENTS),
        "picture" => Some(collections::PICTURES),
        "video" => Some(collections::VIDEOS),
        "audio" => Some(collections::MUSIC),
        "archive" => Some(collections::ARCHIVES),
        "code" => Some(collections::CODE),
        "executable" => Some(EXECUTABLES),
        "script" => Some(SCRIPTS),
        "web" => Some(WEB),
        "font" => Some(FONTS),
        "database" => Some(DATABASES),
        "config" => Some(CONFIGS),
        "log" => Some(LOGS),
        "backup" => Some(BACKUPS),
        "disk" => Some(DISK_IMAGES),
        "data" => Some(DATA),
        "cad" => Some(CAD),
        "shortcut" => Some(SHORTCUTS),
        "system" => Some(SYSTEM),
        "cert" => Some(CERTS),
        "ebook" => Some(EBOOKS),
        // Not extension-mappable (directory/file/other and unknown):
        _ => None,
    }
}

/// Tree-allocated metric for projection/sort/filter.
#[must_use]
pub const fn tree_allocated_for_row(row: &DisplayRow) -> u64 {
    if row.is_directory {
        row.tree_allocated
    } else {
        row.allocated
    }
}

/// Core bulkiness math, shared by [`bulkiness_for_row`] and
/// [`bulkiness_for_record`].
///
/// Splitting the math from the field-picker lets the two wrappers
/// stay thin and guarantees they cannot drift.
#[must_use]
#[inline]
const fn bulkiness_from_sizes(logical: u64, allocated: u64) -> u64 {
    if logical == 0 {
        return 0;
    }
    allocated.saturating_mul(BULKINESS_SCALE) / logical
}

/// Bulkiness metric as fixed-point ratio scaled by [`BULKINESS_SCALE`].
#[must_use]
pub const fn bulkiness_for_row(row: &DisplayRow) -> u64 {
    let (logical, allocated) = if row.is_directory {
        (row.treesize, row.tree_allocated)
    } else {
        (row.size, row.allocated)
    };
    bulkiness_from_sizes(logical, allocated)
}

/// Same metric as [`bulkiness_for_row`] but computed directly from a
/// [`CompactRecord`] — avoids the `DisplayRow` allocation dance on
/// the hot sort-key path.
///
/// This is the *only* caller-visible difference from `bulkiness_for_row`.
/// `CompactRecord` holds the five fields the bulkiness formula actually
/// needs (`is_directory`, `size`, `allocated`, `treesize`,
/// `tree_allocated`), so routing through `DisplayRow::new(...)` (with
/// a `String::new()` path, a zeroed `record_index`, etc.) was pure
/// waste — measured ~μs per candidate on large extension buckets.
///
/// Result is byte-identical to `bulkiness_for_row` called on a
/// `DisplayRow` constructed from the same record; the equivalence is
/// pinned by `bulkiness_for_record_matches_bulkiness_for_row`.
#[must_use]
pub const fn bulkiness_for_record(rec: &CompactRecord) -> u64 {
    let (logical, allocated) = if rec.is_directory() {
        (rec.treesize, rec.tree_allocated)
    } else {
        (rec.size, rec.allocated)
    };
    bulkiness_from_sizes(logical, allocated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::backend::DisplayRow;

    fn file_row(path: &str, size: u64) -> DisplayRow {
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            path.to_owned(),
            size,
            false,
            0,
            0,
            0,
            0x20,
            size,
            0,
            0,
            0,
        )
    }

    fn dir_row(path: &str, treesize: u64, tree_alloc: u64) -> DisplayRow {
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            path.to_owned(),
            0,
            true,
            0,
            0,
            0,
            0x10,
            0,
            3,
            treesize,
            tree_alloc,
        )
    }

    // ── semantic_type_for_row ─────────────────────────────────────────

    #[test]
    fn semantic_type_directory() {
        assert_eq!(
            semantic_type_for_row(&dir_row("C:\\mydir", 0, 0)),
            "directory"
        );
    }

    #[test]
    fn semantic_type_file_no_extension() {
        assert_eq!(semantic_type_for_row(&file_row("C:\\Makefile", 10)), "file");
    }

    // ── semantic_type_from_extension — all 21 categories ─────────────

    #[test]
    fn semantic_type_document() {
        for ext in &["pdf", "docx", "txt", "md", "rtf"] {
            assert_eq!(semantic_type_from_extension(ext), "document", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_picture() {
        for ext in &["jpg", "png", "gif", "svg", "bmp", "heic"] {
            assert_eq!(semantic_type_from_extension(ext), "picture", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_video() {
        for ext in &["mp4", "mkv", "mov", "avi", "wmv"] {
            assert_eq!(semantic_type_from_extension(ext), "video", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_audio() {
        for ext in &["mp3", "flac", "wav", "aac", "ogg"] {
            assert_eq!(semantic_type_from_extension(ext), "audio", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_archive() {
        for ext in &["zip", "rar", "7z", "tar", "gz"] {
            assert_eq!(semantic_type_from_extension(ext), "archive", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_code() {
        for ext in &["rs", "py", "js", "java", "c", "go", "cpp", "ts"] {
            assert_eq!(semantic_type_from_extension(ext), "code", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_executable() {
        for ext in &["exe", "msi", "bat", "cmd", "ps1"] {
            assert_eq!(semantic_type_from_extension(ext), "executable", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_script() {
        for ext in &["sh", "bash", "lua", "pl"] {
            assert_eq!(semantic_type_from_extension(ext), "script", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_web() {
        for ext in &["html", "css", "jsx", "vue", "wasm"] {
            assert_eq!(semantic_type_from_extension(ext), "web", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_font() {
        for ext in &["ttf", "otf", "woff", "woff2"] {
            assert_eq!(semantic_type_from_extension(ext), "font", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_database() {
        for ext in &["db", "sqlite", "sql", "mdf"] {
            assert_eq!(semantic_type_from_extension(ext), "database", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_config() {
        for ext in &["ini", "yaml", "toml", "json", "xml"] {
            assert_eq!(semantic_type_from_extension(ext), "config", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_log() {
        for ext in &["log", "out", "err"] {
            assert_eq!(semantic_type_from_extension(ext), "log", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_backup() {
        for ext in &["bak", "old", "tmp", "swp"] {
            assert_eq!(semantic_type_from_extension(ext), "backup", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_disk_image() {
        // Note: "iso" is in ARCHIVES (checked before DISK_IMAGES), so it maps to
        // "archive".
        for ext in &["vmdk", "vhd", "img", "wim"] {
            assert_eq!(semantic_type_from_extension(ext), "disk", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_data() {
        // Note: "csv" is in DOCUMENTS (checked before DATA), so it maps to "document".
        for ext in &["parquet", "avro", "arrow", "ndjson"] {
            assert_eq!(semantic_type_from_extension(ext), "data", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_system() {
        for ext in &["sys", "dll", "drv"] {
            assert_eq!(semantic_type_from_extension(ext), "system", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_cert() {
        for ext in &["pem", "crt", "cer", "pfx"] {
            assert_eq!(semantic_type_from_extension(ext), "cert", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_ebook() {
        for ext in &["epub", "mobi"] {
            assert_eq!(semantic_type_from_extension(ext), "ebook", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_shortcut() {
        assert_eq!(semantic_type_from_extension("lnk"), "shortcut");
        assert_eq!(semantic_type_from_extension("url"), "shortcut");
    }

    #[test]
    fn semantic_type_cad() {
        for ext in &["dwg", "dxf", "stl"] {
            assert_eq!(semantic_type_from_extension(ext), "cad", "ext={ext}");
        }
    }

    #[test]
    fn semantic_type_unknown_is_other() {
        assert_eq!(semantic_type_from_extension("xyz123"), "other");
        assert_eq!(semantic_type_from_extension("zzz"), "other");
    }

    // ── bulkiness & tree_allocated ────────────────────────────────────

    #[test]
    fn bulkiness_uses_tree_metrics_for_directories() {
        let row = dir_row("C:\\dir", 200, 300);
        assert_eq!(tree_allocated_for_row(&row), 300);
        assert_eq!(bulkiness_for_row(&row), 1_500_000);
    }

    #[test]
    fn bulkiness_uses_file_metrics_for_files() {
        let row = DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            "C:\\f.txt".to_owned(),
            1000,
            false,
            0,
            0,
            0,
            0x20,
            4096,
            0,
            0,
            0,
        );
        assert_eq!(tree_allocated_for_row(&row), 4096);
        assert_eq!(bulkiness_for_row(&row), 4_096_000); // 4096/1000 * 1M
    }

    #[test]
    fn bulkiness_zero_logical_size_returns_zero() {
        let row = file_row("C:\\empty", 0);
        assert_eq!(bulkiness_for_row(&row), 0);
    }

    // ── bulkiness_for_record equivalence (perf refactor guard) ────────

    /// Build a `CompactRecord` whose `size` / `allocated` / `treesize` /
    /// `tree_allocated` fields mirror the supplied values and whose
    /// directory bit is set
    /// per `is_directory`.  All other fields are zero — they don't affect
    /// `bulkiness_for_record`.
    fn compact_record(
        is_directory: bool,
        size: u64,
        allocated: u64,
        treesize: u64,
        tree_allocated: u64,
    ) -> CompactRecord {
        CompactRecord {
            size,
            allocated,
            treesize,
            tree_allocated,
            flags: if is_directory { 0x10 } else { 0x20 },
            ..CompactRecord::default()
        }
    }

    /// File record: `bulkiness_for_record` must return the same value as
    /// `bulkiness_for_row` given equivalent inputs.  Pins the two
    /// wrappers against silent drift in either `bulkiness_from_sizes`
    /// or the field-picker branches.
    #[test]
    fn bulkiness_for_record_matches_bulkiness_for_row_file() {
        let rec = compact_record(false, 1_000, 4_096, 0, 0);
        let row = DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            String::new(),
            rec.size,
            rec.is_directory(),
            0,
            0,
            0,
            rec.flags,
            rec.allocated,
            0,
            rec.treesize,
            rec.tree_allocated,
        );
        assert_eq!(bulkiness_for_record(&rec), bulkiness_for_row(&row));
        assert_eq!(bulkiness_for_record(&rec), 4_096_000);
    }

    /// Directory record: same equivalence must hold when the logical
    /// and allocated pair is sourced from `treesize` / `tree_allocated`
    /// instead of `size` / `allocated`.
    #[test]
    fn bulkiness_for_record_matches_bulkiness_for_row_directory() {
        let rec = compact_record(true, 0, 0, 200, 300);
        let row = dir_row("C:\\dir", 200, 300);
        assert_eq!(bulkiness_for_record(&rec), bulkiness_for_row(&row));
        assert_eq!(bulkiness_for_record(&rec), 1_500_000);
    }

    /// Zero-logical edge case must agree between both wrappers — and
    /// must not panic on the internal divide-by-zero guard.
    #[test]
    fn bulkiness_for_record_zero_logical_returns_zero() {
        let file = compact_record(false, 0, 512, 0, 0);
        let dir = compact_record(true, 0, 0, 0, 512);
        assert_eq!(bulkiness_for_record(&file), 0);
        assert_eq!(bulkiness_for_record(&dir), 0);
    }

    /// `saturating_mul` inside the formula must prevent overflow at
    /// the `u64::MAX * BULKINESS_SCALE` limit.  Regression pin for the
    /// numeric top-N hot path — a panic here would take the whole
    /// daemon down under adversarial input.
    #[test]
    fn bulkiness_for_record_does_not_panic_on_extreme_sizes() {
        let rec = compact_record(false, 1, u64::MAX, 0, 0);
        // `u64::MAX * 1_000_000` saturates; divided by logical=1 it
        // stays at u64::MAX.  The important invariant is "does not
        // panic", not the exact numeric output.
        assert_eq!(bulkiness_for_record(&rec), u64::MAX);
    }

    #[test]
    fn all_type_categories_cover_known_list() {
        // Ensure the static list is complete (24 categories)
        assert_eq!(ALL_TYPE_CATEGORIES.len(), 24);
        assert!(ALL_TYPE_CATEGORIES.contains(&"code"));
        assert!(ALL_TYPE_CATEGORIES.contains(&"directory"));
        assert!(ALL_TYPE_CATEGORIES.contains(&"file"));
        assert!(ALL_TYPE_CATEGORIES.contains(&"other"));
    }

    // ── extensions_for_type ──────────────────────────────────────────

    #[test]
    fn extensions_for_type_code_contains_rs() {
        let exts = extensions_for_type("code").unwrap();
        assert!(exts.contains(&"rs"), "code should contain rs");
        assert!(exts.contains(&"py"), "code should contain py");
    }

    #[test]
    fn extensions_for_type_unmappable_returns_none() {
        assert!(extensions_for_type("directory").is_none());
        assert!(extensions_for_type("file").is_none());
        assert!(extensions_for_type("other").is_none());
    }

    #[test]
    fn extensions_for_type_covers_all_mappable_categories() {
        let mappable = [
            "document",
            "picture",
            "video",
            "audio",
            "archive",
            "code",
            "executable",
            "script",
            "web",
            "font",
            "database",
            "config",
            "log",
            "backup",
            "disk",
            "data",
            "cad",
            "shortcut",
            "system",
            "cert",
            "ebook",
        ];
        for cat in mappable {
            assert!(
                extensions_for_type(cat).is_some(),
                "expected Some for type {cat}"
            );
        }
    }
}
