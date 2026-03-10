//! DataFrame construction helpers for parsed MFT output.

use uffs_polars::DataFrame;

use super::MftReader;
use crate::error::{MftError, Result};

impl MftReader {

    /// Helper to build DataFrame from parsed records (legacy AoS path).
    ///
    /// NOTE: This function is superseded by `build_dataframe_from_columns`
    /// which uses the SoA path and avoids the AoS→SoA transpose. Kept for
    /// reference and potential fallback use.
    #[cfg(windows)]
    #[expect(
        dead_code,
        reason = "kept as fallback for AoS path; superseded by build_dataframe_from_columns"
    )]
    fn build_dataframe_from_records(
        parsed_records: Vec<crate::io::ParsedRecord>,
    ) -> Result<DataFrame> {
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(name_count);
            stream_count_vec.push(stream_count);
            stream_name_vec.push(String::new()); // No expansion, use empty stream name
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
            is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
            is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
            is_pinned_vec.push(parsed.std_info.is_pinned);
            is_unpinned_vec.push(parsed.std_info.is_unpinned);
            is_virtual_vec.push(parsed.std_info.is_virtual);
            flags_vec.push(parsed.std_info.to_raw_flags());
        }

        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            stream_name_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
        )
    }
    /// Builds a `DataFrame` from the collected vectors (legacy 8-column
    /// schema).
    #[cfg(windows)]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per dataframe column in legacy 8-column schema"
    )]
    #[expect(dead_code, reason = "kept as fallback for legacy 8-column schema")]
    fn build_dataframe(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        flags_vec: Vec<u16>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let columns = vec![
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            Series::new("size".into(), size_vec).into_column(),
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` with the full baseline-compatible schema (23
    /// columns).
    #[cfg(windows)]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per dataframe column in full 23-column schema"
    )]
    pub(super) fn build_dataframe_full(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        allocated_size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        mft_changed_vec: Vec<i64>,
        is_directory_vec: Vec<bool>,
        name_count_vec: Vec<u16>,
        stream_count_vec: Vec<u16>,
        stream_name_vec: Vec<String>,
        is_readonly_vec: Vec<bool>,
        is_hidden_vec: Vec<bool>,
        is_system_vec: Vec<bool>,
        is_archive_vec: Vec<bool>,
        is_compressed_vec: Vec<bool>,
        is_encrypted_vec: Vec<bool>,
        is_sparse_vec: Vec<bool>,
        is_reparse_vec: Vec<bool>,
        is_offline_vec: Vec<bool>,
        is_not_indexed_vec: Vec<bool>,
        is_temporary_vec: Vec<bool>,
        is_integrity_stream_vec: Vec<bool>,
        is_no_scrub_data_vec: Vec<bool>,
        is_pinned_vec: Vec<bool>,
        is_unpinned_vec: Vec<bool>,
        is_virtual_vec: Vec<bool>,
        flags_vec: Vec<u32>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let columns = vec![
            // Core identifiers
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            // Size information
            Series::new("size".into(), size_vec).into_column(),
            Series::new("allocated_size".into(), allocated_size_vec).into_column(),
            // Timestamps (4 total, matching C++)
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), mft_changed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            // Type and counts
            Series::new("is_directory".into(), is_directory_vec).into_column(),
            Series::new("name_count".into(), name_count_vec).into_column(),
            Series::new("stream_count".into(), stream_count_vec).into_column(),
            Series::new("stream_name".into(), stream_name_vec).into_column(),
            // Extended attribute flags (matching C++ StandardInfo)
            Series::new("is_readonly".into(), is_readonly_vec).into_column(),
            Series::new("is_hidden".into(), is_hidden_vec).into_column(),
            Series::new("is_system".into(), is_system_vec).into_column(),
            Series::new("is_archive".into(), is_archive_vec).into_column(),
            Series::new("is_compressed".into(), is_compressed_vec).into_column(),
            Series::new("is_encrypted".into(), is_encrypted_vec).into_column(),
            Series::new("is_sparse".into(), is_sparse_vec).into_column(),
            Series::new("is_reparse".into(), is_reparse_vec).into_column(),
            Series::new("is_offline".into(), is_offline_vec).into_column(),
            Series::new("is_not_indexed".into(), is_not_indexed_vec).into_column(),
            Series::new("is_temporary".into(), is_temporary_vec).into_column(),
            // Additional flags for baseline-compatible output
            Series::new("is_integrity_stream".into(), is_integrity_stream_vec).into_column(),
            Series::new("is_no_scrub_data".into(), is_no_scrub_data_vec).into_column(),
            Series::new("is_pinned".into(), is_pinned_vec).into_column(),
            Series::new("is_unpinned".into(), is_unpinned_vec).into_column(),
            Series::new("is_virtual".into(), is_virtual_vec).into_column(),
            // Raw attribute flags (combined value for baseline-compatible output)
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` directly from `ParsedColumns` (`SoA` layout).
    ///
    /// This is the optimized path that avoids the AoS→SoA transpose.
    /// The columns are already in the correct format, so we just wrap them
    /// in Polars Series.
    ///
    /// # Platform
    ///
    /// Cross-platform - works on all platforms.
    #[expect(clippy::single_call_fn, reason = "extracted for clarity")]
    pub(super) fn build_dataframe_from_columns(
        columns: crate::parse::ParsedColumns,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let polars_columns = vec![
            // Core identifiers
            Series::new("frs".into(), columns.frs).into_column(),
            Series::new("parent_frs".into(), columns.parent_frs).into_column(),
            Series::new("name".into(), columns.name).into_column(),
            // Size information
            Series::new("size".into(), columns.size).into_column(),
            Series::new("allocated_size".into(), columns.allocated_size).into_column(),
            // Timestamps (4 total, matching C++)
            Series::new("created".into(), columns.created)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), columns.modified)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), columns.accessed)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), columns.mft_changed)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            // Type and counts
            Series::new("is_directory".into(), columns.is_directory).into_column(),
            Series::new("name_count".into(), columns.name_count).into_column(),
            Series::new("stream_count".into(), columns.stream_count).into_column(),
            Series::new("stream_name".into(), columns.stream_name).into_column(),
            // Extended attribute flags (matching C++ StandardInfo)
            Series::new("is_readonly".into(), columns.is_readonly).into_column(),
            Series::new("is_hidden".into(), columns.is_hidden).into_column(),
            Series::new("is_system".into(), columns.is_system).into_column(),
            Series::new("is_archive".into(), columns.is_archive).into_column(),
            Series::new("is_compressed".into(), columns.is_compressed).into_column(),
            Series::new("is_encrypted".into(), columns.is_encrypted).into_column(),
            Series::new("is_sparse".into(), columns.is_sparse).into_column(),
            Series::new("is_reparse".into(), columns.is_reparse).into_column(),
            Series::new("is_offline".into(), columns.is_offline).into_column(),
            Series::new("is_not_indexed".into(), columns.is_not_indexed).into_column(),
            Series::new("is_temporary".into(), columns.is_temporary).into_column(),
            Series::new("is_integrity_stream".into(), columns.is_integrity_stream).into_column(),
            Series::new("is_no_scrub_data".into(), columns.is_no_scrub_data).into_column(),
            Series::new("is_pinned".into(), columns.is_pinned).into_column(),
            Series::new("is_unpinned".into(), columns.is_unpinned).into_column(),
            Series::new("is_virtual".into(), columns.is_virtual).into_column(),
            // Raw attribute flags (combined value for baseline-compatible output)
            Series::new("flags".into(), columns.flags).into_column(),
        ];

        DataFrame::new_infer_height(polars_columns).map_err(MftError::from)
    }

    /// Create an empty `DataFrame` with the MFT schema.
    #[expect(dead_code, reason = "utility for tests and potential future use")]
    fn create_empty_dataframe() -> Result<DataFrame> {
        use uffs_polars::{Column, DataType, TimeUnit};

        let schema_columns = vec![
            Column::new_empty("frs".into(), &DataType::UInt64),
            Column::new_empty("parent_frs".into(), &DataType::UInt64),
            Column::new_empty("name".into(), &DataType::String),
            Column::new_empty("size".into(), &DataType::UInt64),
            Column::new_empty(
                "created".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "modified".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "accessed".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty("flags".into(), &DataType::UInt16),
        ];

        // Use new_infer_height to infer height from columns (Polars 0.52+ API)
        DataFrame::new_infer_height(schema_columns).map_err(MftError::from)
    }

    /// Convert parsed records to DataFrame (legacy AoS path).
    ///
    /// NOTE: This function is superseded by `build_dataframe_from_columns`
    /// which uses the SoA path and avoids the AoS→SoA transpose. Kept for
    /// reference.
    #[cfg(windows)]
    #[expect(
        dead_code,
        reason = "kept as reference for legacy AoS path; superseded by build_dataframe_from_columns"
    )]
    fn parsed_records_to_dataframe(
        parsed_records: Vec<crate::io::ParsedRecord>,
    ) -> Result<DataFrame> {
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            // Compute counts before moving any fields
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(name_count);
            stream_count_vec.push(stream_count);
            stream_name_vec.push(String::new()); // No expansion, use empty stream name
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
            is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
            is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
            is_pinned_vec.push(parsed.std_info.is_pinned);
            is_unpinned_vec.push(parsed.std_info.is_unpinned);
            is_virtual_vec.push(parsed.std_info.is_virtual);
            flags_vec.push(parsed.std_info.to_raw_flags());
        }

        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            stream_name_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
        )
    }
}

