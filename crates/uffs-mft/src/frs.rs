// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! File Record Segment (FRS) newtypes.
//!
//! NTFS identifies every record in the Master File Table by its
//! **File Record Segment number** — a 0-based index into the `$MFT`
//! file.  The on-disk and FFI representations are an unsigned 64-bit
//! integer, but the workspace has historically threaded raw `u64`
//! values through parse, index, query, and wire surfaces.  That made
//! two real correctness hazards open-coded:
//!
//! * **own-FRS vs parent-FRS swap** — closures, struct literals, and function
//!   calls accept `(parent_frs: u64, own_frs: u64)` and the compiler cannot
//!   tell when a caller transposes them.
//! * **sentinel ambiguity** — `0` means *"root of the filesystem metadata; no
//!   base record"* in some contexts and *"unset / null reference"* in others.
//!   Predicates spelled `frs == 0` mix both meanings.
//!
//! This module introduces two newtypes that close both gaps without
//! touching the wire format:
//!
//! * [`Frs`] — a typed FRS value.  Wraps `u64` byte-identically; carries `ZERO`
//!   (the `$MFT` self-record / "null reference") and `ROOT` (FRS 5 — the `.`
//!   root directory) sentinels; exposes [`Frs::is_zero`] / [`Frs::is_root`] so
//!   predicates read at the semantic level callers actually mean.
//! * [`ParentFrs`] — a typed *parent-directory* FRS.  Composed over [`Frs`] so
//!   the type system makes a parent-vs-child swap a compile error.  Convert via
//!   [`ParentFrs::as_frs`] when looking the parent record up in the index.
//!
//! # Invariants
//!
//! Carried by the type system:
//!
//! * `Copy + Eq + Hash + Ord` — both newtypes slot into `HashMap` / `BTreeMap`
//!   keys, compare cheaply, and pass by value.
//! * `ParentFrs` cannot be silently coerced to `Frs` (or vice versa) without an
//!   explicit `as_frs()` / `ParentFrs::of()` call.
//!
//! Not carried by the type system (kernel-issued / format-defined):
//!
//! * The numeric range is `0..=2^48-1` on real NTFS volumes; the newtype
//!   accepts any `u64` so we keep round-trip parity with the on-disk
//!   `MFT_SEGMENT_REFERENCE.segment_number_low_part + segment_number_high_part`
//!   48-bit encoding without rejecting FFI-sourced values defensively.

use core::fmt;

/// Typed File Record Segment number.
///
/// Newtype wrapper around the raw `u64` FRS used in `MFT_SEGMENT_REFERENCE`,
/// the on-disk `$FILE_NAME.parent_directory` field, and every parse /
/// index / query API.  We preserve the bit pattern byte-for-byte so
/// on-disk + on-wire formats are unchanged.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Frs(u64);

impl Frs {
    /// FRS `0` — the `$MFT` self-record on a real volume, and the
    /// canonical *"null reference"* sentinel for fields like
    /// `ParsedRecord.base_frs` on base (non-extension) records.
    ///
    /// Use [`Self::is_zero`] for the predicate.
    pub const ZERO: Self = Self(0);

    /// FRS `5` — the NTFS root directory (`.`).
    ///
    /// Every other directory's path traversal terminates at this
    /// record; exposing it as a constant keeps the magic number out
    /// of parent-chain walkers.
    pub const ROOT: Self = Self(5);

    /// Wrap a raw `u64` from a Win32 FFI buffer, on-disk parser, or
    /// serde wire decode.
    ///
    /// FRS values are kernel-issued — there is no client-side
    /// validation we can perform on a single value in isolation.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Underlying raw `u64`.
    ///
    /// Use this **only** at FFI / serialization boundaries or when
    /// indexing into a `Vec<FileRecord>` keyed by FRS.  Internal
    /// logic should compare [`Frs`] values directly via the derived
    /// [`Ord`] / [`PartialEq`] impls.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// `true` when this value equals [`Frs::ZERO`].
    ///
    /// Mirrors the historic `frs == 0` / `base_frs == 0` predicate
    /// used throughout the parse layer to detect *"no base record"*
    /// (i.e. this IS the base) and *"null parent reference"*.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// `true` when this value equals [`Frs::ROOT`] (FRS 5 — `.`).
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.0 == Self::ROOT.0
    }
}

impl From<u64> for Frs {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

impl From<Frs> for u64 {
    fn from(value: Frs) -> Self {
        value.0
    }
}

impl fmt::Display for Frs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Typed *parent-directory* FRS.
///
/// Composed over [`Frs`] so a parent reference and the record's own
/// reference are not interchangeable at the type level — fixing the
/// historic `(parent_frs: u64, own_frs: u64)` swap hazard.
///
/// To look the parent record up in the index, convert explicitly via
/// [`ParentFrs::as_frs`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct ParentFrs(Frs);

impl ParentFrs {
    /// Parent reference of `0` — the *"no parent"* sentinel used by
    /// the `$MFT` self-record and any record whose `$FILE_NAME`
    /// attribute was lost.
    pub const ZERO: Self = Self(Frs::ZERO);

    /// The root directory's parent — also FRS `5`, since the root is
    /// its own parent in NTFS.  Useful for parent-chain termination
    /// predicates.
    pub const ROOT: Self = Self(Frs::ROOT);

    /// Wrap a raw `u64` from the on-disk `$FILE_NAME.parent_directory`
    /// field or an FFI / wire buffer.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(Frs::new(raw))
    }

    /// Promote a typed [`Frs`] to a [`ParentFrs`] — used when the
    /// caller is *deliberately* asserting that an [`Frs`] value
    /// represents a parent directory (e.g. building a synthetic
    /// `$FILE_NAME` for a placeholder record).
    #[must_use]
    pub const fn of(frs: Frs) -> Self {
        Self(frs)
    }

    /// Extract the underlying [`Frs`] so this parent reference can be
    /// used as an index lookup key.  The conversion is intentionally
    /// explicit — it documents *"I am now looking up the parent
    /// record"* at the call site.
    #[must_use]
    pub const fn as_frs(self) -> Frs {
        self.0
    }

    /// Underlying raw `u64` — same caller-contract notes as
    /// [`Frs::raw`].
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0.raw()
    }

    /// `true` when this parent reference equals [`ParentFrs::ZERO`].
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// `true` when this parent reference equals [`ParentFrs::ROOT`]
    /// (FRS 5 — parent-chain termination on real NTFS volumes).
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.0.is_root()
    }
}

impl From<u64> for ParentFrs {
    fn from(raw: u64) -> Self {
        Self::new(raw)
    }
}

impl From<Frs> for ParentFrs {
    fn from(frs: Frs) -> Self {
        Self::of(frs)
    }
}

impl From<ParentFrs> for Frs {
    fn from(parent: ParentFrs) -> Self {
        parent.as_frs()
    }
}

impl From<ParentFrs> for u64 {
    fn from(parent: ParentFrs) -> Self {
        parent.raw()
    }
}

impl fmt::Display for ParentFrs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Frs, ParentFrs};

    #[test]
    fn frs_raw_roundtrip_preserves_u64_exactly() {
        // Wire-format contract: any `u64` the kernel hands us must come
        // back byte-identical via `raw()`.  Keeps the on-disk MFT
        // segment-reference decode path bit-for-bit compatible after
        // the newtype migration.
        for raw in [0_u64, 1, 5, 42, 1_u64 << 47_u32, u64::MAX] {
            assert_eq!(Frs::new(raw).raw(), raw, "Frs round-trip drift for {raw}");
        }
    }

    #[test]
    fn frs_sentinels_match_literal_values() {
        assert_eq!(Frs::ZERO, Frs::new(0));
        assert_eq!(Frs::ROOT, Frs::new(5));
        assert!(Frs::ZERO.is_zero());
        assert!(!Frs::ZERO.is_root());
        assert!(Frs::ROOT.is_root());
        assert!(!Frs::ROOT.is_zero());
    }

    #[test]
    fn frs_from_into_u64_symmetry() {
        let raw: u64 = 0x0123_4567_89AB_CDEF;
        let frs: Frs = raw.into();
        let back: u64 = frs.into();
        assert_eq!(back, raw);
    }

    #[test]
    fn frs_display_matches_raw_u64() {
        for raw in [0_u64, 1, 5, 1_234_567, u64::MAX] {
            assert_eq!(format!("{}", Frs::new(raw)), raw.to_string());
        }
    }

    #[test]
    fn frs_ord_matches_underlying_u64() {
        assert!(Frs::ZERO < Frs::new(1));
        assert!(Frs::new(1) < Frs::new(2));
        assert!(Frs::ROOT < Frs::new(6));
        assert!(Frs::ZERO < Frs::new(u64::MAX));
    }

    #[test]
    fn parent_frs_raw_roundtrip_preserves_u64_exactly() {
        for raw in [0_u64, 1, 5, 42, 1_u64 << 47_u32, u64::MAX] {
            assert_eq!(
                ParentFrs::new(raw).raw(),
                raw,
                "ParentFrs round-trip drift for {raw}"
            );
        }
    }

    #[test]
    fn parent_frs_of_and_as_frs_roundtrip() {
        // `ParentFrs::of(frs)` followed by `.as_frs()` must yield the
        // original [`Frs`] byte-identically — this is the documented
        // promotion / demotion path between own-record and
        // parent-record references.
        for raw in [0_u64, 5, 42, 1_u64 << 47_u32] {
            let frs = Frs::new(raw);
            let parent = ParentFrs::of(frs);
            assert_eq!(parent.as_frs(), frs);
            assert_eq!(parent.raw(), raw);
        }
    }

    #[test]
    fn parent_frs_sentinels_match_literal_values() {
        assert_eq!(ParentFrs::ZERO, ParentFrs::new(0));
        assert_eq!(ParentFrs::ROOT, ParentFrs::new(5));
        assert!(ParentFrs::ZERO.is_zero());
        assert!(!ParentFrs::ZERO.is_root());
        assert!(ParentFrs::ROOT.is_root());
        assert!(!ParentFrs::ROOT.is_zero());
    }

    #[test]
    fn parent_frs_display_matches_raw_u64() {
        for raw in [0_u64, 1, 5, 1_234_567, u64::MAX] {
            assert_eq!(format!("{}", ParentFrs::new(raw)), raw.to_string());
        }
    }

    #[test]
    fn parent_frs_into_frs_via_from() {
        // The `From<ParentFrs> for Frs` impl documents the *"I am
        // looking up the parent record now"* boundary.  Verify it
        // produces the same value as the explicit `.as_frs()` method.
        for raw in [0_u64, 5, 1_234_567] {
            let parent = ParentFrs::new(raw);
            let via_method = parent.as_frs();
            let via_from: Frs = parent.into();
            assert_eq!(via_method, via_from);
        }
    }

    #[test]
    fn parent_frs_does_not_coerce_to_frs_silently() {
        // Compile-time contract: this test exists primarily as
        // documentation.  The body asserts that the two newtypes are
        // distinct nominal types — any silent coercion would let the
        // assertion compile against the wrong side.
        let frs = Frs::new(42);
        let parent = ParentFrs::new(42);
        // Equality check is only available after explicit demotion.
        assert_eq!(parent.as_frs(), frs);
    }
}
