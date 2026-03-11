//! Execution helpers and result collection for `IndexQuery`.

use rayon::prelude::*;
use uffs_mft::index::{FileRecord, MftIndex, PathCache};

use super::IndexQuery;
use super::filtering::RecordFilter;
use crate::index_search::{IndexPattern, SearchResult};

impl IndexQuery<'_> {
    /// Build extension filter indices for simple extension queries.
    ///
    /// Returns `Some(Vec<u32>)` if the pattern is a simple suffix (extension)
    /// pattern and the index has an extension index. Returns `None`
    /// otherwise.
    ///
    /// Extracted to reduce line count of `collect` method.
    #[expect(
        clippy::single_call_fn,
        reason = "extracted from collect() to satisfy too_many_lines"
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

    /// Resolve the full path for a search result.
    ///
    /// Handles both primary names and hard links, and appends ADS names if
    /// needed.
    #[expect(
        clippy::single_call_fn,
        reason = "extracted from collect() to satisfy too_many_lines"
    )]
    fn resolve_result_path(
        result: SearchResult,
        record: &FileRecord,
        index: &MftIndex,
        name_idx: u16,
        stream_idx: u16,
        cached_path: Option<String>,
    ) -> SearchResult {
        let stream = index
            .get_stream_at(record, stream_idx)
            .unwrap_or(&record.first_stream);

        let mut base_path = if name_idx == 0 {
            cached_path.unwrap_or_else(|| index.build_path(record.frs))
        } else {
            index.build_path_for_name(record, name_idx)
        };

        let stream_name = index.stream_name(stream);
        let path = if stream_name.is_empty() {
            if record.is_directory() && !base_path.ends_with('\\') {
                base_path.push('\\');
            }
            base_path
        } else {
            format!("{base_path}:{stream_name}")
        };

        result.with_path(path)
    }

    /// Execute the query and collect results.
    ///
    /// Uses Rayon for parallel execution across all records.
    /// Filters are applied in optimal order: type → size → pattern.
    /// When expansion is enabled, each (name × stream) combination produces a
    /// result.
    #[must_use]
    pub fn collect(self) -> Vec<SearchResult> {
        let records = self.index.records();
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

        let path_cache = PathCache::build(index, include_system_metafiles);
        let extension_filter_indices =
            Self::build_extension_filter_indices(self.pattern.as_ref(), index);

        let records_to_scan: Vec<&FileRecord> = extension_filter_indices.as_ref().map_or_else(
            || records.iter().collect(),
            |indices| {
                indices
                    .iter()
                    .filter_map(|&idx| records.get(idx as usize))
                    .collect()
            },
        );

        records_to_scan
            .par_iter()
            .filter(|record| path_cache.is_valid(record.frs) && filters.matches(record))
            .take_any(limit.unwrap_or(usize::MAX))
            .flat_map_iter(|record| {
                let name_count = if expand_names {
                    record.name_count.max(1)
                } else {
                    1
                };
                let stream_count = if expand_streams {
                    record.stream_count.max(1)
                } else {
                    1
                };

                let outer_cached_path = if resolve_paths {
                    path_cache.get(record.frs)
                } else {
                    None
                };

                (0..name_count).flat_map(move |name_idx| {
                    let inner_cached_path = outer_cached_path.clone();
                    (0..stream_count).filter_map(move |stream_idx| {
                        let stream_info = index.get_stream_at(record, stream_idx)?;
                        if !stream_info.is_output_stream() {
                            return None;
                        }

                        let result =
                            SearchResult::from_expanded(record, index, name_idx, stream_idx);
                        if resolve_paths {
                            Some(Self::resolve_result_path(
                                result,
                                record,
                                index,
                                name_idx,
                                stream_idx,
                                inner_cached_path.clone(),
                            ))
                        } else {
                            Some(result)
                        }
                    })
                })
            })
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
