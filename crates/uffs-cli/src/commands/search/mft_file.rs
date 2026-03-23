//! Multi-file MFT streaming search execution.

use std::path::PathBuf;

use anyhow::Result;
use tracing::info;
use uffs_core::output::OutputConfig;

use super::super::output;
use super::streaming_io::write_with_closure;
use super::util::extract_trailing_extension;

/// Configuration for multi-file MFT streaming.
pub(super) struct MultiFileStreamConfig<'a> {
    /// MFT file paths to load.
    pub(super) mft_files: &'a [PathBuf],
    /// Drive letters for each file.
    pub(super) drive_letters: Vec<char>,
    /// Compiled pattern for filtering (None = full scan).
    pub(super) compiled_pattern: Option<uffs_core::index_search::IndexPattern>,
    /// Output format string.
    pub(super) format: &'a str,
    /// Output target path.
    pub(super) out: &'a str,
    /// Output configuration.
    pub(super) output_config: &'a OutputConfig,
    /// Output targets for footer.
    pub(super) output_targets: &'a [char],
    /// Pattern string for footer.
    pub(super) pattern: &'a str,
    /// Whether to use case-sensitive matching.
    pub(super) case_sensitive: bool,
    /// Whether pattern is path-aware.
    pub(super) is_path_pattern: bool,
    /// Record filter for attribute/date filtering.
    pub(super) rec_filter: output::StreamingRecordFilter,
    /// Debug tree flag.
    pub(super) debug_tree: bool,
    /// Chaos seed for testing.
    pub(super) chaos_seed: Option<u64>,
    /// Reserved allocation for size-aware queries.
    pub(super) reserved_allocated: Option<u64>,
}

/// Spawn parallel MFT file loaders.
///
/// Returns the loader thread handle and a receiver for loaded indexes.
fn spawn_parallel_loaders(
    file_drive_pairs: Vec<(PathBuf, char)>,
    debug_tree: bool,
    chaos_seed: Option<u64>,
    reserved_allocated: Option<u64>,
) -> (
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Receiver<(char, uffs_mft::MftIndex, u128)>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<(char, uffs_mft::MftIndex, u128)>();

    let handle = std::thread::spawn(move || {
        std::thread::scope(|scope| {
            for (path, drive) in &file_drive_pairs {
                let sender = tx.clone();
                scope.spawn(move || {
                    let result = super::super::raw_io::load_index_from_mft_file(
                        path,
                        Some(*drive),
                        debug_tree,
                        chaos_seed,
                        reserved_allocated,
                    );
                    match result {
                        Ok(loaded) => drop(sender.send((*drive, loaded.index, loaded.load_ms))),
                        Err(load_err) => tracing::warn!(
                            drive = %drive, path = %path.display(), error = %load_err,
                            "Failed to load MFT file"
                        ),
                    }
                });
            }
        });
    });

    (handle, rx)
}

/// Execute multi-file MFT streaming search.
///
/// Loads multiple MFT files in parallel and streams output as each completes.
pub(super) fn run_multi_file_streaming(config: &MultiFileStreamConfig<'_>) -> Result<usize> {
    let file_drive_pairs: Vec<_> = config
        .mft_files
        .iter()
        .zip(config.drive_letters.iter())
        .map(|(path, &drv)| (path.clone(), drv))
        .collect();

    let (loader_handle, rx) = spawn_parallel_loaders(
        file_drive_pairs,
        config.debug_tree,
        config.chaos_seed,
        config.reserved_allocated,
    );

    // Build C++ pattern for footer.
    let cpp_pattern = format!(
        ">{}",
        config
            .drive_letters
            .iter()
            .map(|drv| format!("{drv}:{}", config.pattern.replace('*', ".*")))
            .collect::<Vec<_>>()
            .join("|")
    );

    // Streaming closure for each drive.
    let rec_filter = &config.rec_filter;
    let compiled_pattern = &config.compiled_pattern;
    let output_config = config.output_config;

    let stream_drive =
        |mft_index: &uffs_mft::MftIndex, writer: &mut dyn std::io::Write| -> Result<usize> {
            let ext_indices: Option<Vec<u32>> = compiled_pattern.as_ref().and_then(|_pat| {
                let ext_index = mft_index.extension_index.as_ref()?;
                let ext = extract_trailing_extension(config.pattern)?;
                let ext_lower = ext.to_ascii_lowercase();
                let ext_id = mft_index.extensions.map.get(ext_lower.as_str())?;
                Some(ext_index.get_records(*ext_id).to_vec())
            });
            output::write_index_streaming_with_filter(
                mft_index,
                compiled_pattern.as_ref(),
                ext_indices.as_deref(),
                config.case_sensitive,
                config.is_path_pattern,
                rec_filter,
                writer,
                "",
                output_config,
                &output::CppFooterContext::empty(),
            )
        };

    // Stream results to output.
    let cols = output::selected_output_columns(output_config);
    let row_count = write_with_closure(config.out, |writer| {
        output::write_native_header_pub(writer, output_config, cols)?;
        let mut total = 0_usize;
        for (drive, received_index, load_ms) in rx {
            info!(drive = %drive, load_ms, records = received_index.len(), "📊 streaming drive");
            total += stream_drive(&received_index, writer)?;
        }
        if config.format == "custom" {
            let footer = output::CppFooterContext {
                output_targets: config.output_targets,
                pattern: &cpp_pattern,
                row_count: total,
            };
            output::write_cpp_footer_pub(writer, &footer)?;
        }
        writer.flush()?;
        Ok(total)
    })?;

    loader_handle
        .join()
        .map_err(|_panic| anyhow::anyhow!("Loader thread panicked"))?;

    Ok(row_count)
}
