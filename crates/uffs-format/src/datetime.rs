// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! FILETIME → `"YYYY-MM-DD HH:MM:SS"` formatting for the CSV writer.

use core::fmt::Write as _;

/// Append `YYYY-MM-DD HH:MM:SS` from a raw FILETIME (100-ns ticks
/// since 1601-01-01) with the supplied timezone offset.
///
/// v13+ of the compact index stores timestamps as **raw FILETIME**
/// (matching the C++ NTFS baseline), not Unix microseconds.  This
/// function interprets its argument as FILETIME; the bias is applied
/// in FILETIME ticks (matching C++ `FileTimeToLocalFileTime`),
/// then the result is decomposed via the Hinnant civil calendar in
/// [`uffs_time::filetime_to_calendar`].
///
/// A `filetime` of 0 (the "unset / null" sentinel) formats as
/// `"0000-00-00 00:00:00"` instead of decomposing to a 1601 date, so
/// callers (and users reading CSV output) can recognise missing
/// timestamps.
pub(crate) fn append_datetime_native(buf: &mut String, filetime: i64, tz_offset_secs: i32) {
    let local_ft = uffs_time::filetime_with_tz_bias(filetime, tz_offset_secs);
    if let Some((year, month, day, hour, minute, second)) =
        uffs_time::filetime_to_calendar(local_ft)
    {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "`String::write_fmt` is infallible"
        )]
        let _ = write!(
            buf,
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        );
    } else {
        buf.push_str("0000-00-00 00:00:00");
    }
}

#[cfg(test)]
mod tests {
    use super::append_datetime_native;

    /// Pin the FILETIME-as-FILETIME invariant — a 2024-01-01 anchor
    /// must NOT decompose to a year-6220 value (that was the symptom
    /// of the old Unix-micros bug).
    #[test]
    fn filetime_2024_utc_decomposes_correctly() {
        let ft_2024: i64 = 133_485_408_000_000_000;
        let mut buf = String::new();
        append_datetime_native(&mut buf, ft_2024, 0);
        assert_eq!(buf, "2024-01-01 00:00:00");
    }

    /// Zero FILETIME must surface as the all-zero sentinel.
    #[test]
    fn zero_filetime_is_zero_sentinel() {
        let mut buf = String::new();
        append_datetime_native(&mut buf, 0, 0);
        assert_eq!(buf, "0000-00-00 00:00:00");
    }

    /// TZ bias is applied in FILETIME ticks before the calendar
    /// decomposition.
    #[test]
    fn pst_offset_rolls_back_to_previous_day() {
        let ft_2024: i64 = 133_485_408_000_000_000;
        let pst_offset: i32 = -8 * 3600;
        let mut buf = String::new();
        append_datetime_native(&mut buf, ft_2024, pst_offset);
        assert_eq!(buf, "2023-12-31 16:00:00");
    }
}
