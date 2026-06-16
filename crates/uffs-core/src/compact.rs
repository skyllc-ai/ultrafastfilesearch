// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Compact in-memory index for search backends.
//!
//! Replaces the full `MftIndex` (224 bytes/record) with a lean 72-byte
//! `CompactRecord` that covers 100% of sortable/filterable columns.
//! Full metadata (ADS, forensic fields) is resolved on-demand from the
//! `.uffs` cache file.
//!
//! See `docs/architecture/COMPACT_INDEX_DESIGN.md` for the full design.
//! Exception: `file_size_policy` — core data structures + builder, tightly
//! coupled.

use std::time::Instant;

use rayon::prelude::*;
use uffs_mft::index::MftIndex;

use crate::bloom::Bloom;
pub use crate::compact_loader::apply_usn_patch;
// Re-export loader types and functions so callers can still use `compact::*`.
#[expect(deprecated, reason = "re-export kept for backward compatibility")]
pub use crate::compact_loader::{
    IndexSource, LoadTiming, MftSource, PatchStats, load_drive, load_mft_file, refresh_drive,
};
use crate::compact_storage::ColumnStorage;
use crate::path_trie::PathTrie;
use crate::trigram::TrigramIndex;

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

/// Children index in CSR (Compressed Sparse Row) layout.
///
/// `children(i)` returns the compact indices of record i's children as
/// a contiguous `&[u32]` slice.  The CSR layout avoids per-record `Vec`
/// allocations and enables bulk serialization/deserialization.
#[derive(Clone)]
pub struct ChildrenIndex {
    /// CSR offsets — one per record + sentinel.  Length = `record_count` + 1.
    /// Children of record `i` are `values[offsets[i]..offsets[i+1]]`.
    offsets: Vec<u32>,
    /// Flat array of all child indices.
    values: Vec<u32>,
}

impl ChildrenIndex {
    /// Total heap capacity (offsets + values) in bytes.
    #[must_use]
    pub const fn heap_size_bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.values.capacity() * size_of::<u32>()
    }

    /// Build from `CompactRecord::parent_idx` in two passes (count + scatter).
    #[must_use]
    pub fn build(records: &[CompactRecord]) -> Self {
        // Count children per parent
        let mut counts = vec![0_u32; records.len()];
        for rec in records {
            let parent = rec.parent_idx;
            if parent != u32::MAX
                && let Some(cnt) = counts.get_mut(parent as usize)
            {
                *cnt += 1;
            }
        }

        // Prefix-sum → offsets
        let mut offsets = Vec::with_capacity(records.len() + 1);
        let mut running = 0_u32;
        for &cnt in &counts {
            offsets.push(running);
            running = running.saturating_add(cnt);
        }
        offsets.push(running);

        // Scatter children into values
        let mut values = vec![0_u32; running as usize];
        let mut write_pos = offsets.clone();
        for (idx, rec) in records.iter().enumerate() {
            let parent = rec.parent_idx;
            if parent != u32::MAX
                && let Some(pos) = write_pos.get_mut(parent as usize)
                && let Some(slot) = values.get_mut(*pos as usize)
            {
                let child_idx = uffs_mft::len_to_u32(idx);
                *slot = child_idx;
                *pos += 1;
            }
        }

        Self { offsets, values }
    }

    /// Construct directly from pre-built CSR arrays (cache deserialization).
    #[must_use]
    pub const fn from_csr(offsets: Vec<u32>, values: Vec<u32>) -> Self {
        Self { offsets, values }
    }

    /// Borrow the CSR components for serialization.
    #[must_use]
    pub(crate) fn as_csr(&self) -> (&[u32], &[u32]) {
        (&self.offsets, &self.values)
    }

    /// Return the children of record `idx` as a contiguous slice.
    #[must_use]
    pub fn get(&self, idx: usize) -> &[u32] {
        let start = self.offsets.get(idx).copied().unwrap_or(0) as usize;
        let end = self.offsets.get(idx + 1).copied().unwrap_or(0) as usize;
        self.values.get(start..end).unwrap_or(&[])
    }

    /// Total number of child entries across all records.
    #[must_use]
    pub const fn total_children(&self) -> usize {
        self.values.len()
    }

    /// Number of records tracked (one slot per record).
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Create an empty children index.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            offsets: vec![0],
            values: Vec::new(),
        }
    }
}

/// Extension inverted index: `extension_id → &[u32]` (record indices).
///
/// CSR layout identical to `ChildrenIndex`.  Built once at load time in a
/// single O(N) pass so `--ext rs` queries can iterate only matching records
/// instead of scanning all 25M entries.
#[derive(Clone)]
pub struct ExtensionIndex {
    /// CSR offsets — length = `max_ext_id` + 2 (one per `ext_id` + sentinel).
    offsets: Vec<u32>,
    /// Flat array of record indices, grouped by `extension_id`.
    values: Vec<u32>,
}

impl ExtensionIndex {
    /// Total heap capacity (offsets + values) in bytes.
    #[must_use]
    pub const fn heap_size_bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.values.capacity() * size_of::<u32>()
    }

    /// Build from compact records in two passes (count + scatter).
    #[must_use]
    pub fn build(records: &[CompactRecord]) -> Self {
        // Find the maximum extension_id to size the offsets array.
        let max_id = records
            .iter()
            .map(|rec| rec.extension_id)
            .max()
            .unwrap_or(0) as usize;

        // Pass 1: count records per extension_id.
        let mut counts = vec![0_u32; max_id + 1];
        for rec in records {
            if rec.name_len == 0 {
                continue;
            }
            if let Some(cnt) = counts.get_mut(rec.extension_id as usize) {
                *cnt += 1;
            }
        }

        // Prefix-sum → offsets.
        let mut offsets = Vec::with_capacity(max_id + 2);
        let mut running = 0_u32;
        for &cnt in &counts {
            offsets.push(running);
            running = running.saturating_add(cnt);
        }
        offsets.push(running);

        // Pass 2: scatter record indices into values.
        let mut values = vec![0_u32; running as usize];
        let mut write_pos = offsets.clone();
        for (idx, rec) in records.iter().enumerate() {
            if rec.name_len == 0 {
                continue;
            }
            let eid = rec.extension_id as usize;
            if let Some(pos) = write_pos.get_mut(eid)
                && let Some(slot) = values.get_mut(*pos as usize)
            {
                let idx_u32 = uffs_mft::len_to_u32(idx);
                *slot = idx_u32;
                *pos += 1;
            }
        }

        Self { offsets, values }
    }

    /// Return record indices for the given `extension_id`.
    #[must_use]
    pub fn get(&self, ext_id: u16) -> &[u32] {
        let eid = ext_id as usize;
        let start = self.offsets.get(eid).copied().unwrap_or(0) as usize;
        let end = self.offsets.get(eid + 1).copied().unwrap_or(0) as usize;
        self.values.get(start..end).unwrap_or(&[])
    }

    /// Create an empty extension index.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            offsets: vec![0],
            values: Vec::new(),
        }
    }

    /// Total number of indexed record entries.
    #[must_use]
    pub const fn total_entries(&self) -> usize {
        self.values.len()
    }
}

/// A loaded drive with compact index.
#[derive(Clone)]
pub struct DriveCompactIndex {
    /// Drive letter (e.g., 'C').
    pub letter: uffs_mft::platform::DriveLetter,
    /// Compact records — one per MFT file/directory.
    ///
    /// Backed by [`ColumnStorage`] so Phase 2b can transparently
    /// swap the heap-resident `Vec` for a memory-mapped runtime
    /// tempfile.  Read-side call sites use [`Deref<[T]>`]; mutating
    /// callers (Windows USN-patch path) go through
    /// `ColumnStorage::as_mut_vec` (internal helper).
    pub records: ColumnStorage<CompactRecord>,
    /// All filenames concatenated (UTF-8 bytes, original case).
    ///
    /// Backed by [`ColumnStorage`]; see [`Self::records`] for the
    /// rationale.
    pub names: ColumnStorage<u8>,
    /// Trigram inverted index built from folded names (char-level, `$UpCase`).
    pub trigram: TrigramIndex,
    /// CSR children index: `children.get(i)` → child indices of record i.
    pub children: ChildrenIndex,
    /// Extension inverted index: `ext_id → record indices`.
    /// Enables O(K) `--ext` queries where K = matching records, not O(N).
    pub ext_index: ExtensionIndex,
    /// NTFS `$UpCase` case folding engine for this volume.
    pub fold: uffs_text::case_fold::CaseFold,
    /// Extension name table: `ext_names[extension_id]` → lowercase extension
    /// string (e.g. `"rs"`, `"txt"`). Index 0 = no extension.
    /// Used to resolve `--ext` filter strings to `u16` IDs for O(1)
    /// per-record matching instead of per-record string parsing.
    pub ext_names: Vec<Box<str>>,
    /// Where this index was loaded from (for future refresh).
    pub source: IndexSource,
    /// `MftIndex.build_epoch` this compact index was built from.
    /// Used as a staleness check when loading from cache.
    pub source_epoch: u64,
    /// Phase 4 bloom filter over folded basenames + extensions.
    ///
    /// `None` for indices built before bloom integration landed (e.g.
    /// in-process unit-test fixtures that don't exercise the Phase 4
    /// search-skip path) and for v ≤ 8 caches before the rebuild step
    /// runs.  After [`build_compact_index`] or a v9+ cache load this
    /// is always `Some(_)`; downstream callers
    /// (`search_dispatch::bloom_pre_check`) treat `None` as "no
    /// pre-check available; fall through to the full search" which
    /// is the safe (correct-but-slower) default.
    pub bloom: Option<Bloom>,
    /// Phase 4 directory-only path trie.  Same `None`-handling
    /// rationale as [`Self::bloom`].
    pub path_trie: Option<PathTrie>,
    /// Phase 8: FRS → `compact_idx` mapping.
    ///
    /// Indexed by FRS-as-`usize`; values are the matching primary
    /// `compact_idx` in [`Self::records`], or [`u32::MAX`] for
    /// unmapped slots (system metafiles 0-15, FRS values higher
    /// than the build-time max, deleted records).
    ///
    /// Populated by [`build_compact_index`] from
    /// [`uffs_mft::MftIndex::frs_to_idx`] (which is otherwise
    /// dropped when the `MftIndex` goes out of scope).  Maintained
    /// in lock-step with [`Self::records`] by
    /// [`crate::compact_loader::apply_usn_patch`] across
    /// create / delete / rename batches: creates extend the table
    /// and assign the new compact slot, deletes mark the slot
    /// `u32::MAX`, renames leave the slot intact (only `parent_idx`
    /// + name move).
    ///
    /// **Why not stored in `MftIndex`?**  The `MftIndex` is
    /// transient — `build_compact_index` consumes it and drops it.
    /// The compact body is what survives to serve search queries
    /// and accept journal patches.  Keeping the mapping next to the
    /// records it indexes means [`crate::compact_loader::apply_usn_patch`]
    /// can patch the body in place without touching the MFT.
    ///
    /// **Backward compatibility**: caches written before v10
    /// (Phase 8) didn't persist this mapping; for those, the field
    /// loads as an empty `Vec` and the surgical patch path
    /// silently degrades to the full-reload fallback.  See the
    /// v9 → v10 cache format bump in `compact_cache::COMPACT_VERSION`.
    pub frs_to_compact: Vec<u32>,
}

/// Per-component heap footprint of a [`DriveCompactIndex`].
#[derive(Debug, Clone)]
pub struct HeapReport {
    /// `records: ColumnStorage<CompactRecord>` capacity in bytes.
    /// Mmap-backed columns (Phase 2b) report `len * sizeof(T)`
    /// since the kernel-mapped pages have no extra slack.
    pub records: usize,
    /// `names: ColumnStorage<u8>` capacity in bytes.
    pub names: usize,
    /// `TrigramIndex` total heap (keys + offsets + values).
    pub trigram: usize,
    /// `ChildrenIndex` total heap (offsets + values).
    pub children: usize,
    /// `ExtensionIndex` total heap (offsets + values).
    pub ext_index: usize,
    /// `ext_names: Vec<Box<str>>` heap (Vec + string data).
    pub ext_names: usize,
    /// `frs_to_compact: Vec<u32>` capacity in bytes (Phase 8 —
    /// `~max_frs * 4` bytes; ~40 MB on a 7M-record drive with
    /// max FRS ≈ 10M).
    pub frs_to_compact: usize,
    /// Sum of all components.
    pub total: usize,
}

impl AsRef<Self> for DriveCompactIndex {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl DriveCompactIndex {
    /// Compute the total heap footprint of this index (in bytes).
    ///
    /// This measures *capacity* (what the allocator reserved), not *len*
    /// (what we're using).  The gap between the two is what `shrink_to_fit`
    /// reclaims.  Use this after loading to verify memory usage.
    #[must_use]
    pub fn heap_size_bytes(&self) -> HeapReport {
        let records = self.records.capacity() * size_of::<CompactRecord>();
        let names = self.names.capacity();
        let trigram = self.trigram.heap_size_bytes();
        let children = self.children.heap_size_bytes();
        let ext_index = self.ext_index.heap_size_bytes();
        let ext_names_data: usize = self.ext_names.iter().map(|en| en.len()).sum();
        let ext_names_vec = self.ext_names.capacity() * size_of::<Box<str>>();
        let ext_names = ext_names_data + ext_names_vec;
        let frs_to_compact = self.frs_to_compact.capacity() * size_of::<u32>();
        HeapReport {
            records,
            names,
            trigram,
            children,
            ext_index,
            ext_names,
            frs_to_compact,
            total: records + names + trigram + children + ext_index + ext_names + frs_to_compact,
        }
    }

    /// Log the heap report at `info` level.
    pub fn log_heap_report(&self) {
        let hr = self.heap_size_bytes();
        let mb = |bytes: usize| bytes / (1024 * 1024);
        tracing::info!(
            drive = %self.letter,
            records_count = self.records.len(),
            records_mb = mb(hr.records),
            names_mb = mb(hr.names),
            trigram_mb = mb(hr.trigram),
            children_mb = mb(hr.children),
            ext_index_mb = mb(hr.ext_index),
            ext_names_mb = mb(hr.ext_names),
            frs_to_compact_mb = mb(hr.frs_to_compact),
            total_mb = mb(hr.total),
            "[HEAP] {}: rec={} names={} tri={} ch={} ext={} f2c={} | total={} MB",
            self.letter,
            mb(hr.records), mb(hr.names), mb(hr.trigram),
            mb(hr.children), mb(hr.ext_index), mb(hr.frs_to_compact),
            mb(hr.total),
        );
    }

    /// Resolve extension filter strings to their `u16` IDs on this drive.
    ///
    /// Returns a sorted, deduplicated `Vec<u16>` of matching IDs.
    /// Extensions not found on this drive are silently ignored.
    ///
    /// The lookup is a linear scan of `ext_names` (~500–2000 short strings),
    /// which takes < 1 µs.  This runs **once per search per drive**, not per
    /// record.
    #[must_use]
    pub(crate) fn resolve_ext_ids(&self, extensions: &[String]) -> Vec<u16> {
        let mut ids = Vec::with_capacity(extensions.len());
        for ext in extensions {
            let normalized = ext.trim().trim_start_matches('.').to_lowercase();
            if normalized.is_empty() {
                continue;
            }
            for (ext_id, name) in (0_u16..).zip(self.ext_names.iter()) {
                if name.as_ref() == normalized {
                    ids.push(ext_id);
                    break;
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// Expand alternate data streams (ADS) for a single record, producing the
/// name × stream cross product as extra `CompactRecord` entries.
#[expect(
    clippy::single_call_fn,
    reason = "Extracted to keep expand_links_and_ads under the too_many_lines limit"
)]
fn expand_ads_streams(
    index: &MftIndex,
    record: &uffs_mft::index::FileRecord,
    resolve_parent: &dyn Fn(uffs_mft::ParentFrs, uffs_mft::Frs) -> u32,
    names: &mut Vec<u8>,
    extra: &mut Vec<CompactRecord>,
) {
    // Collect all names for this record (primary + hardlinks).
    let mut all_names: Vec<(&str, u32)> = Vec::new();
    let primary_name = index.get_name(record.first_name.name);
    if !primary_name.is_empty() {
        let pid = resolve_parent(record.first_name.parent_frs, record.frs);
        all_names.push((primary_name, pid));
    }
    if record.name_count > 1 {
        let mut le = record.first_name.next_entry;
        while le != uffs_mft::NO_ENTRY {
            let Some(lnk) = index.links.get(le as usize) else {
                break;
            };
            let ln = index.get_name(lnk.name);
            if !ln.is_empty() {
                let lp = resolve_parent(lnk.parent_frs, record.frs);
                all_names.push((ln, lp));
            }
            le = lnk.next_entry;
        }
    }

    // Walk output streams (skip default $DATA at head of chain).
    let mut se = record.first_stream.next_entry;
    while se != uffs_mft::NO_ENTRY {
        let Some(stream) = index.streams.get(se as usize) else {
            break;
        };
        if stream.is_output_stream() {
            let sn = index.stream_name(stream);
            if !sn.is_empty() {
                for &(base_name, parent_idx) in &all_names {
                    let combined = format!("{base_name}:{sn}");
                    let name_offset = uffs_mft::len_to_u32(names.len());
                    let name_len = uffs_mft::len_to_u16(combined.len());
                    names.extend_from_slice(combined.as_bytes());

                    extra.push(CompactRecord {
                        size: stream.size.length,
                        allocated: stream.size.allocated,
                        treesize: 0,
                        tree_allocated: 0,
                        created: record.stdinfo.created,
                        modified: record.stdinfo.modified,
                        accessed: record.stdinfo.accessed,
                        name_offset,
                        flags: record.stdinfo.flags,
                        parent_idx,
                        descendants: 0,
                        name_len,
                        extension_id: 0,
                        path_len: 0,
                        name_first_byte: combined.as_bytes().first().copied().unwrap_or(0),
                        _pad: [0; 1],
                    });
                }
            }
        }
        se = stream.next_entry;
    }
}

/// Resolve a typed `ParentFrs` (vs an own typed `Frs`) into a compact-record
/// index, returning `u32::MAX` for the "no real parent" cases (self-reference,
/// `NO_ENTRY` sentinel, or root).
///
/// Extracted as a free helper so the typed `ParentFrs`/`Frs` signature is
/// enforced at every call site AND so `build_compact_index` stays under
/// the clippy `too_many_lines` budget.
#[expect(
    clippy::single_call_fn,
    reason = "Wrapped by a closure in build_compact_index; kept free-standing \
              for clippy::too_many_lines budget headroom"
)]
fn resolve_parent_compact_idx(
    index: &MftIndex,
    parent_frs: uffs_mft::ParentFrs,
    own_frs: uffs_mft::Frs,
) -> u32 {
    let parent = parent_frs.as_frs();
    if parent == own_frs || parent_frs.raw() == u64::from(uffs_mft::NO_ENTRY) || parent.is_root() {
        return u32::MAX;
    }
    let parent_usize = uffs_mft::frs_to_usize(parent.raw());
    index
        .frs_to_idx
        .get(parent_usize)
        .copied()
        .filter(|&idx| idx != uffs_mft::NO_ENTRY)
        .unwrap_or(u32::MAX)
}

/// Expand hardlinks and ADS into additional `CompactRecord` entries.
///
/// Phase 2 (hardlinks): for each valid record with `name_count > 1`, walks the
/// link chain and creates additional records with alternate name/parent.
///
/// Phase 3 (ADS): delegates to [`expand_ads_streams`] for each valid record
/// with `stream_count > 1`.
#[expect(
    clippy::single_call_fn,
    reason = "Extracted to keep build_compact_index under the too_many_lines limit"
)]
fn expand_links_and_ads(
    index: &MftIndex,
    resolver: &uffs_mft::index::PathResolver,
    resolve_parent: &dyn Fn(uffs_mft::ParentFrs, uffs_mft::Frs) -> u32,
    names: &mut Vec<u8>,
) -> Vec<CompactRecord> {
    let mut extra: Vec<CompactRecord> = Vec::new();

    for (idx, record) in index.records.iter().enumerate() {
        if !resolver.is_valid_idx(idx) {
            continue;
        }

        // Phase 2: hardlink expansion.
        if record.name_count > 1 {
            let mut link_entry = record.first_name.next_entry;
            while link_entry != uffs_mft::NO_ENTRY {
                let Some(link) = index.links.get(link_entry as usize) else {
                    break;
                };
                let link_parent = resolve_parent(link.parent_frs, record.frs);
                extra.push(CompactRecord {
                    size: record.first_stream.size.length,
                    allocated: record.first_stream.size.allocated,
                    treesize: record.treesize,
                    tree_allocated: record.tree_allocated,
                    created: record.stdinfo.created,
                    modified: record.stdinfo.modified,
                    accessed: record.stdinfo.accessed,
                    name_offset: link.name.offset,
                    flags: record.stdinfo.flags,
                    parent_idx: link_parent,
                    descendants: record.descendants,
                    name_len: link.name.length(),
                    extension_id: link.name.extension_id(),
                    path_len: 0,
                    name_first_byte: names.get(link.name.offset as usize).copied().unwrap_or(0),
                    _pad: [0; 1],
                });
                link_entry = link.next_entry;
            }
        }

        // Phase 3: ADS expansion (name × stream cross product).
        if record.stream_count > 1 {
            expand_ads_streams(index, record, resolve_parent, names, &mut extra);
        }
    }
    extra
}

/// Compute `path_len` (in **characters**, not bytes) for every record
/// via top-down BFS.
///
/// Root entries (`parent_idx == u32::MAX`) get
/// `path_len = 2 + 1 + name_chars` (e.g. `"C:\" + name`), and children
/// accumulate `parent.path_len + 1 (separator) + name_chars`.
/// Saturates at `u16::MAX` (65 535) for extremely deep paths.
///
/// Character counting matches `str::chars().count()` so the precomputed
/// value agrees with the display-row path-length filter.
pub(crate) fn compute_path_lengths(
    records: &mut [CompactRecord],
    names: &[u8],
    drive_letter: uffs_mft::platform::DriveLetter,
) {
    // Drive prefix in characters: the letter (1 char) + colon (1 char) = 2.
    // `DriveLetter` is ASCII A–Z by construction (validated in
    // `DriveLetter::parse`), so the previous runtime `debug_assert!`
    // is now a tautology and was removed.  The arithmetic only cares
    // about "1 letter char + 1 colon".
    let _: uffs_mft::platform::DriveLetter = drive_letter;
    let drive_prefix_chars: u32 = 1 /* letter */ + 1 /* ':' */;

    // Build forward adjacency list (parent → children) for top-down BFS.
    let record_count = records.len();
    let mut children_of: Vec<Vec<u32>> = vec![Vec::new(); record_count];
    let mut roots: Vec<u32> = Vec::new();

    for (idx, rec) in records.iter().enumerate() {
        let pi = rec.parent_idx;
        if pi == u32::MAX {
            roots.push(uffs_mft::len_to_u32(idx));
        } else if let Some(siblings) = children_of.get_mut(pi as usize) {
            siblings.push(uffs_mft::len_to_u32(idx));
        }
    }

    // BFS from roots.
    let mut queue = alloc::collections::VecDeque::with_capacity(roots.len());
    for &root in &roots {
        let Some(rec) = records.get(root as usize) else {
            continue;
        };
        let name_chars = name_char_count(rec, names);
        let pl = if name_chars == 0 {
            // Drive root directory: "C:\"
            drive_prefix_chars + 1
        } else {
            // Top-level file/dir: "C:\<name>"
            drive_prefix_chars + 1 + name_chars
        };
        if let Some(slot) = records.get_mut(root as usize) {
            slot.path_len = uffs_mft::len_to_u16(pl as usize);
        }
        queue.push_back(root);
    }

    while let Some(idx) = queue.pop_front() {
        let parent_pl = records
            .get(idx as usize)
            .map_or(0, |rec| u32::from(rec.path_len));
        let children: Vec<u32> = children_of
            .get(idx as usize)
            .map_or_else(Vec::new, Clone::clone);
        for &child in &children {
            let child_chars = records
                .get(child as usize)
                .map_or(0, |rec| name_char_count(rec, names));
            // path = parent_path + "\" + name
            let pl = parent_pl.saturating_add(1).saturating_add(child_chars);
            if let Some(slot) = records.get_mut(child as usize) {
                slot.path_len = uffs_mft::len_to_u16(pl as usize);
            }
            queue.push_back(child);
        }
    }
}

/// Count the number of Unicode characters in a record's filename.
///
/// Falls back to `name_len` (byte count) if the name slice is not valid
/// UTF-8 — this is correct for ASCII names and a safe upper bound
/// otherwise.
fn name_char_count(rec: &CompactRecord, names: &[u8]) -> u32 {
    let start = rec.name_offset as usize;
    let end = start + rec.name_len as usize;
    names
        .get(start..end)
        .and_then(|slice| core::str::from_utf8(slice).ok())
        .map_or_else(
            || u32::from(rec.name_len),
            |name| uffs_mft::len_to_u32(name.chars().count()),
        )
}

/// Build a `DriveCompactIndex` from a loaded `MftIndex`.
///
/// Returns `(DriveCompactIndex, compact_build_ms, trigram_build_ms)`.
#[must_use]
pub fn build_compact_index(
    drive_letter: uffs_mft::platform::DriveLetter,
    index: &MftIndex,
) -> (DriveCompactIndex, u128, u128) {
    use uffs_mft::index::PathResolver;

    let compact_start = Instant::now();

    // Build path resolver to determine which records are valid.
    // This filters out system metafiles (FRS 0-15 except root) and
    // propagates invalidity to descendants (e.g., $Extend children).
    let resolver = PathResolver::build(index, false);

    // Closure wraps the free helper `resolve_parent_compact_idx` so the
    // typed `ParentFrs`/`Frs` signature is enforced at every call site
    // (own↔parent swap becomes a compile error).  Keeping the helper
    // free-standing also keeps `build_compact_index` under the
    // clippy::too_many_lines budget.
    let resolve_parent = |parent_frs: uffs_mft::ParentFrs, own_frs: uffs_mft::Frs| -> u32 {
        resolve_parent_compact_idx(index, parent_frs, own_frs)
    };

    // Phase 1: build primary compact records (parallel).
    let mut records: Vec<CompactRecord> = index
        .records
        .par_iter()
        .enumerate()
        .map(|(idx, record)| {
            // Skip invalid records (system metafiles + descendants).
            if !resolver.is_valid_idx(idx) {
                return CompactRecord::default();
            }

            let name_ref = &record.first_name.name;
            let parent_idx = resolve_parent(record.first_name.parent_frs, record.frs);

            CompactRecord {
                size: record.first_stream.size.length,
                allocated: record.first_stream.size.allocated,
                treesize: record.treesize,
                tree_allocated: record.tree_allocated,
                created: record.stdinfo.created,
                modified: record.stdinfo.modified,
                accessed: record.stdinfo.accessed,
                name_offset: name_ref.offset,
                flags: record.stdinfo.flags,
                parent_idx,
                descendants: record.descendants,
                name_len: name_ref.length(),
                extension_id: name_ref.extension_id(),
                path_len: 0,
                name_first_byte: index
                    .names
                    .get(name_ref.offset as usize)
                    .copied()
                    .unwrap_or(0),
                _pad: [0; 1],
            }
        })
        .collect();

    // Phase 2+3: expand hardlinks and ADS (sequential — rare, <1% of records).
    let mut names = index.names.clone();
    let expanded = expand_links_and_ads(index, &resolver, &resolve_parent, &mut names);
    records.extend(expanded);

    // Phase 4: compute path_len (in characters) for every record via
    // top-down BFS.  path_len = char count of "C:\dir\name".
    compute_path_lengths(&mut records, &names, drive_letter);

    let compact_elapsed = compact_start.elapsed().as_millis();

    // Try live $UpCase from the NTFS volume; fall back to compiled-in default.
    let fold = resolve_case_fold(drive_letter);

    let tri_start = Instant::now();
    let trigram = TrigramIndex::build(&records, &names, fold);
    let tri_elapsed = tri_start.elapsed().as_millis();

    // Build children CSR index from parent_idx (two-pass: count + scatter).
    let children = ChildrenIndex::build(&records);

    // Copy extension name table from MftIndex (Arc<str> → Box<str>).
    let mut ext_names: Vec<Box<str>> = index
        .extensions
        .names
        .iter()
        .map(|arc| Box::from(arc.as_ref()))
        .collect();

    let ext_t0 = Instant::now();
    let ext_index = ExtensionIndex::build(&records);
    let ext_build_ms = ext_t0.elapsed().as_millis();
    tracing::info!(
        drive = %drive_letter,
        entries = ext_index.total_entries(),
        build_ms = ext_build_ms,
        "ExtensionIndex built"
    );

    shrink_compact_vecs(drive_letter, &mut records, &mut names, &mut ext_names);

    // Phase 8: clone the FRS → mft_idx mapping off the transient
    // `MftIndex` before it goes out of scope.  In the primary
    // `build_compact_index` path compact_idx == mft_idx (records
    // are produced 1:1 by `index.records.par_iter().enumerate()`),
    // so `frs_to_idx` is exactly the FRS → compact_idx mapping the
    // surgical-patch path needs.  Hardlink / ADS-expanded records
    // append at the END with the same FRS but higher compact_idx;
    // those secondary slots are not addressable from journal events
    // (USN events reference primary FRS) so the primary mapping is
    // sufficient.  `uffs_mft::NO_ENTRY == u32::MAX` matches the
    // sentinel `frs_to_compact` uses for unmapped slots.
    let mut compact_index = DriveCompactIndex {
        letter: drive_letter,
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram,
        children,
        ext_index,
        fold,
        ext_names,
        source: IndexSource::MftFile(std::path::PathBuf::from(format!("{drive_letter}:"))),
        source_epoch: index.build_epoch,
        bloom: None,
        path_trie: None,
        frs_to_compact: index.frs_to_idx.clone(),
    };

    // Phase 4: populate bloom + path_trie from the freshly-built
    // index.  These are needed for the search-skip pre-check
    // (Commit F) and serialised into the v9+ cache (Commit D).
    let bloom = compact_index.build_bloom();
    let path_trie = compact_index.build_path_trie();
    compact_index.bloom = Some(bloom);
    compact_index.path_trie = Some(path_trie);

    (compact_index, compact_elapsed, tri_elapsed)
}

/// Shrink all growable Vecs to exact fit after compact index build.
///
/// Reclaims capacity slack from the doubling growth strategy used during
/// construction.  Saves ~500 MB across 7 drives.
fn shrink_compact_vecs(
    drive_letter: uffs_mft::platform::DriveLetter,
    records: &mut Vec<CompactRecord>,
    names: &mut Vec<u8>,
    ext_names: &mut Vec<Box<str>>,
) {
    let pre = records.capacity() * size_of::<CompactRecord>() + names.capacity();
    records.shrink_to_fit();
    names.shrink_to_fit();
    ext_names.shrink_to_fit();
    let post = records.capacity() * size_of::<CompactRecord>() + names.capacity();
    let reclaimed_mb = pre.saturating_sub(post) / (1024 * 1024);
    if reclaimed_mb > 0 {
        tracing::info!(
            drive = %drive_letter,
            reclaimed_mb,
            "shrink_to_fit reclaimed memory"
        );
    }
}

/// Cache TTL in seconds (4 hours — same as Windows CLI).
///
/// USN Journal handles incremental freshness; this is a safety-net full rescan.
pub(crate) const INDEX_TTL_SECONDS: u64 = 14400;

// ── Live $UpCase resolution ──────────────────────────────────────────

/// Try to read the live `$UpCase` table from the NTFS volume for
/// `drive_letter`. On success, log the result at `INFO` and any diffs
/// from the compiled-in default at `WARN`. On failure, log at `WARN`
/// and fall back to [`CaseFold::default_table()`].
pub(crate) fn resolve_case_fold(
    drive_letter: uffs_mft::platform::DriveLetter,
) -> uffs_text::case_fold::CaseFold {
    let live_table = match uffs_mft::platform::upcase::read_upcase_table(drive_letter) {
        Ok(table) => table,
        Err(err) => {
            tracing::warn!(
                drive = %drive_letter,
                error = %err,
                "$UpCase live read failed — falling back to compiled-in default table"
            );
            return uffs_text::case_fold::CaseFold::default_table();
        }
    };

    // Leak the box to get a `&'static [u16]` for CaseFold::from_ntfs.
    let live_fold = uffs_text::case_fold::CaseFold::from_ntfs(Box::leak(live_table));
    log_upcase_comparison(drive_letter, &live_fold);
    live_fold
}

/// Log the comparison between live and compiled-in `$UpCase` tables.
fn log_upcase_comparison(
    drive_letter: uffs_mft::platform::DriveLetter,
    live_fold: &uffs_text::case_fold::CaseFold,
) {
    let default = uffs_text::case_fold::CaseFold::default_table();
    let diffs = default.diff(live_fold);

    if diffs.is_empty() {
        tracing::info!(
            drive = %drive_letter,
            "$UpCase loaded from live volume — identical to compiled-in default"
        );
        return;
    }

    tracing::info!(
        drive = %drive_letter,
        diff_count = diffs.len(),
        "$UpCase loaded from live volume — differs from compiled-in default"
    );
    for diff in &diffs {
        tracing::warn!(
            drive = %drive_letter,
            codepoint = format_args!("U+{:04X}", diff.codepoint),
            default = format_args!("U+{:04X}", diff.default_maps_to),
            live = format_args!("U+{:04X}", diff.live_maps_to),
            "$UpCase diff"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
// REGRESSION TESTS — Search Pipeline Parity Guards
//
// These tests protect critical behaviors that broke during the v0.4.30
// refactor attempt.  They run on synthetic data (no Windows/MFT needed).
// See `docs/architecture/2026_03_30_04_12_SEARCH_PIPELINE_REGRESSION_ANALYSIS.
// md` ════════════════════════════════════════════════════════════════════════
#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
