// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! The 80-byte [`CompactRecord`] row type + the NTFS metafile-name allowlist.
//!
//! Extracted from `compact.rs` (file-size decomposition); the public path
//! `crate::compact::CompactRecord` is preserved via re-export.

/// Compact per-record data for in-memory search, filter, and sort.
///
/// 80 bytes per record (76 data + 4 explicit tail padding).
/// Derives `bytemuck::Pod` + `Zeroable` so the entire record array can be
/// serialized/deserialized as a single bulk `memcpy` — no per-field encoding.
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct CompactRecord {
    // ── u64 fields first (8-byte aligned) ─────────────────────────
    /// Logical file size in bytes.
    pub size: u64,
    /// Allocated size on disk in bytes ("Size on Disk" column).
    pub allocated: u64,
    /// Sum of logical file sizes in entire subtree.
    pub treesize: u64,
    /// Sum of allocated sizes in entire subtree.
    pub tree_allocated: u64,
    /// Creation time (Unix microseconds).
    pub created: i64,
    /// Last write time (Unix microseconds).
    pub modified: i64,
    /// Last access time (Unix microseconds).
    pub accessed: i64,

    // ── u32 fields (4-byte aligned) ───────────────────────────────
    /// Byte offset into the names blob.
    pub name_offset: u32,
    /// Raw NTFS `FILE_ATTRIBUTE_*` flags.
    pub flags: u32,
    /// Index into the compact array of the parent directory.
    /// `u32::MAX` = root or orphan.
    pub parent_idx: u32,
    /// Count of all descendants in subtree. 0 for files.
    pub descendants: u32,

    // ── u16 fields (2-byte aligned) ───────────────────────────────
    /// UTF-8 byte length of the filename.
    pub name_len: u16,
    /// Interned extension ID (0 = no extension).
    pub extension_id: u16,
    /// Full path length in UTF-8 bytes (e.g. `C:\Windows\System32\cmd.exe` =
    /// 28). Precomputed at index build time via top-down parent-chain walk.
    /// Saturates at `u16::MAX` (65 535) for extremely deep paths.
    pub path_len: u16,

    /// First byte of the filename (e.g. `b'$'` for NTFS metafiles).
    ///
    /// Cached here as a cheap hot-path *gate*: only `$`-prefixed records can be
    /// NTFS metafiles, so [`is_system_metafile`](Self::is_system_metafile) can
    /// reject virtually every record with one sequential field read instead of
    /// a random cache-miss into the names arena.  The handful of `$`-prefixed
    /// candidates then pay one arena lookup for the authoritative name check.
    pub name_first_byte: u8,

    /// Explicit tail padding for 8-byte struct alignment.
    /// Required by `bytemuck::Pod` — no implicit padding allowed.
    #[expect(
        clippy::pub_underscore_fields,
        reason = "bytemuck Pod requires all fields same visibility"
    )]
    pub _pad: [u8; 1],
}

/// The fixed set of reserved NTFS metafile names: the `$`-prefixed records at
/// reserved FRS 0–15 and under the `$Extend` directory.  An NTFS volume can
/// only ever contain *these* specific metafiles.
///
/// Any *other* `$`-prefixed name — `$Recycle.Bin`, `$PatchCache`,
/// `$WinREAgent`, the `WinSxS` `$$_*.cdf-ms` filemaps, or a user file literally
/// named `$foo` — is an ordinary file that file managers and tools like
/// Everything display. Classifying those as metafiles is exactly the bug
/// `--hide-system` had.
///
/// Matched case-insensitively: NTFS itself is case-insensitive, and these
/// canonical names are occasionally surfaced with varied casing.
pub(crate) const NTFS_METAFILE_NAMES: &[&str] = &[
    // Reserved FRS 0–11 (volume root metafiles)
    "$MFT",
    "$MFTMirr",
    "$LogFile",
    "$Volume",
    "$AttrDef",
    "$Bitmap",
    "$Boot",
    "$BadClus",
    "$Secure",
    "$UpCase",
    "$Extend",
    // `$Extend` directory children
    "$ObjId",
    "$Quota",
    "$Reparse",
    "$UsnJrnl",
    "$RmMetadata",
    "$Deleted",
    // `$Extend\$RmMetadata` children
    "$Repair",
    "$Tops",
    "$TxfLog",
    "$Txf",
];

/// Returns whether `name` is one of the reserved `NTFS_METAFILE_NAMES`
/// (a crate-private allowlist, so no intra-doc link from this public item).
///
/// Real metafiles are already excluded from the compact index at build time
/// (`build_compact_index` drops them via `PathResolver` FRS-validity, not by
/// name).  This exact-name check is the *authoritative* classifier for the
/// `--hide-system` filter, so it can never misclassify an ordinary
/// `$`-prefixed file as a metafile.
#[must_use]
#[inline]
pub fn is_ntfs_metafile_name(name: &str) -> bool {
    NTFS_METAFILE_NAMES
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

impl CompactRecord {
    /// Directory flag bit in raw NTFS `FILE_ATTRIBUTE_DIRECTORY`.
    const DIRECTORY_BIT: u32 = 0x0010;

    /// Returns `true` if this record is a directory.
    #[inline]
    #[must_use]
    pub const fn is_directory(self) -> bool {
        self.flags & Self::DIRECTORY_BIT != 0
    }

    /// Returns `true` if this record is one of the reserved NTFS metafiles
    /// (`$MFT`, `$LogFile`, `$Bitmap`, `$Secure`, the `$Extend` family, …).
    ///
    /// The cached [`name_first_byte`](Self::name_first_byte) field is a cheap
    /// gate: every metafile name starts with `$`, and `$`-prefixed records are
    /// a vanishing fraction of an index, so this rejects virtually every record
    /// with a single byte comparison and only touches the names arena for the
    /// handful of `$`-prefixed candidates.  The arena lookup is *required* for
    /// correctness, because an ordinary file may also start with `$`
    /// (`$Recycle.Bin`, `$PatchCache`, the `WinSxS` `$$_*.cdf-ms` filemaps) —
    /// those are NOT metafiles and must not be hidden by `--hide-system`.
    /// See [`is_ntfs_metafile_name`].
    #[inline]
    #[must_use]
    pub fn is_system_metafile(&self, names: &[u8]) -> bool {
        self.name_first_byte == b'$' && is_ntfs_metafile_name(self.name(names))
    }

    /// Get the name from a names blob as a **lossy `&str` view**.
    ///
    /// Valid-UTF-8 names (the common case) are returned verbatim; an ill-formed
    /// (surrogate-bearing) name stored as WTF-8 returns `""` for display. Use
    /// [`Self::name_bytes`] for the lossless bytes that exact/substring search
    /// matches against, so a file with an ill-formed name stays findable
    /// (WI-4.4).
    #[inline]
    #[must_use]
    pub fn name<'a>(&self, names: &'a [u8]) -> &'a str {
        core::str::from_utf8(self.name_bytes(names)).unwrap_or("")
    }

    /// Get the name's **raw bytes** (WTF-8) from a names blob — the lossless
    /// accessor.
    ///
    /// Returns exactly the stored bytes, including the byte-faithful encoding
    /// of an ill-formed NTFS name (unpaired surrogates). This is what makes
    /// every file matchable/findable by its true name regardless of UTF-8
    /// well-formedness (WI-4.4). Returns `&[]` for an out-of-range slice.
    #[inline]
    #[must_use]
    pub fn name_bytes<'a>(&self, names: &'a [u8]) -> &'a [u8] {
        let start = self.name_offset as usize;
        let end = start.saturating_add(self.name_len as usize);
        names.get(start..end).unwrap_or(&[])
    }
}

// Compile-time size assertion.
const _: () = assert!(
    size_of::<CompactRecord>() == 80,
    "CompactRecord must be exactly 80 bytes"
);
