//! Streaming I/O helpers for writing search results to console or file.

use anyhow::{Context, Result};
use tracing::info;
use uffs_core::output::OutputConfig;

use super::super::output;
use super::super::raw_io::QueryFilters;
use super::util::extract_trailing_extension;

/// Shared helper: write streaming output from an `MftIndex` to file or console.
///
/// Used by both `--mft-file` and Windows LIVE full-scan paths (DRY).
pub(super) fn write_streaming_output(
    index: &uffs_mft::MftIndex,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    cpp_pattern: &str,
) -> Result<usize> {
    use std::io::Write as _;

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        // Wrap stdout in BufWriter to avoid per-line WriteFile syscalls.
        // Without this, 2.2M write_all calls each trigger a syscall
        // through the OS pipe, making redirected stdout 15× slower than
        // C's printf (which buffers in the C runtime).
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        let footer_ctx = output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let result =
            output::write_index_streaming(index, &mut writer, format, output_config, &footer_ctx);
        writer.flush()?;
        result
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let footer_ctx = output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let count =
            output::write_index_streaming(index, &mut writer, format, output_config, &footer_ctx)?;
        writer.flush()?;
        info!(file = out, "Results written to file");
        Ok(count)
    }
}

/// Shared helper: write streaming output with pattern filter to file or
/// console.
///
/// Unified path for ALL filtered patterns — compiles the pattern and
/// optionally uses the extension index for O(matches) scan.
#[expect(
    clippy::too_many_arguments,
    reason = "unified streaming writer needs pattern, indices, case, path-mode, filter, format, output config, targets, footer pattern"
)]
pub(super) fn write_streaming_output_with_filter(
    index: &uffs_mft::MftIndex,
    pattern: &uffs_core::IndexPattern,
    record_indices: Option<&[u32]>,
    case_sensitive: bool,
    is_path_pattern: bool,
    record_filter: &output::StreamingRecordFilter,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    cpp_pattern: &str,
) -> Result<usize> {
    use std::io::Write as _;

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        let footer_ctx = output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let result = output::write_index_streaming_with_filter(
            index,
            Some(pattern),
            record_indices,
            case_sensitive,
            is_path_pattern,
            record_filter,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        );
        writer.flush()?;
        result
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let footer_ctx = output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let count = output::write_index_streaming_with_filter(
            index,
            Some(pattern),
            record_indices,
            case_sensitive,
            is_path_pattern,
            record_filter,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        )?;
        writer.flush()?;
        info!(file = out, "Results written to file");
        Ok(count)
    }
}

/// Write streaming output to console or file using a closure.
pub(super) fn write_with_closure<F>(out: &str, write_fn: F) -> Result<usize>
where
    F: FnOnce(&mut dyn std::io::Write) -> Result<usize>,
{
    let is_console = out.is_empty() || out == "-";
    if is_console {
        let stdout = std::io::stdout();
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout.lock());
        write_fn(&mut buf_writer)
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let rows = write_fn(&mut buf_writer)?;
        info!(file = out, "Results written to file");
        Ok(rows)
    }
}

/// Try to get record indices from the extension index for simple suffix
/// patterns.
///
/// Returns `Some(Vec<u32>)` for patterns like `*.rs`, `*.txt` where the
/// extension index provides O(matches) lookup.  Returns `None` for complex
/// patterns that need full-scan matching.
pub(super) fn try_get_extension_indices(
    index: &uffs_mft::MftIndex,
    filters: &QueryFilters<'_>,
) -> Option<Vec<u32>> {
    // Only works for simple glob suffix patterns with no other filters.
    if filters.files_only
        || filters.dirs_only
        || filters.hide_system
        || filters.ext_filter.is_some()
        || filters.min_size.is_some()
        || filters.max_size.is_some()
        || filters.limit > 0
    {
        return None;
    }

    let pattern = filters.parsed.pattern();

    // Extract a trailing literal extension from ANY pattern.
    // Examples:
    //   "*.txt"         → ext = "txt"
    //   "*hallo*.txt"   → ext = "txt"
    //   "foo*.rs"       → ext = "rs"
    //   "*.tar.gz"      → None (multi-dot)
    //   "*hallo*"       → None (no extension)
    //   "nice"          → None (no dot)
    let ext = extract_trailing_extension(pattern)?;

    let ext_index = index.extension_index.as_ref()?;
    let ext_lower = ext.to_ascii_lowercase();
    let ext_id = index.extensions.map.get(ext_lower.as_str())?;
    Some(ext_index.get_records(*ext_id).to_vec())
}

/// Build a `StreamingRecordFilter` from `QueryFilters` + extra CLI params.
#[expect(clippy::too_many_arguments, reason = "collects all filter CLI params")]
pub(super) fn build_record_filter(
    filters: &QueryFilters<'_>,
    attr_filter: Option<&str>,
    newer: Option<&str>,
    older: Option<&str>,
    newer_created: Option<&str>,
    older_created: Option<&str>,
    newer_accessed: Option<&str>,
    older_accessed: Option<&str>,
    exclude: Option<&str>,
    sort: Option<&str>,
    sort_desc: bool,
) -> output::StreamingRecordFilter {
    let exclude_pattern = exclude.and_then(|excl| uffs_core::compile_index_pattern(excl).ok());

    output::StreamingRecordFilter {
        files_only: filters.files_only,
        dirs_only: filters.dirs_only,
        hide_system: filters.hide_system,
        min_size: filters.min_size,
        max_size: filters.max_size,
        attr_filters: attr_filter
            .map(output::parse_attr_filter)
            .unwrap_or_default(),
        newer_modified: newer.and_then(output::parse_age_filter),
        older_modified: older.and_then(output::parse_age_filter),
        newer_created: newer_created.and_then(output::parse_age_filter),
        older_created: older_created.and_then(output::parse_age_filter),
        newer_accessed: newer_accessed.and_then(output::parse_age_filter),
        older_accessed: older_accessed.and_then(output::parse_age_filter),
        exclude_pattern,
        limit: filters.limit as usize,
        sort_spec: sort.map(output::parse_sort_spec).unwrap_or_default(),
        sort_desc,
    }
}
