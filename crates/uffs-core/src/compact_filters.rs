// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 4 filter builders for [`DriveCompactIndex`]
//! (`build_bloom` + `build_path_trie`).
//!
//! Lives in a sibling module rather than directly inside `compact.rs`
//! because that file is a permanent file-size-policy exception (see
//! `scripts/ci/file_size_exceptions.txt`); inlining ~150 LOC of new
//! method + tests would invalidate its "only 13 over limit"
//! rationale.  Multiple `impl DriveCompactIndex { }` blocks across
//! files are a normal Rust pattern: the linker unifies them.
//!
//! ## Plan tasks covered
//!
//! - **4.3** — `DriveCompactIndex::build_bloom` inserts every record's
//!   `$UpCase`-folded basename plus every entry from `ext_names`.  Plan note
//!   "Not full paths" is enforced structurally: this method never sees a path;
//!   only basenames and extensions cross the bloom.
//! - **4.3** (cont.) — `DriveCompactIndex::build_path_trie` is a thin shim over
//!   [`PathTrie::build`] passing the records and names slices.
//!
//! ## Sizing rationale
//!
//! `build_bloom` sizes the filter for `records.len() +
//! ext_names.len()` items at the workspace's standard 1 % FPR
//! target.  For a 7 M-record drive with ~2 K extensions, that's
//! ~7 M items → ~8.5 MB bloom.  Phase 4 Commit G measures this on
//! `fixture_large` to confirm the per-drive PARKED budget; any
//! tightening (smaller bloom, sampling) lands in Phase 6 (adaptive
//! TTL + per-drive sizing).

use crate::bloom::Bloom;
use crate::compact::DriveCompactIndex;
use crate::path_trie::PathTrie;

/// Standard false-positive rate target for shard blooms.
///
/// Pinned to the tiering-plan §6.2 envelope (1 %).  Phase 6 may
/// promote this to a per-drive policy knob; for Phase 4 every shard
/// uses the same target so the headline "≤ 50 MB on a 7-drive idle
/// box" math is reproducible.
pub const SHARD_BLOOM_TARGET_FPR: f64 = 0.01;

impl DriveCompactIndex {
    /// Build a bloom filter populated with every record's
    /// `$UpCase`-folded basename plus every interned extension.
    ///
    /// O(N) over `records.len()`.  Allocates a fresh per-drive
    /// fold buffer; reuses it across every record so the only
    /// per-record work is the codepoint walk + `bloom.insert`.
    ///
    /// **Not** full paths: by design the bloom only answers
    /// "does this drive contain a basename / extension matching
    /// X?" — path-prefix queries go through [`PathTrie`] instead.
    /// The asymmetry keeps the bloom small (the same string
    /// indexed at different paths only contributes one entry).
    ///
    /// Records whose name slice is invalid UTF-8 (rare; NTFS
    /// guarantees UTF-16 names but a USN-patch fragment could
    /// theoretically slip through) are skipped — the bloom errs on
    /// the side of "miss" rather than indexing arbitrary bytes
    /// that wouldn't match the same string after fold-on-query.
    #[must_use]
    pub fn build_bloom(&self) -> Bloom {
        let n_items = self
            .records
            .len()
            .saturating_add(self.ext_names.len())
            .max(1);
        let mut bloom = Bloom::with_capacity_and_fpr(n_items, SHARD_BLOOM_TARGET_FPR);

        // Reuse a single fold buffer across every record — fold_into
        // clears it on entry, so we just declare it once.
        let mut fold_buf: Vec<u8> = Vec::with_capacity(64);

        for record in &self.records {
            let start = record.name_offset as usize;
            let end = start.saturating_add(record.name_len as usize);
            let Some(raw_bytes) = self.names.get(start..end) else {
                continue;
            };
            let Ok(name_str) = core::str::from_utf8(raw_bytes) else {
                continue;
            };
            let folded = self.fold.fold_into(name_str, &mut fold_buf);
            bloom.insert(folded.as_bytes());
        }

        // Extensions are stored in `ext_names` as already-lowercase
        // strings (see `extension_id` doc on `CompactRecord`).
        // Insert them as-is — at query time the search dispatch
        // will lowercase the user's `--ext` argument before probing
        // so the case contract matches.
        for ext in &self.ext_names {
            let bytes = ext.as_bytes();
            if !bytes.is_empty() {
                bloom.insert(bytes);
            }
        }

        bloom
    }

    /// Build a directory-only path trie over this drive's
    /// directories.
    ///
    /// Thin shim over [`PathTrie::build`]; passes the records and
    /// names slices through.  See [`PathTrie`] module docs for the
    /// memory layout and build-cost analysis.
    #[must_use]
    pub fn build_path_trie(&self) -> PathTrie {
        PathTrie::build(&self.records, &self.names)
    }

    /// Extract a [`ParkedBody`](crate::compact_cache::parked::ParkedBody)
    /// view of this drive — bloom + trie + epoch + fold — for the
    /// Warm → Parked tier transition.
    ///
    /// Phase 4 Commit F.  Reuses the in-memory `bloom` / `path_trie`
    /// fields when present (the common case after Phase 4 Commit C
    /// landed); falls back to [`Self::build_bloom`] /
    /// [`Self::build_path_trie`] for indices constructed before the
    /// Phase 4 wiring (or for legacy v ≤ 8 caches whose loader
    /// rebuilds the filters on the fly — see
    /// [`crate::compact_cache::deserialize_compact`]).
    ///
    /// Clones the bloom and trie because `ParkedBody` must own its
    /// data (the source `DriveCompactIndex` is dropped right after
    /// the transition).  The clone is cheap relative to a rebuild —
    /// `Bloom::clone` is a single `Vec<u64>::clone` (≈ 8.75 MB at
    /// the 7 M-record / 1 % FPR sizing) and `PathTrie::clone` is
    /// four `Vec<…>::clone` calls (≈ 5 MB total).  In exchange the
    /// transition completes without scanning records or running the
    /// fold table again.
    #[must_use]
    pub fn to_parked_body(&self) -> crate::compact_cache::ParkedBody {
        let bloom = self.bloom.clone().unwrap_or_else(|| self.build_bloom());
        let path_trie = self
            .path_trie
            .clone()
            .unwrap_or_else(|| self.build_path_trie());
        crate::compact_cache::ParkedBody {
            letter: self.letter,
            source_epoch: self.source_epoch,
            bloom,
            path_trie,
            fold: self.fold,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::compact::{ChildrenIndex, CompactRecord, ExtensionIndex, IndexSource};
    use crate::compact_storage::ColumnStorage;
    use crate::trigram::TrigramIndex;

    /// Build a minimal `DriveCompactIndex` with three records:
    /// the root directory `C` (idx 0), a file `Cargo.toml` under
    /// it (idx 1, `ext_id`=1 → "toml"), and a directory `src`
    /// under it (idx 2).
    fn build_filter_test_drive() -> DriveCompactIndex {
        // Names blob layout:
        //   "C"          [0..1]   → root dir
        //   "Cargo.toml" [1..11]  → file
        //   "src"        [11..14] → dir
        let names = b"CCargo.tomlsrc".to_vec();
        let records = vec![
            CompactRecord {
                name_offset: 0,
                flags: 0x0010, // directory
                parent_idx: u32::MAX,
                name_len: 1,
                name_first_byte: b'C',
                ..CompactRecord::default()
            },
            CompactRecord {
                name_offset: 1,
                parent_idx: 0,
                name_len: 10,
                extension_id: 1,
                name_first_byte: b'C',
                ..CompactRecord::default()
            },
            CompactRecord {
                name_offset: 11,
                flags: 0x0010,
                parent_idx: 0,
                name_len: 3,
                name_first_byte: b's',
                ..CompactRecord::default()
            },
        ];
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let trigram = TrigramIndex::build(&records, &names, fold);
        let children = ChildrenIndex::build(&records);
        let ext_index = ExtensionIndex::build(&records);
        DriveCompactIndex {
            letter: 'C',
            records: ColumnStorage::from_vec(records),
            names: ColumnStorage::from_vec(names),
            trigram,
            children,
            ext_index,
            fold,
            ext_names: vec![Box::from(""), Box::from("toml")],
            source: IndexSource::MftFile(PathBuf::from("C:")),
            source_epoch: 1,
            bloom: None,
            path_trie: None,
        }
    }

    /// Smoke: bloom contains the folded form of every basename
    /// inserted plus every non-empty extension.
    #[test]
    fn build_bloom_contains_every_inserted_basename_and_extension() {
        let drive = build_filter_test_drive();
        let bloom = drive.build_bloom();
        let fold = drive.fold;
        let mut buf: Vec<u8> = Vec::new();

        // Every basename folded.
        for expected in ["C", "Cargo.toml", "src"] {
            buf.clear();
            let folded = fold.fold_into(expected, &mut buf);
            assert!(
                bloom.contains(folded.as_bytes()),
                "bloom missed folded basename {expected:?} → {folded:?}"
            );
        }

        // Every non-empty extension.
        assert!(bloom.contains(b"toml"));
    }

    /// Bloom probabilistically rejects strings the drive doesn't
    /// contain.  At this load (3 items) the FPR is effectively zero;
    /// every novel query must miss.
    #[test]
    fn build_bloom_rejects_disjoint_strings() {
        let drive = build_filter_test_drive();
        let bloom = drive.build_bloom();
        let fold = drive.fold;
        let mut buf: Vec<u8> = Vec::new();

        for novel in ["Plumbus", "qux", "frobnicate"] {
            buf.clear();
            let folded = fold.fold_into(novel, &mut buf);
            assert!(
                !bloom.contains(folded.as_bytes()),
                "bloom false-positive on novel {novel:?} → {folded:?}"
            );
        }
    }

    /// Case-insensitive query: `bloom.contains(fold("CARGO.TOML"))`
    /// returns `true` because the fold is symmetric — both the
    /// indexer and the query side run the same `$UpCase` table.
    #[test]
    fn build_bloom_matches_case_insensitive_via_fold() {
        let drive = build_filter_test_drive();
        let bloom = drive.build_bloom();
        let fold = drive.fold;
        let mut buf: Vec<u8> = Vec::new();

        let folded_upper = fold.fold_into("CARGO.TOML", &mut buf);
        assert!(bloom.contains(folded_upper.as_bytes()));
        buf.clear();

        let folded_lower = fold.fold_into("cargo.toml", &mut buf);
        assert!(bloom.contains(folded_lower.as_bytes()));
        buf.clear();

        let folded_mixed = fold.fold_into("CaRgO.tOmL", &mut buf);
        assert!(bloom.contains(folded_mixed.as_bytes()));
    }

    /// `build_path_trie` integrates with the records' parent-chain.
    /// On the test fixture there are two trie nodes — root `C` at
    /// index 0 and `src` at index 1, with `src` parented to `C`.
    /// The file `Cargo.toml` is filtered out because it's not a
    /// directory.
    #[test]
    fn build_path_trie_indexes_directories_only() {
        let drive = build_filter_test_drive();
        let trie = drive.build_path_trie();

        // Two directories (`C` + `src`); the `Cargo.toml` file is
        // filtered out.
        assert_eq!(trie.len(), 2);
        assert_eq!(trie.roots(), vec![0_u32]);
        assert_eq!(trie.name_of(0), Some(b"C" as &[u8]));
        assert_eq!(trie.name_of(1), Some(b"src" as &[u8]));
        assert_eq!(trie.parent_of(1), Some(0));

        // `lookup_path` walks the trie correctly.
        assert_eq!(trie.lookup_path(&[b"C"]), Some(0));
        assert_eq!(trie.lookup_path(&[b"C", b"src"]), Some(1));
        assert!(trie.lookup_path(&[b"C", b"missing"]).is_none());
    }

    /// Empty drive (no records) still builds a non-empty bloom (the
    /// constructor enforces `nbits ≥ 64`) and an empty trie.
    /// Defensive test for the COLD → PARKED edge case where a
    /// shard's records were dropped but `build_bloom` is asked to
    /// produce a placeholder.
    #[test]
    fn empty_drive_builds_empty_filters_safely() {
        let names: Vec<u8> = Vec::new();
        let records: Vec<CompactRecord> = Vec::new();
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let trigram = TrigramIndex::build(&records, &names, fold);
        let children = ChildrenIndex::build(&records);
        let ext_index = ExtensionIndex::build(&records);
        let drive = DriveCompactIndex {
            letter: 'X',
            records: ColumnStorage::from_vec(records),
            names: ColumnStorage::from_vec(names),
            trigram,
            children,
            ext_index,
            fold,
            ext_names: vec![Box::from("")],
            source: IndexSource::MftFile(PathBuf::from("X:")),
            source_epoch: 0,
            bloom: None,
            path_trie: None,
        };

        let bloom = drive.build_bloom();
        assert!(bloom.nbits() >= 64);
        // No items inserted → no key can match.
        assert!(!bloom.contains(b"anything"));

        let trie = drive.build_path_trie();
        assert!(trie.is_empty());
    }
}
