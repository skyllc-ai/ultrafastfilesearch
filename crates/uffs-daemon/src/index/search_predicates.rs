// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Convert search-request filters into drill-down predicates.
//!
//! These predicates are prepended to each aggregation bucket's
//! drill-down list so a follow-up query reproduces the original
//! scope (pattern + filter mode + size range + drive selection)
//! plus the bucket key.
//!
//! Lifted out of `search.rs` to keep that file under the 800-line
//! policy ceiling.  Re-attached via
//! `#[path = "search_predicates.rs"] mod predicates;` in
//! `search.rs`, so `build_query_predicates` stays addressable as
//! `crate::index::search::build_query_predicates(...)` at the
//! single call site (the aggregation block in
//! [`crate::index::IndexManager::search`]).

use uffs_client::protocol::SearchParams;
use uffs_core::aggregate::finalize::{DrilldownPredicate, DrilldownValue};

/// Build the drill-down-predicate list that prefixes every
/// aggregation bucket's follow-up query.
///
/// Pure function — no `IndexManager` state, no I/O.  Each predicate
/// reproduces one filter from `params`:
///
/// * `pattern`: emitted as `name glob <pattern>` (skipped when the pattern is
///   empty or the trivial `*`).
/// * `filter` (`files` / `dirs`): emitted as `type eq <filter>` (skipped for
///   `all` / unset).
/// * `min_size` / `max_size`: emitted as `size gte <min>` and `size lte <max>`,
///   independently.
/// * `drives`: one `drive eq <letter>` per entry.
///
/// Returns an empty `Vec` when no filters are active — the
/// aggregation engine treats that as the "match everything in the
/// snapshot" baseline.
pub(super) fn build_query_predicates(params: &SearchParams) -> Vec<DrilldownPredicate> {
    let mut preds = Vec::new();

    // Pattern
    if !params.pattern.is_empty() && params.pattern != "*" {
        preds.push(DrilldownPredicate {
            field: "name".to_owned(),
            op: "glob".to_owned(),
            value: DrilldownValue::String(params.pattern.clone()),
        });
    }

    // Filter mode (files / dirs)
    if let Some(filter) = &params.filter
        && filter != "all"
    {
        preds.push(DrilldownPredicate {
            field: "type".to_owned(),
            op: "eq".to_owned(),
            value: DrilldownValue::String(filter.clone()),
        });
    }

    // Size range
    if let Some(min) = params.min_size {
        preds.push(DrilldownPredicate {
            field: "size".to_owned(),
            op: "gte".to_owned(),
            value: DrilldownValue::U64(min),
        });
    }
    if let Some(max) = params.max_size {
        preds.push(DrilldownPredicate {
            field: "size".to_owned(),
            op: "lte".to_owned(),
            value: DrilldownValue::U64(max),
        });
    }

    // Drives
    for &drive in &params.drives {
        preds.push(DrilldownPredicate {
            field: "drive".to_owned(),
            op: "eq".to_owned(),
            value: DrilldownValue::String(drive.to_string()),
        });
    }

    preds
}
