// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! NTFS FILETIME arithmetic — pure `const fn`, zero dependencies.
//!
//! Windows stores timestamps as FILETIME: 100-nanosecond ticks since
//! 1601-01-01 UTC.  This crate provides the minimum helpers needed to:
//!
//! - Apply a timezone bias to a raw FILETIME ([`filetime_with_tz_bias`])
//! - Decompose a FILETIME into calendar fields ([`filetime_to_calendar`])
//! - Convert to Unix microseconds for legacy callers
//!   ([`filetime_to_unix_micros`])
//!
//! Extracted from `uffs_mft::ntfs` so the thin CLI can format timestamps
//! without pulling in the full MFT reader (which in turn depends on
//! `polars`, `tokio`, `reqwest`, and `object_store`).
//!
//! # Domain types ([`Filetime`], [`CalendarParts`])
//!
//! The two newtypes added in Phase 4 sub-phase 5a (per
//! `docs/dev/architecture/code_clean/phase_4_type_design_implementation_plan.
//! md` §4.1.C / §4.4) coexist with the existing `i64`-based free functions:
//!
//! - [`Filetime`] is a zero-cost (`#[repr(transparent)]`) wrapper around the
//!   raw 100-ns tick count.  Inherent methods mirror the free functions
//!   one-for-one so callers can opt into the newtype without having to convert
//!   at every boundary.
//! - [`CalendarParts`] is the named-struct replacement for the previous `(year,
//!   month, day, hour, minute, second)` 6-tuple return of
//!   [`filetime_to_calendar`].  The tuple shape was a textbook "primitive
//!   obsession" / "tuple where a named struct would be clearer" smell — every
//!   call site of the old API destructured the tuple in declaration order, with
//!   no compile-time guard against a swap.
//!
//! The free functions retain `i64`-based signatures because the
//! NTFS parser (`uffs-mft::raw_iocp`, `uffs-mft::parse`) reads FILETIME
//! ticks directly from on-disk structures as `i64` and would otherwise
//! pay an unnecessary `Filetime::from_ticks` ceremony at every read
//! site.  Future sub-phases may push [`Filetime`] deeper into the index
//! / query layers.

// On docs.rs only: enable the `doc_cfg` rustdoc feature so cfg-gated items
// (e.g. `#[cfg(feature = "...")]` or `#[cfg(target_os = "...")]`) render
// with their cfg badge on the rendered docs.  Gated behind `cfg(docsrs)`
// so local `cargo doc` (which doesn't pass `--cfg docsrs`) never exercises
// the nightly-only feature.  See `[package.metadata.docs.rs]` in Cargo.toml
// for the cfg wiring.  The previously-used `doc_auto_cfg` feature was
// merged into `doc_cfg` in Rust 1.92 (rust-lang/rust#138907); the unified
// `doc_cfg` feature preserves the automatic cfg-badge inference behaviour.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

/// Number of 100-nanosecond intervals per second.
pub const FILETIME_TICKS_PER_SECOND: i64 = 10_000_000;

/// Number of 100-nanosecond intervals per microsecond.
pub const FILETIME_TICKS_PER_MICROSECOND: i64 = 10;

/// Difference between the FILETIME epoch (1601-01-01) and the Unix epoch
/// (1970-01-01), in 100-nanosecond intervals.
pub const FILETIME_UNIX_DIFF: i64 = 116_444_736_000_000_000;

/// NTFS FILETIME: a 64-bit signed tick count of 100-nanosecond intervals
/// since 1601-01-01 UTC.
///
/// `Filetime` is `#[repr(transparent)]` over `i64`, so its in-memory
/// layout is identical to the underlying tick count and no conversion
/// cost is paid at the FFI / NTFS-parse boundary.
///
/// # Sentinel
///
/// FILETIME `0` is the documented "unset / null timestamp" sentinel in
/// the NTFS on-disk format.  [`Filetime::UNSET`] is the canonical
/// constant for it; [`Filetime::to_calendar`] returns `None` for
/// `UNSET` (consistent with the free [`filetime_to_calendar`]).
///
/// # Construction
///
/// The wrapper is intentionally unvalidated — any `i64` is a valid
/// `Filetime` at the type level.  Semantic correctness ("this value
/// came from a trusted NTFS source") is the caller's responsibility,
/// because validating every FILETIME at every read site would dominate
/// the index-build cost without adding real safety.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Filetime(i64);

impl Filetime {
    /// The "unset / null timestamp" sentinel — FILETIME `0`.
    ///
    /// `Filetime::to_calendar(Filetime::UNSET)` returns `None`,
    /// matching the long-standing free-function convention.
    pub const UNSET: Self = Self(0);

    /// Wrap a raw FILETIME tick count.
    ///
    /// No validation — the caller has established that `ticks` came
    /// from a trusted NTFS source (or constructed it from a known
    /// reference epoch via [`FILETIME_UNIX_DIFF`] +
    /// [`FILETIME_TICKS_PER_SECOND`]).
    #[inline]
    #[must_use]
    pub const fn from_ticks(ticks: i64) -> Self {
        Self(ticks)
    }

    /// Unwrap to the raw 100-ns tick count.
    ///
    /// Used at FFI / serde / Display boundaries where the API demands
    /// `i64`.  Pure-Rust code paths inside the workspace should prefer
    /// the inherent methods over `ticks()` + free function.
    #[inline]
    #[must_use]
    pub const fn ticks(self) -> i64 {
        self.0
    }

    /// Convert to Unix timestamp microseconds.
    ///
    /// Mirrors the free [`filetime_to_unix_micros`].  Returns `0` for
    /// `Filetime::UNSET` (matching the existing convention).
    #[inline]
    #[must_use]
    pub const fn to_unix_micros(self) -> i64 {
        filetime_to_unix_micros(self.0)
    }

    /// Apply a timezone bias in seconds, returning a new `Filetime`.
    ///
    /// Mirrors the free [`filetime_with_tz_bias`].
    #[inline]
    #[must_use]
    pub const fn with_tz_bias(self, tz_bias_secs: i32) -> Self {
        Self(filetime_with_tz_bias(self.0, tz_bias_secs))
    }

    /// Decompose into proleptic-Gregorian-UTC [`CalendarParts`].
    ///
    /// Mirrors the free [`filetime_to_calendar`].  Returns `None` for
    /// `Filetime::UNSET`.
    #[inline]
    #[must_use]
    pub const fn to_calendar(self) -> Option<CalendarParts> {
        filetime_to_calendar(self.0)
    }
}

/// Calendar breakdown of a [`Filetime`] in the proleptic Gregorian
/// calendar, UTC.
///
/// Returned by [`Filetime::to_calendar`] and [`filetime_to_calendar`].
/// Replaces the previous `(i32, u32, u32, u32, u32, u32)` 6-tuple
/// return whose declaration-order destructuring offered no compile-
/// time guard against a `(day, month, …)` swap at the call site.
///
/// # Field ranges
///
/// All values are produced by Howard Hinnant's civil-calendar algorithm
/// applied to a 64-bit signed tick count, so the ranges are:
///
/// - `year`: any `i32` (the algorithm's domain is unbounded by year); in
///   practice an NTFS FILETIME of `i64::MAX` decodes to ~year 30828 and
///   `i64::MIN` decodes to a year far enough negative to overflow `i32` —
///   callers reading those values are likely operating on corrupt data anyway.
/// - `month`: `1..=12`
/// - `day`: `1..=31`
/// - `hour`: `0..=23`
/// - `minute`: `0..=59`
/// - `second`: `0..=59` (no leap-second handling in NTFS time)
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct CalendarParts {
    /// Year — Gregorian, proleptic, signed (negative for BCE).
    pub year: i32,
    /// Month, 1-based: `1` = January … `12` = December.
    pub month: u32,
    /// Day of month, 1-based.
    pub day: u32,
    /// Hour, 0-based: `0..=23`.
    pub hour: u32,
    /// Minute, 0-based: `0..=59`.
    pub minute: u32,
    /// Second, 0-based: `0..=59` (no leap seconds).
    pub second: u32,
}

/// Converts a Windows FILETIME (100-nanosecond intervals since 1601-01-01)
/// to Unix timestamp in microseconds.
///
/// **Deprecated path** — prefer storing raw FILETIME values and using
/// [`filetime_to_calendar`] for display.  This function exists only for
/// backward compatibility during migration.
#[must_use]
pub const fn filetime_to_unix_micros(filetime: i64) -> i64 {
    if filetime == 0 {
        return 0;
    }
    (filetime - FILETIME_UNIX_DIFF) / FILETIME_TICKS_PER_MICROSECOND
}

/// Decompose a raw FILETIME into [`CalendarParts`] in proleptic
/// Gregorian UTC.
///
/// This mirrors the Windows `RtlTimeToTimeFields` approach — works directly
/// with FILETIME ticks (100-ns intervals since 1601-01-01), no intermediate
/// Unix conversion.  Handles all valid FILETIME values including pre-1970.
///
/// Returns `None` for `filetime == 0` (unset / null timestamp in NTFS).
///
/// Prefer [`Filetime::to_calendar`] in new code so the FILETIME argument
/// carries its semantic at the type level instead of being just another
/// `i64`.
#[must_use]
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "Hinnant algorithm: intermediate values are bounded and non-negative for valid dates"
)]
pub const fn filetime_to_calendar(filetime: i64) -> Option<CalendarParts> {
    if filetime == 0 {
        return None;
    }
    // Convert to total seconds since 1601-01-01, then split into days
    // and time-of-day using Euclidean division (remainder always ≥ 0).
    let total_secs = filetime / FILETIME_TICKS_PER_SECOND;
    let days_since_1601 = total_secs.div_euclid(86400);
    let day_secs = total_secs.rem_euclid(86400);

    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    // Hinnant algorithm expects days since 0000-03-01.
    //   719468 (0000-03-01 to 1970-01-01) − 134774 (1601-01-01 to 1970-01-01)
    //   = 584694
    let z = days_since_1601 + 584_694; // days since 0000-03-01
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    Some(CalendarParts {
        year: year as i32,
        month,
        day,
        hour,
        minute,
        second,
    })
}

/// Apply a timezone bias (in seconds) to a raw FILETIME value.
///
/// The bias is added as FILETIME ticks.  This is the FILETIME equivalent of
/// `system_time + time_zone_bias` in the C++ code.
#[must_use]
pub const fn filetime_with_tz_bias(filetime: i64, tz_bias_secs: i32) -> i64 {
    filetime + (tz_bias_secs as i64) * FILETIME_TICKS_PER_SECOND
}

#[cfg(test)]
#[expect(
    clippy::default_numeric_fallback,
    reason = "test code — integer literals are unambiguous in the surrounding assertions"
)]
mod tests {
    use super::{
        CalendarParts, FILETIME_TICKS_PER_MICROSECOND, FILETIME_TICKS_PER_SECOND,
        FILETIME_UNIX_DIFF, Filetime, filetime_to_calendar, filetime_to_unix_micros,
        filetime_with_tz_bias,
    };

    #[test]
    fn filetime_conversion() {
        let filetime: i64 = 133_485_408_000_000_000;
        let unix_micros = filetime_to_unix_micros(filetime);
        assert_eq!(unix_micros, 1_704_067_200_000_000);
    }

    #[test]
    fn filetime_to_calendar_post_1970() {
        // 2024-01-01 00:00:00 UTC
        let filetime: i64 = 133_485_408_000_000_000;
        let cal = filetime_to_calendar(filetime);
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_pre_1970() {
        // 1959-12-02 03:45:50 UTC — the exact case from the parity baseline.
        // From Dec 2, 1959 00:00:00 to Jan 1, 1970 00:00:00:
        //   1960-1969 = 10 years = 7*365 + 3*366 = 3653 days
        //   Dec 2, 1959 to Jan 1, 1960 = 30 days
        //   Total = 3683 days → unix_secs at midnight = -318_211_200
        //   Plus 3h45m50s = 13550s → -318_197_650
        let unix_secs: i64 = -318_197_650;
        let filetime = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        let cal = filetime_to_calendar(filetime);
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 1959,
                month: 12,
                day: 2,
                hour: 3,
                minute: 45,
                second: 50,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_zero_is_none() {
        assert_eq!(filetime_to_calendar(0), None);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // filetime_to_unix_micros — edge cases
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn filetime_to_unix_micros_zero_returns_zero() {
        // FILETIME 0 means "unset" — must map to 0, not a 1601 date.
        assert_eq!(filetime_to_unix_micros(0), 0);
    }

    #[test]
    fn filetime_to_unix_micros_unix_epoch() {
        // FILETIME at the exact Unix epoch (1970-01-01 00:00:00).
        assert_eq!(filetime_to_unix_micros(FILETIME_UNIX_DIFF), 0);
    }

    #[test]
    fn filetime_to_unix_micros_pre_1970() {
        // 1960-01-01 00:00:00 — 10 years before Unix epoch.
        // Leap years 1960,1964,1968 → 3*366 + 7*365 = 3653 days.
        let ft_1960 = FILETIME_UNIX_DIFF - 3653 * 86400 * FILETIME_TICKS_PER_SECOND;
        let us = filetime_to_unix_micros(ft_1960);
        assert_eq!(us, -315_619_200_000_000);
        assert!(us < 0, "pre-1970 dates must produce negative unix micros");
    }

    #[test]
    fn filetime_to_unix_micros_filetime_epoch() {
        // FILETIME = 1 (one 100ns tick after 1601-01-01 00:00:00).
        // Should produce a large negative Unix µs (roughly -11644473600 seconds).
        let us = filetime_to_unix_micros(1);
        assert!(
            us < -11_000_000_000_000_000,
            "1601 date should be far negative"
        );
    }

    #[test]
    fn filetime_to_unix_micros_y2k() {
        // 2000-01-01 00:00:00 — 30 years, well-known reference.
        let expected_us: i64 = 946_684_800_000_000;
        let ft = FILETIME_UNIX_DIFF + expected_us * FILETIME_TICKS_PER_MICROSECOND;
        assert_eq!(filetime_to_unix_micros(ft), expected_us);
    }

    #[test]
    fn filetime_to_unix_micros_roundtrip_with_calendar() {
        // Verify that filetime → unix_micros and filetime → calendar agree.
        let ft_2024: i64 = 133_485_408_000_000_000; // 2024-01-01 00:00:00
        let us = filetime_to_unix_micros(ft_2024);
        let cal = filetime_to_calendar(ft_2024);

        // Unix micros → days → should match calendar day
        let days_since_epoch = us / (86_400 * 1_000_000);
        assert_eq!(days_since_epoch, 19723); // 2024-01-01 is day 19723
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // filetime_to_calendar — additional edge cases
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn filetime_to_calendar_leap_day_2000() {
        // 2000-02-29 12:00:00 — Feb 29 in a century leap year.
        let unix_secs: i64 = 951_825_600; // 2000-02-29 12:00:00 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(
            filetime_to_calendar(ft),
            Some(CalendarParts {
                year: 2000,
                month: 2,
                day: 29,
                hour: 12,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_leap_day_2024() {
        // 2024-02-29 23:59:59 — last second of a leap day.
        let unix_secs: i64 = 1_709_251_199; // 2024-02-29 23:59:59 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(
            filetime_to_calendar(ft),
            Some(CalendarParts {
                year: 2024,
                month: 2,
                day: 29,
                hour: 23,
                minute: 59,
                second: 59,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_non_leap_1900() {
        // 1900-02-28 — 1900 is NOT a leap year (divisible by 100 but not 400).
        let unix_secs: i64 = -2_203_977_600; // 1900-02-28 00:00:00 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(
            filetime_to_calendar(ft),
            Some(CalendarParts {
                year: 1900,
                month: 2,
                day: 28,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_year_boundary() {
        // 1999-12-31 23:59:59 → 2000-01-01 00:00:00 boundary
        let ft_dec31 = (946_684_799_i64) * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        let ft_jan01 = (946_684_800_i64) * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(
            filetime_to_calendar(ft_dec31),
            Some(CalendarParts {
                year: 1999,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
            })
        );
        assert_eq!(
            filetime_to_calendar(ft_jan01),
            Some(CalendarParts {
                year: 2000,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_filetime_epoch_itself() {
        // FILETIME = 1 tick → 1601-01-01 00:00:00 (essentially).
        let cal = filetime_to_calendar(1);
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 1601,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_to_calendar_midnight_exact() {
        // Exactly midnight — time components must all be zero.
        let ft = 86400_i64 * FILETIME_TICKS_PER_SECOND; // day 1 since 1601
        if let Some(CalendarParts {
            hour,
            minute,
            second,
            ..
        }) = filetime_to_calendar(ft)
        {
            assert_eq!(
                (hour, minute, second),
                (0, 0, 0),
                "midnight should have 00:00:00"
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // filetime_with_tz_bias — timezone handling
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn filetime_with_tz_bias_zero_no_change() {
        let ft: i64 = 133_485_408_000_000_000;
        assert_eq!(filetime_with_tz_bias(ft, 0), ft);
    }

    #[test]
    fn filetime_with_tz_bias_positive_east() {
        // UTC+5 (e.g. Yekaterinburg) — 5 hours ahead.
        let ft: i64 = 133_485_408_000_000_000; // 2024-01-01 00:00:00 UTC
        let biased = filetime_with_tz_bias(ft, 5 * 3600);
        let cal = filetime_to_calendar(biased);
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 2024,
                month: 1,
                day: 1,
                hour: 5,
                minute: 0,
                second: 0,
            }),
            "UTC+5 should show 05:00"
        );
    }

    #[test]
    fn filetime_with_tz_bias_negative_west() {
        // UTC-8 (US Pacific) — 8 hours behind.
        let ft: i64 = 133_485_408_000_000_000; // 2024-01-01 00:00:00 UTC
        let biased = filetime_with_tz_bias(ft, -8 * 3600);
        let cal = filetime_to_calendar(biased);
        // 00:00 - 8h = previous day 16:00
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 2023,
                month: 12,
                day: 31,
                hour: 16,
                minute: 0,
                second: 0,
            }),
            "UTC-8 should roll back to Dec 31"
        );
    }

    #[test]
    fn filetime_with_tz_bias_half_hour() {
        // UTC+5:30 (India) — non-integer hour offset.
        let ft: i64 = 133_485_408_000_000_000; // 2024-01-01 00:00:00 UTC
        let biased = filetime_with_tz_bias(ft, 5 * 3600 + 1800);
        let cal = filetime_to_calendar(biased);
        assert_eq!(
            cal,
            Some(CalendarParts {
                year: 2024,
                month: 1,
                day: 1,
                hour: 5,
                minute: 30,
                second: 0,
            }),
            "UTC+5:30 should show 05:30"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Filetime newtype + CalendarParts — Phase 4 sub-phase 5a tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn filetime_unset_round_trip() {
        assert_eq!(Filetime::UNSET.ticks(), 0);
        assert_eq!(Filetime::from_ticks(0), Filetime::UNSET);
        assert_eq!(Filetime::UNSET.to_calendar(), None);
        assert_eq!(Filetime::UNSET.to_unix_micros(), 0);
    }

    #[test]
    fn filetime_newtype_to_calendar_matches_free_fn() {
        // Newtype path and free-fn path MUST produce byte-identical output.
        let ft: i64 = 133_485_408_000_000_000;
        let via_newtype = Filetime::from_ticks(ft).to_calendar();
        let via_free = filetime_to_calendar(ft);
        assert_eq!(via_newtype, via_free);
        assert_eq!(
            via_newtype,
            Some(CalendarParts {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn filetime_newtype_with_tz_bias_returns_filetime() {
        let utc = Filetime::from_ticks(133_485_408_000_000_000);
        let pst = utc.with_tz_bias(-8 * 3600);
        assert_eq!(
            pst.to_calendar(),
            Some(CalendarParts {
                year: 2023,
                month: 12,
                day: 31,
                hour: 16,
                minute: 0,
                second: 0,
            })
        );
        // Symmetric round-trip.
        assert_eq!(pst.with_tz_bias(8 * 3600), utc);
    }

    #[test]
    fn filetime_newtype_repr_transparent_size() {
        // `#[repr(transparent)]` over `i64` guarantees identical layout.
        // Verified at compile time via `size_of` and `align_of` (both in
        // the 2024 edition prelude — no `core::mem::` qualifier needed).
        assert_eq!(size_of::<Filetime>(), size_of::<i64>());
        assert_eq!(align_of::<Filetime>(), align_of::<i64>());
    }

    #[test]
    fn calendar_parts_field_layout_matches_doc_ranges() {
        // The 2024-02-29 leap-day case exercises every field independently.
        let unix_secs: i64 = 1_709_251_199; // 2024-02-29 23:59:59 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        let parts = Filetime::from_ticks(ft).to_calendar().expect("non-zero");
        assert_eq!(parts.year, 2024);
        assert_eq!(parts.month, 2);
        assert_eq!(parts.day, 29);
        assert_eq!(parts.hour, 23);
        assert_eq!(parts.minute, 59);
        assert_eq!(parts.second, 59);
    }

    #[test]
    fn filetime_to_unix_micros_via_newtype() {
        let ft: i64 = 133_485_408_000_000_000;
        assert_eq!(
            Filetime::from_ticks(ft).to_unix_micros(),
            filetime_to_unix_micros(ft)
        );
    }

    #[test]
    fn filetime_with_tz_bias_symmetric_round_trip() {
        // Reversing a bias must recover the original FILETIME exactly.
        let base = FILETIME_UNIX_DIFF;
        let shifted = filetime_with_tz_bias(base, 3600);
        assert_eq!(shifted - base, 3600 * FILETIME_TICKS_PER_SECOND);
        assert_eq!(filetime_with_tz_bias(shifted, -3600), base);
    }
}
