// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Time-bound parsing: durations, ISO dates, named ranges, and months.

/// Extracts the 1-based month number from a raw FILETIME timestamp.
#[must_use]
pub const fn month_from_filetime(filetime: i64) -> u32 {
    match uffs_time::filetime_to_calendar(filetime) {
        Some((_, month, ..)) => month,
        None => 1, // default to January for unset timestamps
    }
}

/// Backward-compatible alias — callers that still say `month_from_unix_micros`
/// will compile, but the semantics now expect raw FILETIME input.
#[must_use]
pub const fn month_from_unix_micros(filetime: i64) -> u32 {
    month_from_filetime(filetime)
}

/// Parse a month/quarter spec into a vector of allowed months (1-12).
///
/// Accepts:
/// - Month names: `january`, `jan`, `february`, `feb`, … , `december`, `dec`
/// - Quarter names: `Q1`, `Q2`, `Q3`, `Q4`
/// - Comma-separated combinations: `jan,feb`, `Q1,Q3`
///
/// ```
/// # use uffs_core::search::filters::parse_month_spec;
/// assert_eq!(parse_month_spec("january"), vec![1]);
/// assert_eq!(parse_month_spec("Q1"), vec![1, 2, 3]);
/// assert_eq!(parse_month_spec("jan,feb"), vec![1, 2]);
/// assert_eq!(parse_month_spec("Q2,october"), vec![4, 5, 6, 10]);
/// ```
#[must_use]
pub fn parse_month_spec(spec: &str) -> Vec<u32> {
    let mut months = Vec::new();
    for token in spec.split(',') {
        let lower = token.trim().to_ascii_lowercase();
        match lower.as_str() {
            "january" | "jan" => months.push(1),
            "february" | "feb" => months.push(2),
            "march" | "mar" => months.push(3),
            "april" | "apr" => months.push(4),
            "may" => months.push(5),
            "june" | "jun" => months.push(6),
            "july" | "jul" => months.push(7),
            "august" | "aug" => months.push(8),
            "september" | "sep" => months.push(9),
            "october" | "oct" => months.push(10),
            "november" | "nov" => months.push(11),
            "december" | "dec" => months.push(12),
            "q1" => months.extend_from_slice(&[1, 2, 3]),
            "q2" => months.extend_from_slice(&[4, 5, 6]),
            "q3" => months.extend_from_slice(&[7, 8, 9]),
            "q4" => months.extend_from_slice(&[10, 11, 12]),
            _ => {} // silently ignore unknown tokens
        }
    }
    months.sort_unstable();
    months.dedup();
    months
}

// ═══════════════════════════════════════════════════════════════════════════
// Size parsing helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Parse a human-readable size string into bytes.
///
/// Accepts plain integers (bytes) and suffixes: `B`, `KB`, `MB`, `GB`, `TB`.
/// The suffix is **case-insensitive**.  A bare number with no suffix is
/// treated as bytes.
///
/// # Errors
///
/// Returns `Err` if the spec is empty, contains non-numeric characters
/// (after stripping the suffix), or the result overflows `u64`.
///
/// # Examples
///
/// ```
/// # use uffs_core::search::filters::parse_size;
/// assert_eq!(parse_size("1024"), Ok(1024));
/// assert_eq!(parse_size("1KB"), Ok(1024));
/// assert_eq!(parse_size("10mb"), Ok(10 * 1024 * 1024));
/// assert_eq!(parse_size("1GB"), Ok(1024 * 1024 * 1024));
/// assert_eq!(parse_size("2TB"), Ok(2 * 1024 * 1024 * 1024 * 1024));
/// assert_eq!(parse_size("0"), Ok(0));
/// assert!(parse_size("abc").is_err());
/// ```
pub fn parse_size(spec: &str) -> Result<u64, String> {
    // Suffix table: longest-first to avoid prefix ambiguity.
    const SUFFIXES: &[(&str, u64)] = &[
        ("TB", 1024 * 1024 * 1024 * 1024),
        ("GB", 1024 * 1024 * 1024),
        ("MB", 1024 * 1024),
        ("KB", 1024),
        ("B", 1),
    ];

    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("empty size specification".to_owned());
    }

    let upper = trimmed.to_ascii_uppercase();

    let (digits, multiplier) = SUFFIXES
        .iter()
        .find_map(|(suffix, mult)| upper.strip_suffix(suffix).map(|rest| (rest, *mult)))
        .unwrap_or((upper.as_str(), 1));

    let count: u64 = digits
        .trim()
        .parse()
        .map_err(|_parse_err| format!("invalid size: {spec}"))?;

    count
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size overflows u64: {spec}"))
}

// ═══════════════════════════════════════════════════════════════════════════
// Time / attribute parsing helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Current time as a raw FILETIME (100-ns ticks since 1601-01-01).
#[must_use]
pub fn now_filetime() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |dur| {
            let unix_us = uffs_mft::micros_to_i64(dur.as_micros());
            unix_us * uffs_time::FILETIME_TICKS_PER_MICROSECOND + uffs_time::FILETIME_UNIX_DIFF
        })
}

/// Backward-compatible alias.  Semantics changed: returns FILETIME, not
/// Unix microseconds.  All callers compare against FILETIME-valued fields.
#[must_use]
pub fn now_unix_micros() -> i64 {
    now_filetime()
}

/// Parse a time bound string into a raw FILETIME value.
///
/// Supports:
/// - **Duration:** `7d`, `24h`, `30m`, `90s`, `2w`
/// - **ISO date:** `2026-01-15`
/// - **Named ranges:** `today`, `yesterday`, `this_week`, `last_week`,
///   `this_month`, `last_month`, `this_year`, `last_year`, `last_7d`,
///   `last_30d`, `last_90d`, `last_365d`, `ytd`
#[must_use]
pub fn parse_time_bound(spec: &str, now_ft: i64, is_newer: bool) -> Option<i64> {
    let trimmed = spec.trim();

    // ── Named time ranges ──────────────────────────────────────────
    if let Some(ts) = parse_named_time_range(trimmed, now_ft, is_newer) {
        return Some(ts);
    }

    // ── Duration suffix (e.g. "7d", "24h") ─────────────────────────
    if trimmed.len() >= 2 {
        let (num_str, suffix) = trimmed.split_at(trimmed.len() - 1);
        if let Ok(count) = num_str.parse::<i64>() {
            let delta = match suffix {
                "s" => count * TICKS_PER_SECOND,
                "m" => count * 60 * TICKS_PER_SECOND,
                "h" => count * 3600 * TICKS_PER_SECOND,
                "d" => count * TICKS_PER_DAY,
                "w" => count * 7 * TICKS_PER_DAY,
                _ => return None,
            };
            return Some(now_ft - delta);
        }
    }

    // ── ISO date (YYYY-MM-DD) ──────────────────────────────────────
    parse_iso_date(trimmed)
}

/// Parse an ISO date string (`YYYY-MM-DD`) into a FILETIME at midnight UTC.
fn parse_iso_date(trimmed: &str) -> Option<i64> {
    if trimmed.len() == 10 && trimmed.as_bytes().get(4) == Some(&b'-') {
        let parts: Vec<&str> = trimmed.split('-').collect();
        if let [year_s, month_s, day_s] = parts.as_slice()
            && let (Ok(year), Ok(month), Ok(day)) = (
                year_s.parse::<i64>(),
                month_s.parse::<i64>(),
                day_s.parse::<i64>(),
            )
        {
            let days = ymd_to_days_1601(year, month, day);
            return Some(days * TICKS_PER_DAY);
        }
    }
    None
}

/// FILETIME ticks per second (100-ns intervals).
const TICKS_PER_SECOND: i64 = uffs_time::FILETIME_TICKS_PER_SECOND;

/// FILETIME ticks per day.
const TICKS_PER_DAY: i64 = TICKS_PER_SECOND * 86_400;

/// Resolve a named time range to a FILETIME value.
///
/// For `is_newer = true`, returns the start of the range (lower bound).
/// For `is_newer = false`, returns the end of the range (upper bound).
#[expect(
    clippy::too_many_lines,
    reason = "single match with ~20 named time ranges; each arm is self-contained"
)]
fn parse_named_time_range(name: &str, now_ft: i64, is_newer: bool) -> Option<i64> {
    let today_start = now_ft - (now_ft % TICKS_PER_DAY);

    match name.to_ascii_lowercase().as_str() {
        "today" => Some(today_start),
        "yesterday" => {
            if is_newer {
                Some(today_start - TICKS_PER_DAY)
            } else {
                Some(today_start)
            }
        }
        "this_week" | "thisweek" => {
            // FILETIME epoch 1601-01-01 was a Monday, so day 0 = Monday.
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let dow = days_since_1601 % 7; // 0=Mon, 6=Sun
            Some(today_start - dow * TICKS_PER_DAY)
        }
        "last_week" | "lastweek" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let dow = days_since_1601 % 7;
            let this_monday = today_start - dow * TICKS_PER_DAY;
            if is_newer {
                Some(this_monday - 7 * TICKS_PER_DAY)
            } else {
                Some(this_monday)
            }
        }
        "this_month" | "thismonth" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (_, _, day) = days_to_ymd_1601(days_since_1601);
            Some(today_start - (day - 1) * TICKS_PER_DAY)
        }
        "last_month" | "lastmonth" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (year, month, day) = days_to_ymd_1601(days_since_1601);
            let this_month_start = today_start - (day - 1) * TICKS_PER_DAY;
            if is_newer {
                let (prev_year, prev_month) = if month == 1 {
                    (year - 1, 12)
                } else {
                    (year, month - 1)
                };
                let prev_days = days_in_month(prev_year, prev_month);
                Some(this_month_start - prev_days * TICKS_PER_DAY)
            } else {
                Some(this_month_start)
            }
        }
        "this_year" | "thisyear" | "ytd" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (year, _, _) = days_to_ymd_1601(days_since_1601);
            Some(ymd_to_days_1601(year, 1, 1) * TICKS_PER_DAY)
        }
        "last_year" | "lastyear" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (year, _, _) = days_to_ymd_1601(days_since_1601);
            if is_newer {
                Some(ymd_to_days_1601(year - 1, 1, 1) * TICKS_PER_DAY)
            } else {
                Some(ymd_to_days_1601(year, 1, 1) * TICKS_PER_DAY)
            }
        }
        "last_7d" | "last7d" => Some(now_ft - 7 * TICKS_PER_DAY),
        "last_30d" | "last30d" => Some(now_ft - 30 * TICKS_PER_DAY),
        "last_90d" | "last90d" => Some(now_ft - 90 * TICKS_PER_DAY),
        "last_365d" | "last365d" => Some(now_ft - 365 * TICKS_PER_DAY),
        "next_day" | "nextday" | "tomorrow" => {
            if is_newer {
                Some(today_start + TICKS_PER_DAY)
            } else {
                Some(today_start + 2 * TICKS_PER_DAY)
            }
        }
        "next_week" | "nextweek" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let dow = days_since_1601 % 7;
            let this_monday = today_start - dow * TICKS_PER_DAY;
            let next_monday = this_monday + 7 * TICKS_PER_DAY;
            if is_newer {
                Some(next_monday)
            } else {
                Some(next_monday + 7 * TICKS_PER_DAY)
            }
        }
        "next_month" | "nextmonth" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (year, month, day) = days_to_ymd_1601(days_since_1601);
            let this_month_start = today_start - (day - 1) * TICKS_PER_DAY;
            let days = days_in_month(year, month);
            let next_month_start = this_month_start + days * TICKS_PER_DAY;
            if is_newer {
                Some(next_month_start)
            } else {
                let (ny, nm) = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                Some(next_month_start + days_in_month(ny, nm) * TICKS_PER_DAY)
            }
        }
        "next_year" | "nextyear" => {
            let days_since_1601 = today_start / TICKS_PER_DAY;
            let (year, _, _) = days_to_ymd_1601(days_since_1601);
            if is_newer {
                Some(ymd_to_days_1601(year + 1, 1, 1) * TICKS_PER_DAY)
            } else {
                Some(ymd_to_days_1601(year + 2, 1, 1) * TICKS_PER_DAY)
            }
        }
        _ => None,
    }
}

/// Convert days since FILETIME epoch (1601-01-01) to (year, month, day).
///
/// Uses the Hinnant algorithm. 1601-01-01 = day 584389 since 0000-03-01.
fn days_to_ymd_1601(days_since_1601: i64) -> (i64, i64, i64) {
    let z = days_since_1601 + 584_694; // days since 0000-03-01
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    // `z - era * 146_097` is bounded by the era division to
    // `[0, 146_096]`; `try_from` is the lint-free way to narrow it
    // to u32 (saturating fallback is unreachable in practice).
    let doe = u32::try_from(z - era * 146_097).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = i64::from(doy - (153 * mp + 2) / 5 + 1);
    let month = i64::from(if mp < 10 { mp + 3 } else { mp - 9 });
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Days in a given month (1-indexed).
const fn days_in_month(year: i64, month: i64) -> i64 {
    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        2 => {
            if is_leap {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Convert (year, month, day) to days since FILETIME epoch (1601-01-01).
///
/// Inverse of `days_to_ymd_1601`.
fn ymd_to_days_1601(year: i64, month: i64, day: i64) -> i64 {
    // Shift to March-based year for the Hinnant inverse algorithm.
    let (adj_year, adj_month) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = (if adj_year >= 0 {
        adj_year
    } else {
        adj_year - 399
    }) / 400;
    // All three narrowings below are bounded by the algorithm:
    // `adj_year - era * 400` lives in `[0, 399]`, `adj_month` in
    // `[0, 11]`, and `day` in `[1, 31]`.  `try_from` is the lint-free
    // narrowing pattern; the saturating fallback is unreachable for
    // any valid civil date.
    let yoe = u32::try_from(adj_year - era * 400).unwrap_or(0);
    let adj_month_u32 = u32::try_from(adj_month).unwrap_or(0);
    let day_u32 = u32::try_from(day).unwrap_or(1);
    let doy = (153 * adj_month_u32 + 2) / 5 + day_u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = i64::from(doe) + era * 146_097;
    days - 584_694 // convert from 0000-03-01 epoch to 1601-01-01 epoch
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════════
    // ymd_to_days_1601 / days_to_ymd_1601 roundtrip
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn ymd_days_roundtrip_post_1970() {
        let days = ymd_to_days_1601(2024, 6, 15);
        let (year, month, day) = days_to_ymd_1601(days);
        assert_eq!((year, month, day), (2024_i64, 6_i64, 15_i64));
    }

    #[test]
    fn ymd_days_roundtrip_pre_1970() {
        let days = ymd_to_days_1601(1959, 12, 2);
        let (year, month, day) = days_to_ymd_1601(days);
        assert_eq!((year, month, day), (1959_i64, 12_i64, 2_i64));
    }

    #[test]
    fn ymd_days_roundtrip_1601_epoch() {
        let days = ymd_to_days_1601(1601, 1, 1);
        assert_eq!(days, 0_i64, "1601-01-01 should be day 0 in FILETIME epoch");
        let (year, month, day) = days_to_ymd_1601(0_i64);
        assert_eq!((year, month, day), (1601_i64, 1_i64, 1_i64));
    }

    #[test]
    fn ymd_days_roundtrip_leap_day() {
        let days = ymd_to_days_1601(2000, 2, 29);
        let (year, month, day) = days_to_ymd_1601(days);
        assert_eq!((year, month, day), (2000_i64, 2_i64, 29_i64));
    }

    #[test]
    fn ymd_days_roundtrip_non_leap_1900() {
        let feb28 = ymd_to_days_1601(1900, 2, 28);
        let march1 = ymd_to_days_1601(1900, 3, 1);
        assert_eq!(
            march1 - feb28,
            1_i64,
            "no Feb 29 in 1900: Mar 1 should be 1 day after Feb 28"
        );
    }

    #[test]
    fn ymd_days_roundtrip_year_boundary() {
        let dec31 = ymd_to_days_1601(1999, 12, 31);
        let jan01 = ymd_to_days_1601(2000, 1, 1);
        assert_eq!(jan01 - dec31, 1_i64, "Jan 1 should be 1 day after Dec 31");
    }

    #[test]
    fn ymd_days_roundtrip_all_months_2024() {
        for month in 1_i64..=12_i64 {
            let days = ymd_to_days_1601(2024, month, 1);
            let (year, mon, day) = days_to_ymd_1601(days);
            assert_eq!(
                (year, mon, day),
                (2024_i64, month, 1_i64),
                "roundtrip failed for 2024-{month:02}-01"
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // parse_time_bound — pre-1970 and edge cases
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn parse_time_bound_1601_epoch() {
        let result = parse_time_bound("1601-01-01", 0_i64, true);
        assert!(result.is_some(), "1601-01-01 should parse");
        assert_eq!(result.unwrap(), 0_i64, "1601-01-01 should be FILETIME 0");
    }

    #[test]
    fn parse_time_bound_leap_day_iso() {
        let result = parse_time_bound("2000-02-29", 0_i64, true);
        assert!(
            result.is_some(),
            "2000-02-29 should parse as valid leap day"
        );
    }

    #[test]
    fn parse_time_bound_duration_weeks() {
        let now = 100_i64 * TICKS_PER_DAY;
        let result = parse_time_bound("2w", now, true).unwrap();
        assert_eq!(result, now - 14_i64 * TICKS_PER_DAY);
    }
}
