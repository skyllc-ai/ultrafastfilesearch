//! Result expansion helpers for `IndexQuery` execution.

use uffs_mft::index::{FileRecord, MftIndex, PathCache};

use crate::index_search::SearchResult;

/// Expand a matching record into `(name × stream)` search results.
pub(super) struct RecordExpander<'a> {
    /// Index used for name, stream, and path resolution.
    index: &'a MftIndex,
    /// Shared path cache used when `resolve_paths` is enabled.
    path_cache: &'a PathCache<'a>,
    /// Whether hard-link names should be expanded.
    expand_names: bool,
    /// Whether Alternate Data Streams should be expanded.
    expand_streams: bool,
    /// Whether full paths should be materialized on results.
    resolve_paths: bool,
}

impl<'a> RecordExpander<'a> {
    /// Create a reusable record expander.
    #[must_use]
    #[expect(
        clippy::single_call_fn,
        reason = "keeps execution.rs focused on orchestration"
    )]
    pub(super) const fn new(
        index: &'a MftIndex,
        path_cache: &'a PathCache<'a>,
        expand_names: bool,
        expand_streams: bool,
        resolve_paths: bool,
    ) -> Self {
        Self {
            index,
            path_cache,
            expand_names,
            expand_streams,
            resolve_paths,
        }
    }

    /// Collect all output search results for the record.
    #[must_use]
    pub(super) fn collect_results(
        &self,
        record_idx: usize,
        record: &FileRecord,
    ) -> Vec<SearchResult> {
        let name_count = if self.expand_names {
            record.name_count.max(1)
        } else {
            1
        };
        let stream_count = if self.expand_streams {
            record.stream_count.max(1)
        } else {
            1
        };
        let path_index = self.path_cache.index();
        let path_resolver = self.path_cache.resolver();
        let dir_cache = self.path_cache.dir_cache();
        let cached_path = self.resolve_paths.then(|| {
            debug_assert!(
                path_resolver.is_valid_idx(record_idx),
                "collect_results only resolves paths for valid record indices"
            );
            path_resolver.materialize_path_cached(path_index, record_idx, dir_cache)
        });

        let mut results = Vec::with_capacity(usize::from(name_count) * usize::from(stream_count));
        for name_idx in 0..name_count {
            for stream_idx in 0..stream_count {
                let Some(stream_info) = self.index.get_stream_at(record, stream_idx) else {
                    continue;
                };
                if !stream_info.is_output_stream() {
                    continue;
                }

                let expanded_result =
                    SearchResult::from_expanded(record, self.index, name_idx, stream_idx);
                let final_result = if self.resolve_paths {
                    self.resolve_result_path(
                        expanded_result,
                        record,
                        record_idx,
                        name_idx,
                        stream_idx,
                        cached_path.clone(),
                    )
                } else {
                    expanded_result
                };
                results.push(final_result);
            }
        }

        results
    }

    /// Resolve the full path for a search result.
    ///
    /// Handles both primary names and hard links, and appends ADS names if
    /// needed.
    fn resolve_result_path(
        &self,
        result: SearchResult,
        record: &FileRecord,
        record_idx: usize,
        name_idx: u16,
        stream_idx: u16,
        cached_path: Option<String>,
    ) -> SearchResult {
        let path_index = self.path_cache.index();
        let path_resolver = self.path_cache.resolver();
        let stream = self
            .index
            .get_stream_at(record, stream_idx)
            .unwrap_or(&record.first_stream);

        let mut base_path = if name_idx == 0 {
            cached_path.unwrap_or_else(|| path_resolver.materialize_path(path_index, record_idx))
        } else {
            path_resolver.materialize_path_for_name(path_index, record_idx, name_idx)
        };

        let stream_name = self.index.stream_name(stream);
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
}
