// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Storage serialization/deserialization regression tests.

use super::*;

const RECORD_COUNT_OFFSET: usize = 48;
const NAMES_SIZE_OFFSET: usize = 56;
const LINKS_COUNT_OFFSET: usize = 64;

fn empty_serialized_index() -> Vec<u8> {
    MftIndex::new(crate::platform::DriveLetter::C).serialize(123, 456, crate::usn::Usn::new(789))
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn deserialize_rejects_record_count_that_is_too_large() {
    let mut data = empty_serialized_index();
    write_u64(&mut data, RECORD_COUNT_OFFSET, u64::MAX);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Record section too large")
    ));
}

#[test]
fn deserialize_rejects_names_size_beyond_remaining_bytes() {
    let mut data = empty_serialized_index();
    // Use a size larger than remaining bytes after header+frs_to_idx+records.
    write_u64(&mut data, NAMES_SIZE_OFFSET, 9999);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Names section exceeds remaining data")
    ));
}

#[test]
fn deserialize_rejects_links_count_beyond_remaining_bytes() {
    let mut data = empty_serialized_index();
    write_u64(&mut data, LINKS_COUNT_OFFSET, 1);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Links section exceeds remaining data")
    ));
}

// ── WI-5.3 malformed-input corpus (cache deserializer) ───────────────
//
// The cache deserializer parses **untrusted on-disk bytes** (a persisted
// index reloaded at startup). The daemon runs `panic = "abort"`, so a panic
// here on a truncated/corrupt cache file is a whole-process DoS. The
// deserializer already returns `Result<_, &'static str>` and reads via
// `.get(..).ok_or(..)?` + `checked_*` (WI-5.2 era); this corpus locks that
// in. The contract under test for arbitrary input is: **`deserialize`
// returns `Ok` or `Err` — it never panics.**

/// Build a *populated* index (records, names, several sections) and serialize
/// it, so the mutation/fuzz corpus exercises every section, not just the
/// header of an empty index.
fn populated_serialized_index() -> Vec<u8> {
    let mut index = MftIndex::new(crate::platform::DriveLetter::C);
    for (frs, name) in [
        (100_u64, "alpha.txt"),
        (101, "beta.rs"),
        (102, "gamma"),
        (103, "delta.log"),
    ] {
        let offset = index.add_name(name);
        let ext = index.intern_extension(name);
        let len = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let record = index.get_or_create(frs.into());
        record.first_name.name = IndexNameRef::new(offset, len, true, ext);
    }
    index.build_extension_index();
    index.serialize(123, 456, crate::usn::Usn::new(789))
}

/// A valid blob must round-trip (sanity baseline for the mutation tests).
#[test]
fn deserialize_accepts_a_valid_roundtrip() {
    let data = populated_serialized_index();
    MftIndex::deserialize(&data).expect("a freshly serialized index must deserialize cleanly");
}

/// Truncating a valid blob at *every* length must never panic. The
/// deserializer is lenient about some trailing/optional sections, so a
/// near-complete prefix may legitimately deserialize `Ok`; the guarantee
/// WI-5.3 locks in is **liveness** (Ok or Err, never a panic/abort), not
/// that every prefix is rejected. Truncation into a *length-bearing header*
/// field, however, must always be an error (checked separately below).
#[test]
fn deserialize_survives_truncation_at_every_boundary() {
    let full = populated_serialized_index();
    let full_len = full.len();

    for cut in 0..full_len {
        let truncated = full.get(..cut).unwrap_or(&[]);
        // Property under test: returns (Ok or Err) without panicking.
        let _outcome = MftIndex::deserialize(truncated);
    }

    // A blob truncated inside the fixed header (before all section lengths are
    // even readable) is always rejected — never silently accepted.
    let header_cut = full.get(..LINKS_COUNT_OFFSET).unwrap_or(&[]);
    MftIndex::deserialize(header_cut)
        .expect_err("a blob cut inside the fixed header must be rejected");

    // The complete blob still round-trips.
    MftIndex::deserialize(&full).expect("the complete blob must round-trip");
}

/// Empty and 1-byte inputs are rejected without panic.
#[test]
fn deserialize_rejects_tiny_inputs() {
    MftIndex::deserialize(&[]).expect_err("empty input must be rejected");
    MftIndex::deserialize(&[0_u8]).expect_err("1-byte input must be rejected");
    MftIndex::deserialize(&[0xFF_u8]).expect_err("1-byte input must be rejected");
}

/// Writing an out-of-range count/size into each length-bearing header field
/// must produce a clean error (no overflow panic, no OOB slice).
#[test]
fn deserialize_rejects_oversized_section_fields() {
    for offset in [RECORD_COUNT_OFFSET, NAMES_SIZE_OFFSET, LINKS_COUNT_OFFSET] {
        let mut data = populated_serialized_index();
        if data.len() >= offset + 8 {
            write_u64(&mut data, offset, u64::MAX);
            let result = MftIndex::deserialize(&data);
            assert!(
                result.is_err(),
                "u64::MAX in the section-length field at offset {offset} must be rejected, got {result:?}"
            );
        }
    }
}

/// Seeded, deterministic fuzz: thousands of mutated/random blobs through the
/// deserializer, asserting it always returns (Ok or Err) and never panics.
/// `ChaCha8Rng::seed_from_u64` makes the corpus reproducible in CI.
#[test]
fn deserialize_never_panics_on_seeded_fuzz() {
    use rand::{Rng as _, RngExt as _, SeedableRng as _};
    use rand_chacha::ChaCha8Rng;

    let baseline = populated_serialized_index();
    let mut rng = ChaCha8Rng::seed_from_u64(0x0053_F0FF_u64);

    for _ in 0..5_000 {
        let blob = match rng.random_range(0_u8..4) {
            // (a) flip a handful of bytes in an otherwise-valid blob.
            0 => {
                let mut data = baseline.clone();
                let flips = rng.random_range(1_usize..=8);
                for _ in 0..flips {
                    if !data.is_empty() {
                        let idx = rng.random_range(0..data.len());
                        if let Some(byte) = data.get_mut(idx) {
                            *byte = rng.random::<u8>();
                        }
                    }
                }
                data
            }
            // (b) truncate a valid blob at a random length.
            1 => {
                let cut = rng.random_range(0..=baseline.len());
                baseline.get(..cut).unwrap_or(&[]).to_vec()
            }
            // (c) valid blob with random trailing garbage appended.
            2 => {
                let mut data = baseline.clone();
                let extra = rng.random_range(0_usize..64);
                let mut tail = vec![0_u8; extra];
                rng.fill_bytes(&mut tail);
                data.extend_from_slice(&tail);
                data
            }
            // (d) fully random bytes of random length.
            _ => {
                let len = rng.random_range(0_usize..256);
                let mut data = vec![0_u8; len];
                rng.fill_bytes(&mut data);
                data
            }
        };

        // The property: returns Ok or Err, never panics. We deliberately
        // ignore which — corrupt input legitimately may or may not parse;
        // liveness (no panic / no abort) is what WI-5.3 guarantees.
        let _outcome = MftIndex::deserialize(&blob);
    }
}
