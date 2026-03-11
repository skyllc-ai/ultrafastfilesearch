//! Execution-planning helpers for `IndexQuery` collection.

use uffs_mft::index::{FileRecord, MftIndex, PathCache};

use crate::index_search::IndexPattern;

/// Candidate-record scan plan for `IndexQuery::collect()`.
pub(super) enum ScanPlan<'a> {
    /// Scan the full record slice directly.
    Full(&'a [FileRecord]),
    /// Scan a narrowed set of record indices from the extension index.
    Filtered {
        /// Backing slice used for index lookup.
        records: &'a [FileRecord],
        /// Candidate indices from the extension index.
        indices: Vec<u32>,
    },
}

/// Precomputed inputs for `IndexQuery::collect()`.
pub(super) struct CollectPlan<'a> {
    /// Shared path cache used for path validity checks and materialization.
    pub(super) path_cache: PathCache<'a>,
    /// Candidate records to scan after extension-index planning.
    pub(super) scan_plan: ScanPlan<'a>,
}

impl<'a> CollectPlan<'a> {
    /// Build the collection execution plan.
    #[must_use]
    #[expect(
        clippy::single_call_fn,
        reason = "keeps execution.rs focused on orchestration"
    )]
    pub(super) fn build(
        index: &'a MftIndex,
        pattern: Option<&IndexPattern>,
        include_system_metafiles: bool,
    ) -> Self {
        let path_cache = PathCache::build(index, include_system_metafiles);
        let records = index.records();
        let scan_plan = Self::build_extension_filter_indices(pattern, index).map_or(
            ScanPlan::Full(records),
            |indices| ScanPlan::Filtered { records, indices },
        );

        Self {
            path_cache,
            scan_plan,
        }
    }

    /// Build extension filter indices for simple extension queries.
    ///
    /// Returns `Some(Vec<u32>)` if the pattern is a simple suffix (extension)
    /// pattern and the index has an extension index. Returns `None`
    /// otherwise.
    #[expect(
        clippy::single_call_fn,
        reason = "keeps collect-plan construction readable"
    )]
    fn build_extension_filter_indices(
        pattern: Option<&IndexPattern>,
        index: &MftIndex,
    ) -> Option<Vec<u32>> {
        let pat = pattern?;
        let ext_index = index.extension_index.as_ref()?;

        let IndexPattern::Suffix { suffix, .. } = pat else {
            return None;
        };

        let ext_str = suffix.strip_prefix('.')?;
        if ext_str.contains('.') {
            return None;
        }

        let ext_lower = ext_str.to_ascii_lowercase();
        let ext_id = index.extensions.map.get(ext_lower.as_str())?;
        let record_indices = ext_index.get_records(*ext_id);
        Some(record_indices.to_vec())
    }
}
