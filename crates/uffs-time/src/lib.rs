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

#![no_std]

/// Number of 100-nanosecond intervals per second.
pub const FILETIME_TICKS_PER_SECOND: i64 = 10_000_000;

/// Number of 100-nanosecond intervals per microsecond.
pub const FILETIME_TICKS_PER_MICROSECOND: i64 = 10;

/// Difference between the FILETIME epoch (1601-01-01) and the Unix epoch
/// (1970-01-01), in 100-nanosecond intervals.
pub const FILETIME_UNIX_DIFF: i64 = 116_444_736_000_000_000;

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

/// Decompose a raw FILETIME into calendar fields `(year, month, day, hour,
/// minute, second)`.
///
/// This mirrors the Windows `RtlTimeToTimeFields` approach — works directly
/// with FILETIME ticks (100-ns intervals since 1601-01-01), no intermediate
/// Unix conversion.  Handles all valid FILETIME values including pre-1970.
///
/// Returns `None` for `filetime == 0` (unset / null timestamp in NTFS).
#[must_use]
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "Hinnant algorithm: intermediate values are bounded and non-negative for valid dates"
)]
pub const fn filetime_to_calendar(filetime: i64) -> Option<(i32, u32, u32, u32, u32, u32)> {
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

    Some((year as i32, month, day, hour, minute, second))
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
    clippy::min_ident_chars,
    clippy::default_numeric_fallback,
    reason = "test code — relaxed linting for test clarity"
)]
mod tests {
    use super::{
        FILETIME_TICKS_PER_MICROSECOND, FILETIME_TICKS_PER_SECOND, FILETIME_UNIX_DIFF,
        filetime_to_calendar, filetime_to_unix_micros, filetime_with_tz_bias,
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
        assert_eq!(cal, Some((2024, 1, 1, 0, 0, 0)));
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
        assert_eq!(cal, Some((1959, 12, 2, 3, 45, 50)));
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
        assert_eq!(cal, Some((2024, 1, 1, 0, 0, 0)));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // filetime_to_calendar — additional edge cases
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn filetime_to_calendar_leap_day_2000() {
        // 2000-02-29 12:00:00 — Feb 29 in a century leap year.
        let unix_secs: i64 = 951_825_600; // 2000-02-29 12:00:00 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(filetime_to_calendar(ft), Some((2000, 2, 29, 12, 0, 0)));
    }

    #[test]
    fn filetime_to_calendar_leap_day_2024() {
        // 2024-02-29 23:59:59 — last second of a leap day.
        let unix_secs: i64 = 1_709_251_199; // 2024-02-29 23:59:59 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(filetime_to_calendar(ft), Some((2024, 2, 29, 23, 59, 59)));
    }

    #[test]
    fn filetime_to_calendar_non_leap_1900() {
        // 1900-02-28 — 1900 is NOT a leap year (divisible by 100 but not 400).
        let unix_secs: i64 = -2_203_977_600; // 1900-02-28 00:00:00 UTC
        let ft = unix_secs * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(filetime_to_calendar(ft), Some((1900, 2, 28, 0, 0, 0)));
    }

    #[test]
    fn filetime_to_calendar_year_boundary() {
        // 1999-12-31 23:59:59 → 2000-01-01 00:00:00 boundary
        let ft_dec31 = (946_684_799_i64) * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        let ft_jan01 = (946_684_800_i64) * FILETIME_TICKS_PER_SECOND + FILETIME_UNIX_DIFF;
        assert_eq!(
            filetime_to_calendar(ft_dec31),
            Some((1999, 12, 31, 23, 59, 59))
        );
        assert_eq!(filetime_to_calendar(ft_jan01), Some((2000, 1, 1, 0, 0, 0)));
    }

    #[test]
    fn filetime_to_calendar_filetime_epoch_itself() {
        // FILETIME = 1 tick → 1601-01-01 00:00:00 (essentially).
        let cal = filetime_to_calendar(1);
        assert_eq!(cal, Some((1601, 1, 1, 0, 0, 0)));
    }

    #[test]
    fn filetime_to_calendar_midnight_exact() {
        // Exactly midnight — time components must all be zero.
        let ft = 86400_i64 * FILETIME_TICKS_PER_SECOND; // day 1 since 1601
        let cal = filetime_to_calendar(ft);
        if let Some((_, _, _, h, m, s)) = cal {
            assert_eq!((h, m, s), (0, 0, 0), "midnight should have 00:00:00");
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
        assert_eq!(cal, Some((2024, 1, 1, 5, 0, 0)), "UTC+5 should show 05:00");
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
            Some((2023, 12, 31, 16, 0, 0)),
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
            Some((2024, 1, 1, 5, 30, 0)),
            "UTC+5:30 should show 05:30"
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
