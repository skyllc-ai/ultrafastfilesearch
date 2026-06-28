// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! WI-4.4 malformed-name tests: an ill-formed (surrogate-bearing) NTFS name
//! must be stored byte-faithfully, preserved at its true position (not
//! collapsed), rendered lossily (U+FFFD) for display, and still enumerated by
//! its true bytes.
//!
//! Split out of the parent `compact_tests` to keep that file under the 800-LOC
//! policy; the synthetic-index fixtures (`fixture_index`, `push_name`) are
//! shared via `super`.

use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, SizeInfo};

use super::{fixture_index, push_name};
use crate::compact::{MalformedRender, build_compact_index};

// ── WI-4.4: a crooked (surrogate-named) file cannot hide from the search
//    layer. Build the compact/search index from an MftIndex holding an
//    ill-formed name and prove it is enumerated and byte-recoverable. ──

/// WTF-8 of `evil` + lone-high-surrogate(U+D800) + `.exe`
/// (`0xD800` → 3-byte WTF-8 `ED A0 80`). Not valid UTF-8.
const CROOKED_NAME_WTF8: &[u8] = &[
    b'e', b'v', b'i', b'l', 0xED, 0xA0, 0x80, b'.', b'e', b'x', b'e',
];

/// Like `push_name`, but stores raw WTF-8 bytes (the lossless ingestion path)
/// for an ill-formed NTFS name.
fn push_name_bytes(index: &mut MftIndex, bytes: &[u8]) -> IndexNameRef {
    let offset = index.add_name_bytes(bytes);
    let len = u16::try_from(bytes.len()).expect("test name too long");
    // Ill-formed → not ASCII, no extension (the `.exe` here is decorative).
    IndexNameRef::new(offset, len, false, 0)
}

#[test]
fn crooked_surrogate_name_is_visible_in_compact_index() {
    let mut idx = fixture_index();

    // Plant a file whose name contains an unpaired surrogate under root.
    let crooked = push_name_bytes(&mut idx, CROOKED_NAME_WTF8);
    let rec = idx.get_or_create(909.into());
    rec.first_name.name = crooked;
    rec.first_name.parent_frs = Into::into(ROOT_FRS);
    rec.first_stream.size = SizeInfo {
        length: 1337,
        allocated: 1536,
    };

    let (drive, _, _) = build_compact_index(uffs_mft::platform::DriveLetter::C, &idx);

    // The crooked file must appear in the compact index — found by its TRUE
    // bytes via the lossless `name_bytes` accessor. This is the "cannot hide"
    // guarantee: a malicious ill-formed name is still enumerated.
    let found = drive
        .records
        .iter()
        .find(|cr| cr.name_bytes(&drive.names) == CROOKED_NAME_WTF8)
        .expect(
            "crooked surrogate-named file must be present + byte-recoverable in the search index",
        );

    assert_eq!(
        found.size, 1337,
        "the crooked file's metadata transfers too"
    );
    // Its lossy &str view is empty (not valid UTF-8) — display degrades, but
    // the file is NOT hidden (it is enumerated above).
    assert_eq!(found.name(&drive.names), "");
    // And the true bytes carry no U+FFFD replacement — nothing was lost.
    assert!(
        !found
            .name_bytes(&drive.names)
            .windows(3)
            .any(|win| win == [0xEF, 0xBF, 0xBD]),
        "the search index must hold the true bytes, not a lossy replacement"
    );
}

// ── WI-4.4: forensic facts on the resolved DisplayRow ────────────────────
//   A CLEAN-named file living under a CROOKED (surrogate-named) directory:
//   its own leaf is well-formed (malformed=false) but its PATH is poisoned
//   (malformed_path=true). This is the "clean file under a crooked dir is
//   still flagged" guarantee, and it exercises the real search → resolve →
//   row-forensics path end to end.

#[test]
fn clean_child_under_crooked_dir_is_path_malformed_not_leaf_malformed() {
    let mut idx = MftIndex::new(uffs_mft::platform::DriveLetter::C);

    // Root.
    let root_name = push_name(&mut idx, ".");
    let root = idx.get_or_create(ROOT_FRS.into());
    root.stdinfo.set_directory(true);
    root.first_name.name = root_name;
    root.first_name.parent_frs = Into::into(ROOT_FRS);

    // A directory whose NAME is ill-formed (lone surrogate), under root.
    let crooked_dir = push_name_bytes(&mut idx, CROOKED_NAME_WTF8);
    let dir = idx.get_or_create(500.into());
    dir.stdinfo.set_directory(true);
    dir.first_name.name = crooked_dir;
    dir.first_name.parent_frs = Into::into(ROOT_FRS);

    // A CLEAN-named child file under the crooked directory.
    let child_name = push_name(&mut idx, "report.txt");
    let child = idx.get_or_create(501.into());
    child.first_name.name = child_name;
    child.first_name.parent_frs = Into::into(500);
    child.first_stream.size = SizeInfo {
        length: 42,
        allocated: 64,
    };

    let (drive, _, _) = build_compact_index(uffs_mft::platform::DriveLetter::C, &idx);

    // Match-all enumeration through the real search/resolve/forensics path.
    let drives = vec![drive];
    let mut filters = crate::search::filters::SearchFilters::default();
    let (rows, _) = crate::search::query::collect_global_top_n(
        &drives,
        100,
        crate::search::field::FieldId::Name,
        false,
        crate::search::backend::FilterMode::All,
        &mut filters,
    );

    // The CLEAN child: leaf well-formed, but PATH malformed (ancestor crooked).
    let child_row = rows
        .iter()
        .find(|row| row.name() == "report.txt")
        .expect("clean-named child must be enumerated");
    assert!(
        !child_row.malformed,
        "the child's own leaf name is well-formed"
    );
    assert!(
        child_row.malformed_path,
        "a clean file under a crooked directory must be flagged malformed_path"
    );
    // A clean leaf carries no name_hex evidence (only ill-formed leaves do).
    assert!(child_row.name_hex.is_none());

    // The crooked DIRECTORY itself: leaf malformed + name_hex evidence present.
    // Its lossy `&str` view is empty, so match it by its true bytes via name_hex.
    let dir_row = rows
        .iter()
        .find(|row| row.malformed)
        .expect("the crooked directory must itself be enumerated and flagged");
    assert!(dir_row.malformed_path);
    assert_eq!(
        dir_row.name_hex.as_deref(),
        Some("6576696ceda0802e657865"),
        "name_hex is the lowercase hex of the true WTF-8 bytes of `evil<D800>.exe`"
    );
}

// ── WI-4.4 regression: a malformed directory must NOT collapse the path ──
//   This is the bug behind the G-drive parity mismatch: the lossy `name()`
//   returned "" for a surrogate-named directory, so the path resolver pushed
//   an empty segment and `…\evil�.exe\report.txt` collapsed to `…\report.txt`
//   (re-parenting the child to the volume root + producing duplicate parent
//   rows). `name_display()` renders the segment lossily (U+FFFD) so its
//   position is preserved — matching the reference C++ tool.

#[test]
fn crooked_dir_segment_is_preserved_in_resolved_path() {
    use crate::search::tree;

    // root → <crooked surrogate dir, FRS 500> → report.txt (FRS 501)
    let mut idx = MftIndex::new(uffs_mft::platform::DriveLetter::C);
    let root_name = push_name(&mut idx, ".");
    let root = idx.get_or_create(ROOT_FRS.into());
    root.stdinfo.set_directory(true);
    root.first_name.name = root_name;
    root.first_name.parent_frs = Into::into(ROOT_FRS);

    let crooked_dir = push_name_bytes(&mut idx, CROOKED_NAME_WTF8);
    let dir = idx.get_or_create(500.into());
    dir.stdinfo.set_directory(true);
    dir.first_name.name = crooked_dir;
    dir.first_name.parent_frs = Into::into(ROOT_FRS);

    let child_name = push_name(&mut idx, "report.txt");
    let child = idx.get_or_create(501.into());
    child.first_name.name = child_name;
    child.first_name.parent_frs = Into::into(500);

    let (drive, _, _) = build_compact_index(uffs_mft::platform::DriveLetter::C, &idx);

    // The crooked directory's lossy display: `evil` + U+FFFD + `.exe`.
    let crooked_display = "evil\u{FFFD}.exe"; // one U+FFFD per offending code unit (matches C++)
    assert!(
        crooked_display.contains('\u{FFFD}'),
        "fixture must render lossily (got {crooked_display:?})"
    );

    // `name_display` preserves the segment where `name` empties it.
    let dir_rec = drive
        .records
        .iter()
        .find(|cr| cr.name_bytes(&drive.names) == CROOKED_NAME_WTF8)
        .expect("crooked dir present");
    assert_eq!(
        dir_rec.name(&drive.names),
        "",
        "lossy &str view still empties"
    );
    assert_eq!(
        dir_rec.name_display(&drive.names),
        crooked_display,
        "name_display renders the surrogate as U+FFFD, not empty"
    );

    let child_idx = drive
        .records
        .iter()
        .position(|cr| cr.name_bytes(&drive.names) == b"report.txt")
        .expect("child present");

    // Both resolvers must keep the child UNDER the crooked dir (not at root).
    let expected = format!("C:\\{crooked_display}\\report.txt");
    let plain = tree::resolve_path(&drive, child_idx, "C:", MalformedRender::Lossy);
    assert_eq!(
        plain, expected,
        "plain resolver must preserve the crooked segment"
    );
    assert_ne!(
        plain, "C:\\report.txt",
        "regression: the crooked segment must not collapse to the parent"
    );

    // End-to-end: the `--normalize-malformed` render mode threads through the
    // resolver, marking the bad segment inline.
    let normalized = tree::resolve_path(&drive, child_idx, "C:", MalformedRender::Normalized);
    assert_eq!(
        normalized, "C:\\evil<BAD:D800>.exe\\report.txt",
        "normalized render must surface the <BAD:HHHH> marker in the path"
    );

    let mut dir_cache = tree::DirCache::default();
    let mut mal_cache = tree::MalformedCache::default();
    let (mpath, malformed) = tree::resolve_path_cached_with_malformed(
        &drive,
        child_idx,
        "C:",
        &mut dir_cache,
        &mut mal_cache,
        MalformedRender::Lossy,
    );
    assert_eq!(
        mpath, expected,
        "malformed-aware resolver agrees on the preserved path"
    );
    assert!(
        malformed,
        "the path is flagged malformed (crooked ancestor)"
    );
}

#[test]
fn crooked_leaf_file_is_not_dropped_from_results() {
    // A file whose own LEAF name is ill-formed must still be enumerated in
    // search results (it was being dropped by a lossy-`name().is_empty()`
    // guard — the reason the crooked *files* went missing on the G drive).
    let mut idx = fixture_index();
    let crooked = push_name_bytes(&mut idx, CROOKED_NAME_WTF8);
    let rec = idx.get_or_create(909.into());
    rec.first_name.name = crooked;
    rec.first_name.parent_frs = Into::into(ROOT_FRS);
    rec.first_stream.size = SizeInfo {
        length: 1337,
        allocated: 1536,
    };

    let (drive, _, _) = build_compact_index(uffs_mft::platform::DriveLetter::C, &idx);
    let drives = vec![drive];
    let mut filters = crate::search::filters::SearchFilters::default();
    let (rows, _) = crate::search::query::collect_global_top_n(
        &drives,
        1000,
        crate::search::field::FieldId::Name,
        false,
        crate::search::backend::FilterMode::All,
        &mut filters,
    );

    let crooked_display = "evil\u{FFFD}.exe"; // one U+FFFD per offending code unit (matches C++)
    let found = rows
        .iter()
        .find(|row| row.malformed)
        .expect("the crooked-leaf file must be enumerated in search results, not silently dropped");
    assert!(
        found.path.contains(crooked_display),
        "its path must carry the lossy leaf name, got {:?}",
        found.path
    );
}

#[test]
fn malformed_name_renders_lossy_and_normalized() {
    // `evil` + lone HIGH surrogate U+D800 + `.exe`.
    let mut idx = fixture_index();
    let crooked = push_name_bytes(&mut idx, CROOKED_NAME_WTF8);
    let rec = idx.get_or_create(909.into());
    rec.first_name.name = crooked;
    rec.first_name.parent_frs = Into::into(ROOT_FRS);

    let (drive, _, _) = build_compact_index(uffs_mft::platform::DriveLetter::C, &idx);
    let crooked_rec = drive
        .records
        .iter()
        .find(|cr| cr.name_bytes(&drive.names) == CROOKED_NAME_WTF8)
        .expect("crooked record present");

    // Default (lossy): the surrogate run collapses to a single U+FFFD.
    assert_eq!(
        crooked_rec.name_display_with(&drive.names, MalformedRender::Lossy),
        "evil\u{FFFD}.exe"
    );
    // Normalized: the offending code unit (U+D800) becomes a greppable,
    // reversible `<BAD:HHHH>` marker; the valid prefix/suffix are preserved.
    assert_eq!(
        crooked_rec.name_display_with(&drive.names, MalformedRender::Normalized),
        "evil<BAD:D800>.exe"
    );

    // A well-formed name is byte-for-byte identical (and borrowed) under both
    // modes — the markers never touch valid names.
    let docs = drive
        .records
        .iter()
        .find(|cr| cr.name_bytes(&drive.names) == b"Docs")
        .expect("Docs present");
    assert_eq!(
        docs.name_display_with(&drive.names, MalformedRender::Lossy),
        "Docs"
    );
    assert_eq!(
        docs.name_display_with(&drive.names, MalformedRender::Normalized),
        "Docs"
    );
}
