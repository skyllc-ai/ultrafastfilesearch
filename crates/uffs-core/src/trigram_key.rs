// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Trigram key helpers for character-level trigram indices.
//!
//! Provides pack/unpack between 3 folded `u16` codepoints and a `u64`
//! key suitable for hash-map lookup.  Used by [`crate::trigram::TrigramIndex`]
//! and the on-disk compact-cache trigram CSR (see
//! [`crate::compact_cache`]).
//!
//! Relocated from `uffs-text::trigram_key` (2026-05-14) so that
//! `uffs-text` can ship a clean, externally-meaningful publish surface
//! consisting solely of the NTFS-compatible case-folding engine.  These
//! trigram packers are UFFS-index-specific and have no value outside
//! the search-index implementation, so they live next to their only
//! consumer.  See `docs/dev/baseline/2026-05-13/phase_2_5_audits/
//! uffs-text.md` row "`trigram_key` — RELOCATE (1 consumer)".

/// Pack 3 folded `u16` codepoints into a `u64`.
///
/// Layout (MSB → LSB):
///
/// | bits   | content      |
/// |--------|--------------|
/// | 48..64 | always zero  |
/// | 32..48 | `cp0`        |
/// | 16..32 | `cp1`        |
/// |  0..16 | `cp2`        |
///
/// This gives lexicographic ordering on `(cp0, cp1, cp2)` when the `u64`
/// is compared directly: the always-zero top 16 bits are identical for
/// every packed value, so a numeric `<` falls through to `cp0`, then
/// `cp1`, then `cp2` in that order.  The trigram CSR's binary-search
/// lookup in `crate::trigram::TrigramIndex` and the on-disk compact-
/// cache trigram block in `crate::compact_cache` both rely on this
/// property.
#[inline]
#[must_use]
pub(crate) const fn pack_char_trigram(cp0: u16, cp1: u16, cp2: u16) -> u64 {
    (cp0 as u64) << 32 | (cp1 as u64) << 16 | (cp2 as u64)
}

// NOTE: An inverse `unpack_char_trigram` lived alongside `pack_char_trigram`
// before the 2026-05-14 relocation from `uffs-text`.  It had no production
// callers — the index hot-path and the on-disk `compact_cache` v6+ CSR both
// read packed `u64`s directly without ever needing the codepoints back out.
// It was retained only as test scaffolding for round-trip tests.
//
// The tests below replace that round-trip indirection with direct bit-layout
// assertions: they pin the EXACT layout that `compact_cache` reads from disk
// (per the `pack_char_trigram` docstring table: top 16 bits zero, cp0 in
// bits 32..48, cp1 in bits 16..32, cp2 in bits 0..16).  That is a strictly
// stronger property — any change to `pack_char_trigram` that preserves
// lossless round-trip but rearranges the bit positions would slip past
// round-trip testing yet break the on-disk format.  These tests catch that.

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-encode the documented layout once so the assertions read as
    /// "the bits land where the docstring says they land".  Defined as
    /// a separate `const fn` instead of calling `pack_char_trigram` so
    /// that a regression which corrupts both producers identically
    /// still fails the equality test.
    const fn expected_packed(cp0: u16, cp1: u16, cp2: u16) -> u64 {
        ((cp0 as u64) << 32) | ((cp1 as u64) << 16) | (cp2 as u64)
    }

    #[test]
    fn pack_layout_ascii() {
        let ch_a = u16::from(b'A');
        let ch_b = u16::from(b'B');
        let ch_c = u16::from(b'C');
        let packed = pack_char_trigram(ch_a, ch_b, ch_c);

        // Per-slot extraction matches the docstring table.
        assert_eq!(
            (packed >> 32_u32) & 0xFFFF_u64,
            u64::from(ch_a),
            "cp0 slot bits 32..48"
        );
        assert_eq!(
            (packed >> 16_u32) & 0xFFFF_u64,
            u64::from(ch_b),
            "cp1 slot bits 16..32"
        );
        assert_eq!(packed & 0xFFFF_u64, u64::from(ch_c), "cp2 slot bits 0..16");
        // Whole-value equality with the documented expression.
        assert_eq!(packed, expected_packed(ch_a, ch_b, ch_c));
    }

    #[test]
    fn pack_high_16_bits_always_zero() {
        // Layout contract: bits 48..64 carry no data.  This catches an
        // accidental shift by 48 (which would put cp0 into the reserved
        // slot and break the docstring invariant the CSR relies on).
        for (cp0, cp1, cp2) in [
            (0_u16, 0, 0),
            (u16::MAX, u16::MAX, u16::MAX),
            (0x00DC, 0x0042, 0x0045),
            (0xFEDC, 0x1234, 0x5678),
        ] {
            let packed = pack_char_trigram(cp0, cp1, cp2);
            assert_eq!(
                packed >> 48_u32,
                0_u64,
                "high 16 bits must be zero for cp0={cp0:#06x} cp1={cp1:#06x} cp2={cp2:#06x}"
            );
        }
    }

    #[test]
    fn pack_full_u16_range_is_lossless() {
        // Pin: every codepoint in the full u16 range round-trips out of
        // its slot, with no slot bleeding into another (catches an
        // accidental off-by-one in the shift amounts).
        let high = u16::MAX;
        let mid = 0x1234_u16;
        let low = u16::MIN;
        let packed = pack_char_trigram(high, mid, low);
        assert_eq!((packed >> 32_u32) & 0xFFFF_u64, u64::from(high));
        assert_eq!((packed >> 16_u32) & 0xFFFF_u64, u64::from(mid));
        assert_eq!(packed & 0xFFFF_u64, u64::from(low));
        assert_eq!(packed, expected_packed(high, mid, low));
    }

    #[test]
    fn lexicographic_order() {
        // Same-prefix trigrams differ only in the lowest 16 bits, so a
        // direct `u64` comparison must reflect the codepoint order.  This
        // is the property the trigram CSR relies on for binary-search
        // lookup correctness.
        let abc = pack_char_trigram(u16::from(b'A'), u16::from(b'B'), u16::from(b'C'));
        let abd = pack_char_trigram(u16::from(b'A'), u16::from(b'B'), u16::from(b'D'));
        assert!(abc < abd);
    }

    #[test]
    fn lexicographic_order_cp0_dominates() {
        // A change in cp0 (high-16-bits) must outweigh any change in cp2
        // (low-16-bits).  Without this property the binary search over
        // sorted `u64` keys would mis-rank entries when the index spans
        // multiple cp0 buckets.
        let bza = pack_char_trigram(u16::from(b'B'), u16::from(b'Z'), u16::from(b'A'));
        let caa = pack_char_trigram(u16::from(b'C'), u16::from(b'A'), u16::from(b'A'));
        assert!(bza < caa, "cp0 ordering must dominate cp1/cp2");
    }
}
