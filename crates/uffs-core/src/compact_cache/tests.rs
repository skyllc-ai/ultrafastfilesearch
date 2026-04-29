// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`super`] (`compact_cache`).
//!
//! Test strategy:
//!
//! * **Round-trips** — exercise both the heap-backed
//!   [`super::deserialize_compact`] and the runtime-mmap-backed
//!   [`super::deserialize_compact_into_runtime`] over the same serialized bytes
//!   and assert byte-equal observable state.
//! * **Backwards compat** — patch a v6 cache to v5 to confirm the trigram
//!   rebuild fallback still kicks in.
//! * **Header invariants** — current [`super::COMPACT_VERSION`] is stamped into
//!   byte 8/9 of every serialized payload.
//! * **Error paths** — v1 caches and truncated bytes are rejected on both code
//!   paths.
//!
//! Extracted from `compact_cache.rs` into a sibling submodule so the
//! lib file stays close to the 800 LOC soft limit instead of leaning
//! on the file-size exception (which was originally granted at "only
//! 26 over limit"; growing tests would invalidate that rationale).

use super::*;

/// Build a minimal `DriveCompactIndex` with 3 records for testing.
fn make_test_index() -> DriveCompactIndex {
    let names = b"foobarbaz".to_vec(); // "foo" [0..3], "bar" [3..6], "baz" [6..9]
    let records = vec![
        CompactRecord {
            name_offset: 0,
            flags: 0x0010, // directory
            parent_idx: u32::MAX,
            name_len: 3,
            name_first_byte: b'f',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 3,
            parent_idx: 0,
            name_len: 3,
            name_first_byte: b'b',
            ..CompactRecord::default()
        },
        CompactRecord {
            name_offset: 6,
            parent_idx: 0,
            name_len: 3,
            name_first_byte: b'b',
            ..CompactRecord::default()
        },
    ];
    let fold = uffs_text::case_fold::CaseFold::default_table();
    let trigram = TrigramIndex::build(&records, &names, fold);
    let children = ChildrenIndex::build(&records);
    let ext_index = ExtensionIndex::build(&records);
    DriveCompactIndex {
        letter: 'T',
        records: ColumnStorage::from_vec(records),
        names: ColumnStorage::from_vec(names),
        trigram,
        children,
        ext_index,
        fold,
        ext_names: vec![Box::from("")],
        source: IndexSource::MftFile(PathBuf::from("T:")),
        source_epoch: 42,
        bloom: None,
        path_trie: None,
    }
}

#[test]
fn v6_round_trip_preserves_trigram() {
    let index = make_test_index();
    let (tri_keys, tri_offsets, tri_values) = index.trigram.as_csr();
    let original_key_count = tri_keys.len();
    assert!(original_key_count > 0, "test index should have trigrams");

    let serialized = serialize_compact(&index);
    let (loaded, tri_ms) = deserialize_compact(&serialized, 'T').unwrap();

    // Trigram loaded from disk — should be fast (< 10ms on any hardware).
    assert!(
        tri_ms < 500,
        "trigram took {tri_ms}ms — should be near-zero for cached CSR"
    );

    // Verify trigram CSR is identical.
    let (loaded_keys, loaded_offsets, loaded_values) = loaded.trigram.as_csr();
    assert_eq!(loaded_keys, tri_keys, "trigram keys mismatch");
    assert_eq!(loaded_offsets, tri_offsets, "trigram offsets mismatch");
    assert_eq!(loaded_values, tri_values, "trigram values mismatch");

    // Verify other fields survived.
    assert_eq!(loaded.letter, 'T');
    assert_eq!(loaded.records.len(), 3);
    assert_eq!(loaded.names.as_slice(), b"foobarbaz");
    assert_eq!(loaded.source_epoch, 42);
}

#[test]
fn v5_backward_compat_rebuilds_trigram() {
    // Serialize a v6 index, then patch the version to v5 and replace
    // the trigram section with the v5 sentinel (trigram_count = 0).
    let index = make_test_index();
    let mut serialized = serialize_compact(&index);

    // Patch version to 5.
    serialized
        .get_mut(8..10)
        .expect("buffer too short for version")
        .copy_from_slice(&5_u16.to_le_bytes());

    // Find the trigram section: after children CSR.
    // Children CSR starts after names, offsets are (records+1)*4, then values.
    let record_count = index.records.len();
    let names_len = index.names.len();
    let records_end = 26 + record_count * RECORD_BYTES;
    let names_end = records_end + names_len;
    let csr_offsets_end = names_end + (record_count + 1) * 4;
    let total_children = index.children.total_children();
    let postings_end = csr_offsets_end + total_children * 4;

    // Truncate at postings_end + 4 (v5 sentinel: trigram_count = 0).
    serialized.truncate(postings_end + 4);
    serialized
        .get_mut(postings_end..postings_end + 4)
        .expect("buffer too short for trigram sentinel")
        .copy_from_slice(&0_u32.to_le_bytes());

    let (loaded, _tri_ms) = deserialize_compact(&serialized, 'T').unwrap();

    // Trigram was rebuilt — should match the original.
    let (orig_keys, orig_offsets, orig_values) = index.trigram.as_csr();
    let (loaded_keys, loaded_offsets, loaded_values) = loaded.trigram.as_csr();
    assert_eq!(loaded_keys, orig_keys, "rebuilt trigram keys mismatch");
    assert_eq!(
        loaded_offsets, orig_offsets,
        "rebuilt trigram offsets mismatch"
    );
    assert_eq!(
        loaded_values, orig_values,
        "rebuilt trigram values mismatch"
    );
}

#[test]
fn current_header_version() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);
    let b8 = *serialized.get(8).expect("missing byte 8");
    let b9 = *serialized.get(9).expect("missing byte 9");
    let version = u16::from_le_bytes([b8, b9]);
    assert_eq!(version, COMPACT_VERSION);
}

/// Phase 4 Commit D — every folded record name in the source
/// index must resolve to `contains() == true` after a v9
/// round-trip.  `build_bloom` inserts case-folded basenames (see
/// `compact_filters::build_bloom`), so the query side mirrors the
/// fold contract.  Bloom false-negatives are impossible by
/// construction; a failure here proves the cache-format wiring
/// dropped or corrupted the section.
#[test]
fn v9_round_trip_preserves_bloom() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);
    let (loaded, _tri_ms) = deserialize_compact(&serialized, 'T').expect("deser v9");

    let bloom = loaded.bloom.as_ref().expect("v9 cache must carry bloom");
    let mut fold_buf: Vec<u8> = Vec::new();
    for record in &index.records {
        let start = record.name_offset as usize;
        let end = start + usize::from(record.name_len);
        let name_bytes = index
            .names
            .get(start..end)
            .expect("record name slice in source index");
        let name_str =
            core::str::from_utf8(name_bytes).expect("test fixture names are valid UTF-8");
        fold_buf.clear();
        let folded = index.fold.fold_into(name_str, &mut fold_buf);
        assert!(
            bloom.contains(folded.as_bytes()),
            "loaded bloom missed folded name {name_str:?} -> {folded:?}"
        );
    }
}

/// Phase 4 Commit D — the loaded path-trie must be byte-equivalent
/// to one freshly built from the same records / names / fold.  A
/// mismatch in node count, name buffer, or CSR arrays signals the
/// trie section round-trip dropped data.
#[test]
fn v9_round_trip_preserves_path_trie() {
    let index = make_test_index();
    let expected = index.build_path_trie();

    let serialized = serialize_compact(&index);
    let (loaded, _tri_ms) = deserialize_compact(&serialized, 'T').expect("deser v9");
    let actual = loaded.path_trie.as_ref().expect("v9 cache must carry trie");

    assert_eq!(
        actual.nodes().len(),
        expected.nodes().len(),
        "trie node count differs after round-trip"
    );
    assert_eq!(
        actual.names(),
        expected.names(),
        "trie names buffer differs after round-trip"
    );
    assert_eq!(
        actual.child_offsets(),
        expected.child_offsets(),
        "trie CSR offsets differ after round-trip"
    );
    assert_eq!(
        actual.child_indices(),
        expected.child_indices(),
        "trie CSR indices differ after round-trip"
    );
}

#[test]
fn v1_rejected() {
    let mut data = vec![0_u8; 64];
    data.get_mut(..8)
        .expect("buffer too short for magic")
        .copy_from_slice(COMPACT_MAGIC);
    data.get_mut(8..10)
        .expect("buffer too short for version")
        .copy_from_slice(&1_u16.to_le_bytes());
    assert!(deserialize_compact(&data, 'X').is_err());
}

#[test]
fn truncated_data_rejected() {
    assert!(deserialize_compact(b"short", 'X').is_err());
}

#[test]
fn ext_names_round_trips() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);
    let (deser, _) = deserialize_compact(&serialized, 'T').expect("deser");
    assert_eq!(deser.ext_names, index.ext_names);
}

// ─── Phase 2b: runtime-mmap deserialize variant ─────────────────────

/// Open a fresh `DefaultRuntimeDir` + per-test runtime tempfile path
/// inside a `TempDir` so the tempfile is scoped to the test.
fn runtime_fixture(file_name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join(file_name);
    (dir, path)
}

#[test]
fn v6_round_trip_via_runtime_mmap_preserves_trigram() {
    let index = make_test_index();
    let (orig_keys, orig_offsets, orig_values) = index.trigram.as_csr();
    let original_keys = orig_keys.to_vec();
    let original_offsets = orig_offsets.to_vec();
    let original_values = orig_values.to_vec();
    assert!(!original_keys.is_empty(), "test index should have trigrams");

    let serialized = serialize_compact(&index);
    let (_tmp, runtime_path) = runtime_fixture("v6_runtime_mmap.live");
    let runtime_dir = uffs_security::runtime_dir::DefaultRuntimeDir::default();
    let (loaded, tri_ms) =
        deserialize_compact_into_runtime(&serialized, 'T', &runtime_dir, &runtime_path)
            .expect("runtime mmap deser");

    // v6+ CSR is loaded from disk — never rebuilt.
    assert!(
        tri_ms < 500,
        "trigram took {tri_ms}ms via runtime mmap — should be near-zero for cached CSR"
    );

    // Trigram CSR survives byte-for-byte through the mmap path.
    let (loaded_keys, loaded_offsets, loaded_values) = loaded.trigram.as_csr();
    assert_eq!(loaded_keys, original_keys.as_slice());
    assert_eq!(loaded_offsets, original_offsets.as_slice());
    assert_eq!(loaded_values, original_values.as_slice());

    // Records + names columns are now mmap-backed but observably
    // identical to the heap-backed source.
    assert_eq!(loaded.letter, 'T');
    assert_eq!(loaded.records.len(), 3);
    assert_eq!(loaded.names.as_slice(), b"foobarbaz");
    assert_eq!(loaded.source_epoch, 42);
    assert_eq!(loaded.ext_names, index.ext_names);
}

#[test]
fn mmap_path_byte_equal_to_heap_path() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);

    let (heap_loaded, _) = deserialize_compact(&serialized, 'T').expect("heap deser");

    let (_tmp, runtime_path) = runtime_fixture("byte_equal.live");
    let runtime_dir = uffs_security::runtime_dir::DefaultRuntimeDir::default();
    let (mmap_loaded, _) =
        deserialize_compact_into_runtime(&serialized, 'T', &runtime_dir, &runtime_path)
            .expect("mmap deser");

    // The two storage variants must yield identical observable
    // contents — bytemuck-cast records and names slice should be
    // bitwise-equal.
    assert_eq!(
        bytemuck::cast_slice::<CompactRecord, u8>(heap_loaded.records.as_slice()),
        bytemuck::cast_slice::<CompactRecord, u8>(mmap_loaded.records.as_slice()),
        "records bytes must match between heap and mmap variants",
    );
    assert_eq!(
        heap_loaded.names.as_slice(),
        mmap_loaded.names.as_slice(),
        "names bytes must match between heap and mmap variants",
    );
    assert_eq!(heap_loaded.ext_names, mmap_loaded.ext_names);
    assert_eq!(heap_loaded.source_epoch, mmap_loaded.source_epoch);
}

#[test]
fn runtime_mmap_rejects_truncated_data() {
    let (_tmp, runtime_path) = runtime_fixture("truncated.live");
    let runtime_dir = uffs_security::runtime_dir::DefaultRuntimeDir::default();
    // `Result::err()` avoids the `T: Debug` bound that `expect_err`
    // would impose on `(DriveCompactIndex, u128)`.
    let err = deserialize_compact_into_runtime(b"short", 'X', &runtime_dir, &runtime_path)
        .err()
        .expect("truncated data must error via runtime path");
    assert_eq!(err.kind(), io::ErrorKind::Other);
}

// ─── Phase 2b Commit F: memory regression — storage-variant pinning ───
//
// The runtime mmap path (`deserialize_compact_into_runtime`) must serve
// the two largest columns (`records`, `names`) from the kernel page
// cache, not the heap.  The heap path (`deserialize_compact`) must
// continue to allocate `Vec`s.  Asserting the [`ColumnStorage`] variant
// directly is the deterministic, byte-precise version of the RSS check
// the implementation plan (§3 Phase 2b Commit F) originally proposed —
// the RSS approach is dominated by page granularity (4 KB / 16 KB) at
// any feasible test scale and would be flaky in CI.

#[test]
fn runtime_path_yields_mmap_backed_columns() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);

    let (_tmp, runtime_path) = runtime_fixture("variant_mmap.live");
    let runtime_dir = uffs_security::runtime_dir::DefaultRuntimeDir::default();
    let (loaded, _tri_ms) =
        deserialize_compact_into_runtime(&serialized, 'T', &runtime_dir, &runtime_path)
            .expect("mmap deser");

    assert!(
        loaded.records.is_mmap(),
        "records column must be Mmap-backed after deserialize_compact_into_runtime"
    );
    assert!(
        loaded.names.is_mmap(),
        "names column must be Mmap-backed after deserialize_compact_into_runtime"
    );
    assert!(
        !loaded.records.is_vec(),
        "records column must NOT be Vec-backed after deserialize_compact_into_runtime"
    );
    assert!(
        !loaded.names.is_vec(),
        "names column must NOT be Vec-backed after deserialize_compact_into_runtime"
    );
}

#[test]
fn heap_path_yields_vec_backed_columns() {
    let index = make_test_index();
    let serialized = serialize_compact(&index);

    let (loaded, _tri_ms) = deserialize_compact(&serialized, 'T').expect("heap deser");

    assert!(
        loaded.records.is_vec(),
        "records column must be Vec-backed after deserialize_compact (legacy heap path)"
    );
    assert!(
        loaded.names.is_vec(),
        "names column must be Vec-backed after deserialize_compact (legacy heap path)"
    );
    assert!(
        !loaded.records.is_mmap(),
        "records column must NOT be Mmap-backed after deserialize_compact"
    );
    assert!(
        !loaded.names.is_mmap(),
        "names column must NOT be Mmap-backed after deserialize_compact"
    );
}

// ─── Mac wake-up regression: purge_legacy_compact_cache_dir contract ───
//
// Pin the four-arm behaviour of [`super::purge_legacy_compact_cache_dir`]
// so the v0.4.23 legacy-directory recovery path can never silently
// regress.  Each test sets up the on-disk state directly inside a
// `tempfile::TempDir` (no dependence on the global `cache_dir()`), then
// exercises the helper and asserts both the return value and the
// resulting filesystem state.

/// Path doesn't exist → `Ok(())`, filesystem unchanged.  Cold-boot
/// first save hits this arm.
#[test]
fn purge_legacy_dir_missing_path_is_noop() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let path = tmp.path().join("Z_compact.uffs");
    assert!(!path.exists(), "precondition: path absent");

    purge_legacy_compact_cache_dir(&path, 'Z').expect("missing path must be Ok(())");

    assert!(
        !path.exists(),
        "missing path must remain missing after purge"
    );
}

/// Path is a regular file → `Ok(())`, file untouched.  Steady-state
/// after the first successful save lands here on every subsequent call.
#[test]
fn purge_legacy_dir_regular_file_is_noop() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let path = tmp.path().join("Z_compact.uffs");
    std::fs::write(&path, b"existing cache bytes").expect("seed regular file");

    purge_legacy_compact_cache_dir(&path, 'Z').expect("regular file must be Ok(())");

    let bytes = std::fs::read(&path).expect("regular file must still be readable");
    assert_eq!(
        bytes, b"existing cache bytes",
        "regular file content must survive purge unchanged"
    );
}

/// Path is an **empty** directory (the v0.4.23 layout) → directory
/// removed, helper returns `Ok(())`.  This is the actual legacy state
/// observed on dogfood Mac filesystems on 2026-04-28.
#[test]
fn purge_legacy_dir_empty_directory_is_removed() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let path = tmp.path().join("Z_compact.uffs");
    std::fs::create_dir(&path).expect("seed empty directory");
    assert!(path.is_dir(), "precondition: path is empty dir");

    purge_legacy_compact_cache_dir(&path, 'Z').expect("empty dir must be Ok(())");

    assert!(
        !path.exists(),
        "empty legacy directory must be removed by purge"
    );
}

/// Path is a **non-empty** directory → helper returns `Err` (the
/// underlying `ENOTEMPTY`) and the directory is left intact.  Defensive
/// guard against a hypothetical future regression that populates the
/// path: we want the daemon to surface the failure loudly rather than
/// silently `remove_dir_all` user data.
#[test]
fn purge_legacy_dir_non_empty_directory_is_refused() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let path = tmp.path().join("Z_compact.uffs");
    std::fs::create_dir(&path).expect("seed dir");
    std::fs::write(path.join("unexpected.bin"), b"surprise").expect("seed dir contents");

    let err = purge_legacy_compact_cache_dir(&path, 'Z')
        .expect_err("non-empty dir must propagate the underlying io::Error");
    assert!(
        matches!(
            err.kind(),
            io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::Other
        ),
        "non-empty dir must produce DirectoryNotEmpty (or fall back to Other on \
         older platforms); got {kind:?}",
        kind = err.kind(),
    );

    assert!(
        path.is_dir(),
        "non-empty dir must remain on disk after refused purge",
    );
    assert!(
        path.join("unexpected.bin").is_file(),
        "non-empty dir contents must be untouched after refused purge",
    );
}
