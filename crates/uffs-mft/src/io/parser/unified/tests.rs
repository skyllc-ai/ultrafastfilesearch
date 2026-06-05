// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`super`] (the unified MFT record parser).
//!
//! Covers the UTF-16 → name decode path end to end:
//!
//! * **Lossy decode (WI-4.1)** — [`super::decode_name_u16`] counts U+FFFD
//!   substitutions for ill-formed UTF-16 and bumps the process-global
//!   [`super::lossy_name_count`] tally so index-build can warn.
//! * **Lossless WTF-8 retention (WI-4.4)** — [`super::wtf8_from_utf16le`]
//!   encodes unpaired surrogates byte-faithfully (3-byte `ED xx xx`) instead of
//!   replacing them, and [`super::store_name_lossless`] stores those true bytes
//!   so a surrogate-named file stays findable and cannot be hidden.

use super::{decode_name_u16, lossy_name_count};

#[test]
fn decode_name_u16_lossless_bmp_and_astral() {
    // "Aé😀" — BMP + an astral char (valid surrogate pair). No loss.
    // 'A'=0x0041, 'é'=0x00E9, '😀'=U+1F600 → D83D DE00.
    let units = [0x0041_u16, 0x00E9, 0xD83D, 0xDE00];
    let (name, count) = decode_name_u16(&units);
    assert_eq!(count, 0, "well-formed UTF-16 must decode losslessly");
    assert_eq!(name, "Aé😀");
    assert!(!name.contains(char::REPLACEMENT_CHARACTER));
}

#[test]
fn decode_name_u16_unpaired_surrogate_is_counted_and_replaced() {
    // A lone high surrogate (0xD800) with no following low surrogate —
    // legal on NTFS, illegal in UTF-8. Must NOT panic; must substitute
    // exactly one U+FFFD and report the count.
    let units = [
        0x0066_u16, // 'f'
        0xD800,     // unpaired high
        0x006F,     // 'o'
    ];
    let before = lossy_name_count();
    let (name, count) = decode_name_u16(&units);
    assert_eq!(count, 1, "one unpaired surrogate → one replacement");
    assert!(
        name.contains(char::REPLACEMENT_CHARACTER),
        "decoded name must contain U+FFFD"
    );
    // The process-global tally rose by at least this call's replacement count,
    // so the index-build warn/stat sees the loss (WI-4.1). `lossy_name_count`
    // is a process-wide AtomicU64; other tests in this binary decode lossy
    // names concurrently, so the delta is asserted as a lower bound (`>=`)
    // rather than an exact equality — exactness here would be order-dependent
    // and flaky under parallel test execution.
    assert!(
        lossy_name_count() >= before + u64::from(count),
        "global lossy tally must rise by at least the replacement count"
    );
}

#[test]
fn decode_name_u16_lone_low_surrogate_is_counted() {
    // A lone LOW surrogate (0xDC00) with no preceding high surrogate.
    let units = [0xDC00_u16];
    let (name, count) = decode_name_u16(&units);
    assert_eq!(count, 1);
    assert_eq!(name, "\u{FFFD}");
}

// ── WI-4.4: lossless WTF-8 retention ─────────────────────────────────
use super::{store_name_lossless, wtf8_from_utf16le};
use crate::index::MftIndex;
use crate::platform::DriveLetter;

/// Encode a `&[u16]` to its little-endian byte form (mirrors how the
/// parser hands raw on-disk UTF-16LE to the WTF-8 encoder).
fn utf16le_bytes(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len().saturating_mul(2));
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[test]
fn wtf8_well_formed_matches_utf8() {
    // Well-formed UTF-16 must produce exactly the standard UTF-8 bytes —
    // the common path is byte-identical to a normal `String`.
    for name in ["readme.txt", "café", "日本語", "emoji_😀_file", "Ω≈ç"] {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut wtf8 = Vec::new();
        wtf8_from_utf16le(&utf16le_bytes(&units), &mut wtf8);
        assert_eq!(
            wtf8,
            name.as_bytes(),
            "well-formed name must be plain UTF-8"
        );
    }
}

#[test]
fn wtf8_unpaired_high_surrogate_is_byte_faithful() {
    // "f" + lone high surrogate 0xD800 + "o". WTF-8 encodes 0xD800 as the
    // 3-byte sequence ED A0 80 — byte-faithful, NOT U+FFFD. This is what
    // keeps the file findable by its true name.
    let units = [0x0066_u16, 0xD800, 0x006F];
    let mut wtf8 = Vec::new();
    wtf8_from_utf16le(&utf16le_bytes(&units), &mut wtf8);
    assert_eq!(wtf8, vec![b'f', 0xED, 0xA0, 0x80, b'o']);
    // It is intentionally NOT valid UTF-8 (that is the whole point — the
    // true bytes are retained, not lost to a replacement char).
    core::str::from_utf8(&wtf8).expect_err("WTF-8 surrogate bytes are not valid UTF-8");
    // And it contains no U+FFFD replacement bytes (EF BF BD).
    assert!(!wtf8.windows(3).any(|win| win == [0xEF, 0xBF, 0xBD]));
}

#[test]
fn wtf8_lone_low_surrogate_is_byte_faithful() {
    // Lone low surrogate 0xDC00 → WTF-8 ED B0 80.
    let units = [0xDC00_u16];
    let mut wtf8 = Vec::new();
    wtf8_from_utf16le(&utf16le_bytes(&units), &mut wtf8);
    assert_eq!(wtf8, vec![0xED, 0xB0, 0x80]);
}

#[test]
fn wtf8_valid_pair_becomes_astral_utf8() {
    // A valid surrogate pair must combine into the normal 4-byte UTF-8
    // astral form, not two separate 3-byte surrogate encodings.
    let units = [0xD83D_u16, 0xDE00]; // 😀 U+1F600
    let mut wtf8 = Vec::new();
    wtf8_from_utf16le(&utf16le_bytes(&units), &mut wtf8);
    assert_eq!(wtf8, "😀".as_bytes());
    assert_eq!(core::str::from_utf8(&wtf8).unwrap(), "😀");
}

#[test]
fn store_name_lossless_common_path_is_plain_utf8() {
    // lossy == 0 → stores the display String's bytes verbatim (no WTF-8
    // re-encode), zero extra work on the hot path.
    let mut index = MftIndex::new(DriveLetter::C);
    let raw = utf16le_bytes(&"café".encode_utf16().collect::<Vec<_>>());
    let (off, len) = store_name_lossless(&mut index, "café", &raw, 0);
    assert_eq!(len, "café".len());
    let stored = index
        .names
        .get(off as usize..(off as usize).saturating_add(len))
        .expect("stored slice in range");
    assert_eq!(stored, "café".as_bytes());
}

#[test]
fn store_name_lossless_surrogate_path_retains_true_bytes() {
    // lossy > 0 → the raw UTF-16 is re-encoded to byte-faithful WTF-8 and
    // stored; the stored length is the WTF-8 length. The lossy display
    // "f\u{FFFD}o" is NOT what gets stored — the true bytes are.
    let mut index = MftIndex::new(DriveLetter::C);
    let units = [0x0066_u16, 0xD800, 0x006F];
    let raw = utf16le_bytes(&units);
    let display = "f\u{FFFD}o"; // what decode_utf16le_into produced
    let (off, len) = store_name_lossless(&mut index, display, &raw, 1);
    let stored = index
        .names
        .get(off as usize..(off as usize).saturating_add(len))
        .expect("stored slice in range");
    assert_eq!(stored, vec![b'f', 0xED, 0xA0, 0x80, b'o']);
    // Stored bytes are the lossless WTF-8, distinct from the lossy display.
    assert_ne!(stored, display.as_bytes());
}
