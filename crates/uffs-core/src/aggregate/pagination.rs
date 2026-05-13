// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Cursor-based pagination for aggregate results.
//!
//! Allows clients to page through large result sets (e.g. terms with
//! thousands of extensions) without loading all data at once.

use super::finalize::{AggregateResult, AggregateResultData, BucketRow};

/// A pagination cursor — opaque token encoding position in a result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateCursor {
    /// Which aggregate result index this cursor belongs to.
    pub result_index: usize,
    /// Offset (number of items already returned).
    pub offset: usize,
    /// Page size (items per page).
    pub page_size: usize,
}

impl AggregateCursor {
    /// Create a new cursor starting at the beginning.
    #[must_use]
    pub const fn new(result_index: usize, page_size: usize) -> Self {
        Self {
            result_index,
            offset: 0,
            page_size,
        }
    }

    /// Advance the cursor by one page. Returns `None` if at end.
    #[must_use]
    pub const fn next(&self) -> Self {
        Self {
            result_index: self.result_index,
            offset: self.offset + self.page_size,
            page_size: self.page_size,
        }
    }

    /// Encode cursor as an opaque string token.
    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}:{}:{}", self.result_index, self.offset, self.page_size)
    }

    /// Decode a cursor from an opaque string token.
    #[must_use]
    pub fn decode(token: &str) -> Option<Self> {
        let mut parts = token.split(':');
        let result_index = parts.next()?.parse().ok()?;
        let offset = parts.next()?.parse().ok()?;
        let page_size = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            result_index,
            offset,
            page_size,
        })
    }
}

/// A paginated view of bucket rows.
#[derive(Debug, Clone)]
pub struct PaginatedBuckets {
    /// The current page of rows.
    pub rows: Vec<BucketRow>,
    /// Total number of rows available.
    pub total: usize,
    /// Current offset.
    pub offset: usize,
    /// Cursor token for the next page, if any.
    pub next_cursor: Option<String>,
    /// Whether there are more pages.
    pub has_more: bool,
}

/// Apply pagination to an aggregate result.
///
/// Returns a `PaginatedBuckets` view for bucket-type results,
/// or `None` if the result is not a bucket type.
#[must_use]
pub fn paginate_result(
    result: &AggregateResult,
    cursor: &AggregateCursor,
) -> Option<PaginatedBuckets> {
    let (AggregateResultData::Buckets { rows, .. } | AggregateResultData::Rollup { rows, .. }) =
        &result.data
    else {
        return None;
    };

    let total = rows.len();
    let start = cursor.offset.min(total);
    let end = start.saturating_add(cursor.page_size).min(total);
    let page_rows = rows
        .get(start..end)
        .map(<[BucketRow]>::to_vec)
        .unwrap_or_default();
    let has_more = end < total;

    let next_cursor = has_more.then(|| cursor.next().encode());

    Some(PaginatedBuckets {
        rows: page_rows,
        total,
        offset: start,
        next_cursor,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let cursor = AggregateCursor::new(2, 25);
        let encoded = cursor.encode();
        let decoded = AggregateCursor::decode(&encoded).unwrap();
        assert_eq!(cursor, decoded);
    }

    #[test]
    fn cursor_advance() {
        let cursor = AggregateCursor::new(0, 20);
        let next = cursor.next();
        assert_eq!(next.offset, 20);
        assert_eq!(next.page_size, 20);
    }

    #[test]
    fn decode_invalid_cursor() {
        assert!(AggregateCursor::decode("invalid").is_none());
        assert!(AggregateCursor::decode("1:2").is_none());
        assert!(AggregateCursor::decode("a:b:c").is_none());
    }
}
