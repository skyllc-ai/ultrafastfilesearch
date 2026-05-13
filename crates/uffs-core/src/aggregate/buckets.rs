// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bucket classification functions for histogram and range aggregations.
//!
//! Each classifier maps a raw value to a bucket index and provides
//! human-readable labels for each bucket.

/// Size bucket classification.
///
/// Default boundaries (§9.3 of the architecture doc):
/// - Tiny:   0 – 1 KB
/// - Small:  1 KB – 100 KB
/// - Medium: 100 KB – 1 MB
/// - Large:  1 MB – 100 MB
/// - Huge:   100 MB – 1 GB
/// - Giant:  1 GB – 10 GB
/// - Colossal: > 10 GB
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum SizeBucket {
    /// 0 – 1 023 bytes.
    Tiny = 0,
    /// 1 024 – 102 399 bytes.
    Small = 1,
    /// 102 400 – 1 048 575 bytes.
    Medium = 2,
    /// 1 048 576 – 104 857 599 bytes.
    Large = 3,
    /// 104 857 600 – 1 073 741 823 bytes.
    Huge = 4,
    /// 1 073 741 824 – 10 737 418 239 bytes.
    Giant = 5,
    /// ≥ 10 737 418 240 bytes.
    Colossal = 6,
}

/// Number of size bucket variants.
pub(crate) const SIZE_BUCKET_COUNT: usize = 7;

/// Size bucket boundary upper limits (exclusive).
const SIZE_BOUNDARIES: [u64; 6] = [
    1_024,          // < 1 KB
    102_400,        // < 100 KB
    1_048_576,      // < 1 MB
    104_857_600,    // < 100 MB
    1_073_741_824,  // < 1 GB
    10_737_418_240, // < 10 GB
];

impl SizeBucket {
    /// Classify a byte size into a bucket.
    #[inline]
    #[must_use]
    pub const fn classify(size: u64) -> Self {
        if size < SIZE_BOUNDARIES[0] {
            Self::Tiny
        } else if size < SIZE_BOUNDARIES[1] {
            Self::Small
        } else if size < SIZE_BOUNDARIES[2] {
            Self::Medium
        } else if size < SIZE_BOUNDARIES[3] {
            Self::Large
        } else if size < SIZE_BOUNDARIES[4] {
            Self::Huge
        } else if size < SIZE_BOUNDARIES[5] {
            Self::Giant
        } else {
            Self::Colossal
        }
    }

    /// Human-readable label for this bucket.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tiny => "0 – 1 KB",
            Self::Small => "1 KB – 100 KB",
            Self::Medium => "100 KB – 1 MB",
            Self::Large => "1 MB – 100 MB",
            Self::Huge => "100 MB – 1 GB",
            Self::Giant => "1 GB – 10 GB",
            Self::Colossal => "> 10 GB",
        }
    }

    /// All bucket variants in order.
    pub const ALL: [Self; SIZE_BUCKET_COUNT] = [
        Self::Tiny,
        Self::Small,
        Self::Medium,
        Self::Large,
        Self::Huge,
        Self::Giant,
        Self::Colossal,
    ];
}

/// Age bucket classification.
///
/// Groups files by how recently they were modified/created/accessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum AgeBucket {
    /// Last 24 hours.
    Today = 0,
    /// 1 – 7 days.
    ThisWeek = 1,
    /// 7 – 30 days.
    ThisMonth = 2,
    /// 30 – 90 days.
    LastQuarter = 3,
    /// 90 – 365 days.
    ThisYear = 4,
    /// 1 – 3 years.
    Recent = 5,
    /// 3 – 10 years.
    Old = 6,
    /// > 10 years.
    Ancient = 7,
}

/// Number of age bucket variants.
pub(crate) const AGE_BUCKET_COUNT: usize = 8;

/// Age bucket boundary thresholds in microseconds (age from now).
const AGE_BOUNDARIES_US: [i64; 7] = [
    86_400_000_000,      // 1 day
    604_800_000_000,     // 7 days
    2_592_000_000_000,   // 30 days
    7_776_000_000_000,   // 90 days
    31_536_000_000_000,  // 365 days
    94_608_000_000_000,  // 3 years
    315_360_000_000_000, // 10 years
];

impl AgeBucket {
    /// Classify a timestamp into an age bucket.
    ///
    /// `age_us` is the difference `now_us - timestamp_us` (positive = past).
    #[inline]
    #[must_use]
    pub const fn classify(age_us: i64) -> Self {
        if age_us < AGE_BOUNDARIES_US[0] {
            Self::Today
        } else if age_us < AGE_BOUNDARIES_US[1] {
            Self::ThisWeek
        } else if age_us < AGE_BOUNDARIES_US[2] {
            Self::ThisMonth
        } else if age_us < AGE_BOUNDARIES_US[3] {
            Self::LastQuarter
        } else if age_us < AGE_BOUNDARIES_US[4] {
            Self::ThisYear
        } else if age_us < AGE_BOUNDARIES_US[5] {
            Self::Recent
        } else if age_us < AGE_BOUNDARIES_US[6] {
            Self::Old
        } else {
            Self::Ancient
        }
    }

    /// Human-readable label for this bucket.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::ThisWeek => "This week",
            Self::ThisMonth => "This month",
            Self::LastQuarter => "Last 90 days",
            Self::ThisYear => "This year",
            Self::Recent => "1 – 3 years",
            Self::Old => "3 – 10 years",
            Self::Ancient => "> 10 years",
        }
    }

    /// All bucket variants in order.
    pub const ALL: [Self; AGE_BUCKET_COUNT] = [
        Self::Today,
        Self::ThisWeek,
        Self::ThisMonth,
        Self::LastQuarter,
        Self::ThisYear,
        Self::Recent,
        Self::Old,
        Self::Ancient,
    ];
}

/// Path-risk bucket classification based on full path length.
///
/// Identifies files approaching or exceeding Windows `MAX_PATH` limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PathRiskBucket {
    /// 0 – 127 chars. Well within safe limits.
    Safe = 0,
    /// 128 – 199 chars. Getting long.
    Long = 1,
    /// 200 – 259 chars. Approaching `MAX_PATH`.
    Warning = 2,
    /// ≥ 260 chars. At or beyond `MAX_PATH`.
    Critical = 3,
}

/// Number of path-risk bucket variants.
pub const PATH_RISK_BUCKET_COUNT: usize = 4;

impl PathRiskBucket {
    /// Classify a path length (in chars) into a risk bucket.
    #[inline]
    #[must_use]
    pub const fn classify(path_len: u16) -> Self {
        if path_len < 128 {
            Self::Safe
        } else if path_len < 200 {
            Self::Long
        } else if path_len < 260 {
            Self::Warning
        } else {
            Self::Critical
        }
    }

    /// Human-readable label for this bucket.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Safe => "< 128 chars",
            Self::Long => "128 – 199 chars",
            Self::Warning => "200 – 259 chars",
            Self::Critical => "≥ 260 chars",
        }
    }

    /// All bucket variants in order.
    pub const ALL: [Self; PATH_RISK_BUCKET_COUNT] =
        [Self::Safe, Self::Long, Self::Warning, Self::Critical];
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests assert against fixtures with known shape; indexing panic = test failure"
)]
mod tests {
    use super::*;

    // ── Size bucket tests ──────────────────────────────────────

    #[test]
    fn size_bucket_boundaries() {
        assert_eq!(SizeBucket::classify(0), SizeBucket::Tiny);
        assert_eq!(SizeBucket::classify(1023), SizeBucket::Tiny);
        assert_eq!(SizeBucket::classify(1024), SizeBucket::Small);
        assert_eq!(SizeBucket::classify(102_399), SizeBucket::Small);
        assert_eq!(SizeBucket::classify(102_400), SizeBucket::Medium);
        assert_eq!(SizeBucket::classify(1_048_575), SizeBucket::Medium);
        assert_eq!(SizeBucket::classify(1_048_576), SizeBucket::Large);
        assert_eq!(SizeBucket::classify(104_857_599), SizeBucket::Large);
        assert_eq!(SizeBucket::classify(104_857_600), SizeBucket::Huge);
        assert_eq!(SizeBucket::classify(1_073_741_823), SizeBucket::Huge);
        assert_eq!(SizeBucket::classify(1_073_741_824), SizeBucket::Giant);
        assert_eq!(SizeBucket::classify(10_737_418_239), SizeBucket::Giant);
        assert_eq!(SizeBucket::classify(10_737_418_240), SizeBucket::Colossal);
        assert_eq!(SizeBucket::classify(u64::MAX), SizeBucket::Colossal);
    }

    #[test]
    fn size_bucket_labels() {
        assert_eq!(SizeBucket::Tiny.label(), "0 – 1 KB");
        assert_eq!(SizeBucket::Colossal.label(), "> 10 GB");
    }

    #[test]
    fn size_bucket_all_ordered() {
        for i in 1..SizeBucket::ALL.len() {
            assert!(SizeBucket::ALL[i - 1] < SizeBucket::ALL[i]);
        }
    }

    // ── Age bucket tests ───────────────────────────────────────

    #[test]
    fn age_bucket_boundaries() {
        let day = 86_400_000_000_i64;
        assert_eq!(AgeBucket::classify(0), AgeBucket::Today);
        assert_eq!(AgeBucket::classify(day - 1), AgeBucket::Today);
        assert_eq!(AgeBucket::classify(day), AgeBucket::ThisWeek);
        assert_eq!(AgeBucket::classify(7 * day - 1), AgeBucket::ThisWeek);
        assert_eq!(AgeBucket::classify(7 * day), AgeBucket::ThisMonth);
        assert_eq!(AgeBucket::classify(30 * day - 1), AgeBucket::ThisMonth);
        assert_eq!(AgeBucket::classify(30 * day), AgeBucket::LastQuarter);
        assert_eq!(AgeBucket::classify(90 * day - 1), AgeBucket::LastQuarter);
        assert_eq!(AgeBucket::classify(90 * day), AgeBucket::ThisYear);
        assert_eq!(AgeBucket::classify(365 * day - 1), AgeBucket::ThisYear);
        assert_eq!(AgeBucket::classify(365 * day), AgeBucket::Recent);
        assert_eq!(AgeBucket::classify(3 * 365 * day - 1), AgeBucket::Recent);
        assert_eq!(AgeBucket::classify(3 * 365 * day), AgeBucket::Old);
        assert_eq!(AgeBucket::classify(10 * 365 * day - 1), AgeBucket::Old);
        assert_eq!(AgeBucket::classify(10 * 365 * day), AgeBucket::Ancient);
    }

    #[test]
    fn age_bucket_labels() {
        assert_eq!(AgeBucket::Today.label(), "Today");
        assert_eq!(AgeBucket::Ancient.label(), "> 10 years");
    }

    // ── Path-risk bucket tests ─────────────────────────────────

    #[test]
    fn path_risk_bucket_boundaries() {
        assert_eq!(PathRiskBucket::classify(0), PathRiskBucket::Safe);
        assert_eq!(PathRiskBucket::classify(127), PathRiskBucket::Safe);
        assert_eq!(PathRiskBucket::classify(128), PathRiskBucket::Long);
        assert_eq!(PathRiskBucket::classify(199), PathRiskBucket::Long);
        assert_eq!(PathRiskBucket::classify(200), PathRiskBucket::Warning);
        assert_eq!(PathRiskBucket::classify(259), PathRiskBucket::Warning);
        assert_eq!(PathRiskBucket::classify(260), PathRiskBucket::Critical);
        assert_eq!(PathRiskBucket::classify(u16::MAX), PathRiskBucket::Critical);
    }

    #[test]
    fn path_risk_labels() {
        assert_eq!(PathRiskBucket::Safe.label(), "< 128 chars");
        assert_eq!(PathRiskBucket::Critical.label(), "≥ 260 chars");
    }
}
