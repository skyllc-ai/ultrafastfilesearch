// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Logical Cluster Number (LCN) newtype.
//!
//! NTFS identifies a position on disk by its **cluster number** — a
//! 0-based index from the start of the volume.  Cluster numbers are
//! signed 64-bit values (`LONGLONG` in the Win32 ABI) because:
//!
//! * `FSCTL_GET_RETRIEVAL_POINTERS` uses `-1` (`LCN_HOLE`) to mark sparse /
//!   unallocated extents of a file (see the `MftExtent.lcn` field that this
//!   newtype now backs).
//! * Data-run decoding parses signed deltas from the on-disk `$DATA` attribute,
//!   which can in principle move negative on a short hop before the running
//!   total clamps positive again.
//!
//! Wrapping that raw `i64` in a [`Lcn`] newtype lets the compiler
//! enforce that:
//!
//! * sparse / hole detection always goes through [`Lcn::is_hole`], instead of
//!   every caller open-coding `lcn < 0`;
//! * the unsigned byte-offset arithmetic (`raw_unsigned() * bytes_per_cluster`)
//!   is documented as a deliberate exact-bit-pattern reinterpret — the same
//!   `cast_unsigned` discipline that already lives in `MftExtent::byte_offset`;
//! * cross-crate consumers (`uffs-diag`, the `commands/windows/*` CLI surface)
//!   compare LCN values monotonically through the derived `Ord` rather than
//!   ad-hoc signed integer arithmetic.

use core::fmt;

/// Logical Cluster Number — a signed cluster index from the start of
/// an NTFS volume.
///
/// Newtype wrapper around the raw `i64` (`LONGLONG`) the Win32 ABI
/// uses for cluster identifiers — `FSCTL_GET_RETRIEVAL_POINTERS`,
/// `FSCTL_GET_VOLUME_BITMAP`, and the on-disk `$DATA` data-run encoding
/// all carry signed 64-bit cluster values.  We preserve that
/// representation byte-for-byte so on-disk + on-wire formats are
/// unchanged.
///
/// # Sparse / hole convention
///
/// NTFS marks unallocated extents in retrieval-pointer buffers with
/// `LCN_HOLE = -1`.  Any negative value is treated as sparse by the
/// kernel and by [`Self::is_hole`].
///
/// **Data runs** use a *different* sparse convention: a run whose
/// encoded length is `0` results in an LCN that has not advanced
/// from its previous value, and the existing decoder marks those as
/// sparse via `lcn == Lcn::ZERO`.  Both checks are preserved by the
/// migration; callers continue to use the predicate that matches
/// their NTFS structure.
///
/// # Invariants
///
/// Carried by the type system:
///
/// * `Copy + Eq + Hash + Ord` — safe to drop into `HashMap` / `BTreeMap` keys,
///   compare cheaply, and pass by value.
///
/// Not carried by the type system (kernel-issued / format-defined):
///
/// * Valid cluster numbers are non-negative; only sparse-marker sentinels are
///   negative.  Callers MUST check [`Self::is_hole`] before treating a value as
///   an unsigned byte offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lcn(i64);

impl Lcn {
    /// Cluster `0` — the first cluster of the volume.
    ///
    /// On NTFS this is the boot sector and is never assigned to user
    /// data, so `Lcn::ZERO` doubles as the "running data-run total has
    /// not yet advanced" sentinel that the decoder treats as sparse.
    pub const ZERO: Self = Self(0);

    /// `LCN_HOLE = -1` — the canonical Win32 sentinel for a sparse /
    /// unallocated extent in `FSCTL_GET_RETRIEVAL_POINTERS` output.
    ///
    /// Use [`Self::is_hole`] for the actual sparse check (which
    /// matches *any* negative value, mirroring the existing
    /// `lcn < 0` discipline); this constant exists primarily for
    /// constructing test fixtures and documenting intent.
    pub const HOLE: Self = Self(-1);

    /// Wrap a raw `i64` from a Win32 FFI buffer or the on-disk
    /// data-run decoder.
    ///
    /// Cluster numbers are kernel-issued — there is no client-side
    /// validation we can perform on a single value in isolation
    /// (negative values are valid sparse markers).
    #[must_use]
    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    /// Underlying raw `i64`.
    ///
    /// Use this **only** at FFI / serialization boundaries or for
    /// the signed arithmetic that data-run delta decoding requires.
    /// Internal logic should compare [`Lcn`] values directly via
    /// the derived [`Ord`] / [`PartialEq`] impls.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Underlying value reinterpreted as an unsigned 64-bit cluster
    /// number — the byte-offset arithmetic helper.
    ///
    /// # Caller contract
    ///
    /// **Callers MUST first verify the value is non-negative** (via
    /// [`Self::is_hole`]).  Passing a sparse-marker LCN through this
    /// method silently yields a huge bogus offset
    /// (`i64::MIN.cast_unsigned() == 0x8000_0000_0000_0000`).
    /// Existing call sites already perform this check before the
    /// multiplication; the migration preserves that discipline.
    #[must_use]
    pub const fn raw_unsigned(self) -> u64 {
        self.0.cast_unsigned()
    }

    /// `true` when this value is a sparse / hole marker (`< 0`).
    ///
    /// Mirrors the historic `extent.lcn < 0` guard used in
    /// `MftExtent::byte_offset`, the extent-map physical-offset
    /// translator, the chunking pass, and the diagnostic dumpers.
    /// Any negative value counts as a hole — not just the canonical
    /// `LCN_HOLE = -1` — so flaky FFI buffers that hand out other
    /// negative sentinels still get filtered out.
    #[must_use]
    pub const fn is_hole(self) -> bool {
        self.0 < 0
    }

    /// `true` when this value equals [`Lcn::ZERO`].
    ///
    /// Used by the data-run decoder's sparse check
    /// (`DataRun::is_sparse`), which treats a run that has not
    /// advanced the running LCN total as sparse.  Distinct from
    /// [`Self::is_hole`] because the data-run encoding never emits
    /// `-1`; it emits `0` (no offset encoded).
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl From<i64> for Lcn {
    fn from(raw: i64) -> Self {
        Self(raw)
    }
}

impl From<Lcn> for i64 {
    fn from(value: Lcn) -> Self {
        value.0
    }
}

impl fmt::Display for Lcn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Lcn;

    #[test]
    fn raw_roundtrip_preserves_i64_exactly() {
        // Wire-format contract: any `i64` the kernel hands us must come
        // back byte-identical via `raw()`.  This keeps the
        // `FSCTL_GET_RETRIEVAL_POINTERS` decode path bit-for-bit
        // compatible after the newtype migration.
        for raw in [i64::MIN, -1, 0, 1, 42, i64::MAX] {
            assert_eq!(Lcn::new(raw).raw(), raw, "round-trip drift for {raw}");
        }
    }

    #[test]
    fn from_into_i64_symmetry() {
        let raw: i64 = 0x0123_4567_89AB_CDEF;
        let lcn: Lcn = raw.into();
        let back: i64 = lcn.into();
        assert_eq!(back, raw);
    }

    #[test]
    fn zero_and_hole_sentinels_match_literal_values() {
        assert_eq!(Lcn::ZERO, Lcn::new(0));
        assert_eq!(Lcn::HOLE, Lcn::new(-1));
        assert!(Lcn::ZERO.is_zero());
        assert!(!Lcn::ZERO.is_hole());
        assert!(Lcn::HOLE.is_hole());
        assert!(!Lcn::HOLE.is_zero());
    }

    #[test]
    fn is_hole_matches_any_negative_value() {
        // Pin the `< 0` discipline: not just `-1`, every negative
        // sentinel a flaky FFI buffer might emit is treated as
        // sparse.  Matches the historic `extent.lcn < 0` guard.
        assert!(Lcn::new(-1).is_hole());
        assert!(Lcn::new(-2).is_hole());
        assert!(Lcn::new(i64::MIN).is_hole());
        assert!(!Lcn::new(0).is_hole());
        assert!(!Lcn::new(1).is_hole());
        assert!(!Lcn::new(i64::MAX).is_hole());
    }

    #[test]
    fn raw_unsigned_reinterprets_bit_pattern() {
        // Byte-offset arithmetic uses `cast_unsigned` for the
        // exact-bit-pattern reinterpret.  Pin that contract: for
        // non-negative values the unsigned view equals the raw
        // value; for negative values it equals the i64-as-u64
        // bit reinterpret (test fixture only — callers must check
        // `is_hole` first in production code).
        assert_eq!(Lcn::new(0).raw_unsigned(), 0_u64);
        assert_eq!(Lcn::new(1).raw_unsigned(), 1_u64);
        assert_eq!(Lcn::new(i64::MAX).raw_unsigned(), i64::MAX.cast_unsigned());
        assert_eq!(Lcn::new(-1).raw_unsigned(), u64::MAX);
    }

    #[test]
    fn display_matches_raw_i64() {
        // Logs / tracing rely on `Display` rendering as the raw number,
        // not the `Lcn(N)` Debug-wrapper form.
        for raw in [i64::MIN, -1, 0, 1, 1_234_567, i64::MAX] {
            assert_eq!(format!("{}", Lcn::new(raw)), raw.to_string());
        }
    }

    #[test]
    fn ord_matches_underlying_i64_ordering() {
        // Extent-walk and bitmap traversal rely on the natural
        // signed ordering: hole sentinels sort below `ZERO`, valid
        // clusters sort by their numeric address.
        assert!(Lcn::HOLE < Lcn::ZERO);
        assert!(Lcn::ZERO < Lcn::new(1));
        assert!(Lcn::new(1) < Lcn::new(2));
        assert!(Lcn::new(i64::MIN) < Lcn::HOLE);
        assert!(Lcn::ZERO < Lcn::new(i64::MAX));
    }
}
