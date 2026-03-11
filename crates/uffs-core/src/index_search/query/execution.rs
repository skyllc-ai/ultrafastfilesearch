//! Execution helpers and result collection for `IndexQuery`.

use rayon::prelude::*;

use super::IndexQuery;
use super::expansion::RecordExpander;
use super::filtering::RecordFilter;
use super::planning::CollectPlan;
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
        let plan = CollectPlan::build(index, self.pattern.as_ref(), include_system_metafiles);
        let expander = RecordExpander::new(
            index,
            &plan.path_cache,
            expand_names,
            expand_streams,
            resolve_paths,
        );

        plan.records_to_scan
            .par_iter()
            .filter(|record| plan.path_cache.is_valid(record.frs) && filters.matches(record))
            .take_any(limit.unwrap_or(usize::MAX))
            .flat_map_iter(|record| expander.collect_results(record))
            .collect()
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
