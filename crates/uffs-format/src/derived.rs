// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Derived helpers consumed by the formatter:
//!
//! - `semantic_type_for_row` maps a row to the category label the `Type` output
//!   column emits (`"picture"`, `"code"`, `"directory"`, …).
//! - `bulkiness_for_row` returns the fixed-point packing ratio the `Bulkiness`
//!   column emits (allocated / logical, ×`1_000_000`).
//!
//! These mirror the originals in `uffs_core::search::derived` — kept
//! in sync via the `format_derived_matches_core_*` regression tests
//! in that module.  The duplication is intentional: `uffs-core`
//! still needs them on its pre-formatting hot paths (aggregation,
//! numeric top-N, sort-key derivation on `DisplayRow`), while the
//! formatter here needs a polars-free, trait-generic variant that
//! both `DisplayRow` and `SearchRow` can feed.

use crate::row::FormatRow;

/// Bulkiness fixed-point scale — `1.0 == BULKINESS_SCALE`.
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

/// Document file extensions (superset of the `collections::DOCUMENTS`
/// list in `uffs-core` — kept here so `uffs-format` stays crate-free
/// of the heavier `uffs-core::extensions` module).
const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "rtf", "txt", "md", "odt", "pages", "tex", "epub", "csv",
];
/// Picture file extensions.
const PICTURES: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "svg", "ico", "heic", "heif", "raw",
    "cr2", "nef", "arw", "dng", "psd", "ai",
];
/// Video file extensions.
const VIDEOS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "ts", "vob",
];
/// Audio / music file extensions.
const MUSIC: &[&str] = &[
    "mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus", "aiff", "ape", "alac",
];
/// Archive file extensions.
const ARCHIVES: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "cab", "arj", "lzma", "tgz", "iso",
];
/// Source-code file extensions.
const CODE: &[&str] = &[
    "rs", "py", "js", "ts", "java", "c", "cpp", "h", "hpp", "go", "rb", "kt", "swift", "scala",
    "cs", "vb", "fs", "ml", "hs", "clj", "dart", "zig", "nim", "cr", "jl",
];

/// Return the extension (no leading dot) of a filename.
///
/// Dot-gated: dotfiles (`.bash_history`), dotless names (`README`), and
/// trailing-dot names (`foo.`) all return `None`.  Visible to the rest
/// of the crate so [`writer::write_display_row_columns`] can format the
/// `Extension` column with the same rule as the sort key (regression:
/// T62 `--sort extension` MCP failure where `.bash_history`'s displayed
/// `ext` disagreed with its sort position).
pub(crate) fn extension_from_name(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    name.get(dot + 1..)
}

/// Map a row to its semantic type category string.
///
/// Used by the `Type` output column.  Directories always return
/// `"directory"`; files with no extension return `"file"`; otherwise
/// the filename's extension is matched against the category tables
/// above.  Unknown extensions fall through to `"other"`.
pub fn semantic_type_for_row<R: FormatRow>(row: &R) -> &'static str {
    if row.is_directory() {
        return "directory";
    }
    let Some(ext) = extension_from_name(row.name()) else {
        return "file";
    };
    let ext_lower = ext.to_ascii_lowercase();
    semantic_type_from_extension(&ext_lower)
}

/// Map an extension string to its category label.
#[must_use]
pub fn semantic_type_from_extension(ext: &str) -> &'static str {
    if DOCUMENTS.contains(&ext) {
        "document"
    } else if PICTURES.contains(&ext) {
        "picture"
    } else if VIDEOS.contains(&ext) {
        "video"
    } else if MUSIC.contains(&ext) {
        "audio"
    } else if ARCHIVES.contains(&ext) {
        "archive"
    } else if CODE.contains(&ext) {
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

/// Core bulkiness math — allocated / logical, fixed-point ×[`BULKINESS_SCALE`].
///
/// A logical size of zero returns zero (avoids divide-by-zero on
/// empty files).  `saturating_mul` caps the intermediate product at
/// `u64::MAX` so rows with pathologically large allocated sizes can
/// still be safely rendered.
#[inline]
const fn bulkiness_from_sizes(logical: u64, allocated: u64) -> u64 {
    if logical == 0 {
        return 0;
    }
    allocated.saturating_mul(BULKINESS_SCALE) / logical
}

/// Bulkiness metric scaled by [`BULKINESS_SCALE`].
///
/// Directories use `treesize` / `tree_allocated`; files use `size` /
/// `allocated`.  Matches `uffs_core::search::derived::bulkiness_for_row`
/// byte-for-byte on shared inputs.
pub fn bulkiness_for_row<R: FormatRow>(row: &R) -> u64 {
    let (logical, allocated) = if row.is_directory() {
        (row.treesize(), row.tree_allocated())
    } else {
        (row.size(), row.allocated())
    };
    bulkiness_from_sizes(logical, allocated)
}
