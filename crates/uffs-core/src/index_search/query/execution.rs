//! Execution helpers and result collection for `IndexQuery`.

use rayon::prelude::*;

use super::IndexQuery;
use super::expansion::RecordExpander;
use super::filtering::RecordFilter;
use super::planning::{CollectPlan, ScanPlan};
use crate::index_search::SearchResult;

impl IndexQuery<'_> {
    /// Execute the query and collect results.
    ///
    /// Uses Rayon for parallel execution across all records.
    /// Filters are applied in optimal order: type → size → pattern.
    /// When expansion is enabled, each (name × stream) combination produces a
    /// result.
    #[must_use]
    pub fn collect(self) -> Vec<SearchResult> {
        let resolve_paths = self.options.resolve_paths;
        let expand_names = self.options.expand_names;
        let expand_streams = self.options.expand_streams;
        let include_system_metafiles = self.options.include_system_metafiles;
        let limit = self.limit;
        let index = self.index;
        let filters = RecordFilter::new(
            index,
            self.pattern.as_ref(),
            self.options.case_sensitive,
            self.options.type_filter,
            self.min_size,
            self.max_size,
        );
        let CollectPlan {
            path_cache,
            scan_plan,
        } = CollectPlan::build(index, self.pattern.as_ref(), include_system_metafiles);
        let path_resolver = path_cache.resolver();
        let expander = RecordExpander::new(
            index,
            &path_cache,
            expand_names,
            expand_streams,
            resolve_paths,
        );
        let scan_limit = limit.unwrap_or(usize::MAX);

        match scan_plan {
            ScanPlan::Full(records) => records
                .par_iter()
                .enumerate()
                .filter(|(record_idx, record)| {
                    path_resolver.is_valid_idx(*record_idx) && filters.matches(record)
                })
                .take_any(scan_limit)
                .flat_map_iter(|(record_idx, record)| expander.collect_results(record_idx, record))
                .collect(),
            ScanPlan::Filtered { records, indices } => indices
                .par_iter()
                .filter_map(|&record_idx_u32| {
                    let record_idx = usize::try_from(record_idx_u32).ok()?;
                    let record = records.get(record_idx)?;
                    (path_resolver.is_valid_idx(record_idx) && filters.matches(record))
                        .then_some((record_idx, record))
                })
                .take_any(scan_limit)
                .flat_map_iter(|(record_idx, record)| expander.collect_results(record_idx, record))
                .collect(),
        }
    }

    /// Count matching records without collecting results.
    ///
    /// More efficient than `collect().len()` when you only need the count.
    #[must_use]
    pub fn count(self) -> usize {
        let records = self.index.records();
        let filters = RecordFilter::new(
            self.index,
            self.pattern.as_ref(),
            self.options.case_sensitive,
            self.options.type_filter,
            self.min_size,
            self.max_size,
        );

        records
            .par_iter()
            .filter(|record| filters.matches(record))
            .count()
    }
}
