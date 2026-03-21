//! Query routing helpers for hybrid search execution.

use super::pattern::IndexPattern;

/// Query execution mode for hybrid query engine.
///
/// Controls whether queries use the fast `MftIndex` path or the full-featured
/// `DataFrame` path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueryMode {
    /// Automatically choose the best path based on query complexity.
    ///
    /// Simple queries (glob, extension, size filters) use `IndexQuery`.
    /// Complex queries (SQL, aggregations, sorting) use `MftQuery`.
    #[default]
    Auto,

    /// Force use of `IndexQuery` (fast path).
    ///
    /// Best for simple searches where speed is critical.
    /// Some features may not be available (SQL, aggregations).
    ForceIndex,

    /// Force use of `MftQuery` (`DataFrame` path).
    ///
    /// Full feature set including SQL, aggregations, and sorting.
    /// Slower due to `DataFrame` conversion overhead.
    ForceDataFrame,
}

impl QueryMode {
    /// Parse from string (for CLI).
    #[must_use]
    pub fn from_str_opt(input: &str) -> Option<Self> {
        match input.to_ascii_lowercase().as_str() {
            "auto" | "hybrid" => Some(Self::Auto),
            "index" | "fast" => Some(Self::ForceIndex),
            "dataframe" | "df" | "polars" | "full" => Some(Self::ForceDataFrame),
            _ => None,
        }
    }
}

impl core::fmt::Display for QueryMode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Auto => write!(formatter, "auto"),
            Self::ForceIndex => write!(formatter, "index"),
            Self::ForceDataFrame => write!(formatter, "dataframe"),
        }
    }
}

/// Query complexity classification for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryComplexity {
    /// Simple query - can use `IndexQuery`.
    Simple,
    /// Complex query - requires `DataFrame`.
    Complex,
}

/// Analyze a pattern to determine query complexity.
#[must_use]
pub const fn analyze_pattern_complexity(pattern: &IndexPattern) -> QueryComplexity {
    match pattern {
        IndexPattern::Any
        | IndexPattern::Exact { .. }
        | IndexPattern::Prefix { .. }
        | IndexPattern::Suffix { .. }
        | IndexPattern::Contains { .. }
        | IndexPattern::PrefixSuffix { .. }
        | IndexPattern::ExactSet { .. }
        | IndexPattern::SuffixSet { .. }
        | IndexPattern::ContainsAny { .. }
        | IndexPattern::Regex { .. }
        | IndexPattern::Or { .. } => QueryComplexity::Simple,
    }
}

/// Features that require `DataFrame` path.
///
/// Uses bitflags pattern to avoid excessive bools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryFeatures(u8);

impl QueryFeatures {
    /// No special features.
    pub const NONE: Self = Self(0);
    /// SQL query requested.
    pub const SQL: Self = Self(1 << 0);
    /// Aggregation requested (count by extension, etc.).
    pub const AGGREGATION: Self = Self(1 << 1);
    /// Sorting requested (other than limit).
    pub const SORTING: Self = Self(1 << 2);
    /// Group by requested.
    pub const GROUP_BY: Self = Self(1 << 3);
    /// Join with another dataset.
    pub const JOIN: Self = Self(1 << 4);

    /// Create empty features.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Add a feature.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check if a feature is set.
    #[must_use]
    pub const fn has(self, feature: Self) -> bool {
        (self.0 & feature.0) != 0
    }

    /// Check if any feature requires `DataFrame`.
    #[must_use]
    pub const fn requires_dataframe(self) -> bool {
        self.0 != 0
    }
}
