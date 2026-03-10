//! Cross-platform command handlers for `uffs_mft`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::display::{clean_path_for_display, format_bytes, format_duration, format_number_commas};

fn required_output_path(output_path: Option<&Path>) -> Result<&Path> {
    output_path.ok_or_else(|| {
        anyhow::anyhow!("internal error: --output should have been validated before export")
    })
}

/// Load MFT from a saved file and optionally export it.
///
/// Works on all platforms - parses NTFS structures from saved file.
/// Supports both UFFS-MFT format and raw NTFS format.
#[expect(
    clippy::too_many_lines,
    reason = "cli output function with complex display logic"
)]
#[expect(clippy::print_stdout, reason = "intentional user-facing cli output")]
#[expect(
    clippy::shadow_reuse,
    reason = "shadow reuse improves readability in sequential processing"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short identifiers used for concise loop variables"
)]
#[expect(
    clippy::single_call_fn,
    reason = "logical separation of load command implementation"
)]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "bool params map directly to cli flags"
)]
pub fn cmd_load(
    input: &Path,
    output_path: Option<&Path>,
    info_only: bool,
    build_index: bool,
    debug_tree: bool,
    drive_override: Option<char>,
    forensic: bool,
) -> Result<()> {
    use std::time::Instant;

    use uffs_mft::raw::LoadRawOptions;
    use uffs_mft::{MftReader, load_raw_mft};

    // Validate arguments upfront - don't print anything if we're going to fail
    if !info_only && !build_index && !debug_tree && output_path.is_none() {
        anyhow::bail!(
            "--output is required when not using --info-only, --build-index, or --debug-tree"
        );
    }

    let start_time = Instant::now();

    // Load header first (with volume letter override if provided)
    let load_options = LoadRawOptions {
        header_only: true,
        volume_letter: drive_override.map(|c| c.to_ascii_uppercase()),
        forensic,
    };
    let raw_data = load_raw_mft(input, &load_options)
        .with_context(|| format!("Failed to load raw MFT header from {}", input.display()))?;
    let header = raw_data.header;

    // Get absolute path and file size for display
    let abs_path = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let abs_path = clean_path_for_display(&abs_path);
    let file_size = std::fs::metadata(input).map_or(0, |meta| meta.len());

    // Determine format type for display
    let format_str = if header.version == 0 {
        "raw NTFS (compatible)"
    } else {
        "UFFS-MFT"
    };

    // Print formatted output
    println!("═══════════════════════════════════════════════════════════════");
    println!("                         MFT FILE INFO");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("📁 FILE DETAILS");
    println!("  Path:                 {}", abs_path.display());
    println!("  File size:           {}", format_bytes(file_size));
    if header.version == 0 {
        println!("  Format:               {format_str}");
    } else {
        println!("  Format:               {format_str} v{}", header.version);
    }
    println!("  Volume letter:        {}:", header.volume_letter);
    println!();
    println!("📊 MFT STRUCTURE");
    println!(
        "  Total records:        {}",
        format_number_commas(header.record_count)
    );
    println!(
        "  Bytes per record:     {}",
        format_number_commas(u64::from(header.record_size))
    );
    println!(
        "  Original MFT size:   {}",
        format_bytes(header.original_size)
    );
    println!();
    if header.version > 0 {
        println!("💾 COMPRESSION");
        if header.is_compressed() {
            println!(
                "  Compressed size:     {}",
                format_bytes(header.compressed_size)
            );
            #[expect(
                clippy::cast_precision_loss,
                reason = "precision loss acceptable for display percentages"
            )]
            #[expect(
                clippy::float_arithmetic,
                reason = "floating-point needed for compression ratio calculation"
            )]
            let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
            println!("  Compression ratio:    {ratio:.1}%");
            #[expect(
                clippy::float_arithmetic,
                reason = "floating-point needed for savings calculation"
            )]
            let savings = 100.0_f64 - ratio;
            println!("  Space saved:          {savings:.1}%");
        } else {
            println!("  Status:               uncompressed");
        }
    }

    // Create load options for data loading (not header-only)
    let data_load_options = LoadRawOptions {
        header_only: false,
        volume_letter: drive_override.map(|c| c.to_ascii_uppercase()),
        forensic,
    };

    // Print forensic mode warning if enabled
    if forensic {
        println!();
        println!("⚠️  FORENSIC MODE ENABLED");
        println!("  Including: deleted records, corrupt records, extension records");
        println!("  Output may contain 10-50% more rows than normal mode");
    }

    if info_only {
        // Parse the MFT to get detailed statistics
        println!();
        println!("📈 PARSING MFT FOR STATISTICS...");

        let df = MftReader::load_raw_to_dataframe_with_options(input, &data_load_options)
            .with_context(|| format!("Failed to parse raw MFT from {}", input.display()))?;

        let total_parsed = df.height();

        // Extract statistics from the DataFrame
        let dir_count = df
            .column("is_directory")
            .ok()
            .and_then(|col| col.bool().ok())
            .map_or(0, |bool_col| u64::from(bool_col.sum().unwrap_or(0)));
        let file_count = (total_parsed as u64).saturating_sub(dir_count);

        // Helper closure to count bool columns
        let count_bool = |name: &str| -> u64 {
            df.column(name)
                .ok()
                .and_then(|col| col.bool().ok())
                .map_or(0, |bool_col| u64::from(bool_col.sum().unwrap_or(0)))
        };

        let hidden_count = count_bool("is_hidden");
        let system_count = count_bool("is_system");
        let compressed_count = count_bool("is_compressed");
        let encrypted_count = count_bool("is_encrypted");
        let sparse_count = count_bool("is_sparse");

        // Total size calculation
        let total_size: u64 = df
            .column("size")
            .ok()
            .and_then(|col| col.u64().ok())
            .map_or(0, |size_col| size_col.iter().flatten().sum());

        println!();
        println!("📊 FILE STATISTICS");
        println!(
            "  Records parsed:       {}",
            format_number_commas(total_parsed as u64)
        );
        println!(
            "  Directories:          {}",
            format_number_commas(dir_count)
        );
        println!(
            "  Files:                {}",
            format_number_commas(file_count)
        );
        println!("  Total file size:     {}", format_bytes(total_size));
        println!();
        println!("🏷️  ATTRIBUTES");
        println!(
            "  Hidden:               {}",
            format_number_commas(hidden_count)
        );
        println!(
            "  System:               {}",
            format_number_commas(system_count)
        );
        println!(
            "  Compressed:           {}",
            format_number_commas(compressed_count)
        );
        println!(
            "  Encrypted:            {}",
            format_number_commas(encrypted_count)
        );
        println!(
            "  Sparse:               {}",
            format_number_commas(sparse_count)
        );

        println!();
        let elapsed = start_time.elapsed();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Build index and show tree metrics (for debugging)
    if build_index {
        println!();
        println!("🔨 BUILDING MFTINDEX...");

        let build_start = Instant::now();
        let index = MftReader::load_raw_to_index_with_options(input, &data_load_options)
            .with_context(|| format!("Failed to build index from {}", input.display()))?;
        let build_time = build_start.elapsed();

        println!();
        println!("✅ INDEX BUILT");
        println!(
            "  Records:              {}",
            format_number_commas(index.len() as u64)
        );
        println!("  Build time:          {}", format_duration(build_time));

        // Show sample tree metrics
        println!();
        println!("📊 TREE METRICS SAMPLE (first 10 directories):");
        println!();
        println!(
            "  {:<8} {:<12} {:<15} {:<15}",
            "FRS", "Descendants", "TreeSize", "TreeAllocated"
        );
        println!("  {}", "─".repeat(60));

        let mut shown = 0_i32;
        for record in &index.records {
            if record.is_directory() && shown < 10_i32 {
                println!(
                    "  {:<8} {:<12} {:<15} {:<15}",
                    record.frs,
                    record.descendants,
                    format_bytes(record.treesize),
                    format_bytes(record.tree_allocated)
                );
                shown += 1_i32;
            }
        }

        // Show root directory specifically
        if let Some(root) = index.records.iter().find(|r| r.frs == 5) {
            println!();
            println!("📁 ROOT DIRECTORY (FRS 5):");
            println!(
                "  Descendants:          {}",
                format_number_commas(root.descendants.into())
            );
            println!("  Tree size:           {}", format_bytes(root.treesize));
            println!(
                "  Tree allocated:      {}",
                format_bytes(root.tree_allocated)
            );
        }

        let elapsed = start_time.elapsed();
        println!();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Debug tree metrics computation (detailed hardlink handling)
    if debug_tree {
        use uffs_mft::MftIndex;
        use uffs_mft::parse::{
            ParseOptions, ParseResult, apply_fixup, parse_record, parse_record_forensic,
        };

        println!();
        println!("═══════════════════════════════════════════════════════════════");
        println!("                    DEBUG TREE METRICS");
        println!("═══════════════════════════════════════════════════════════════");
        println!();

        // Load raw MFT data
        let raw = load_raw_mft(input, &data_load_options)
            .with_context(|| format!("Failed to load raw MFT from {}", input.display()))?;
        println!("Raw MFT loaded: {} records", raw.header.record_count);

        // Parse all records
        let capacity = usize::try_from(raw.header.record_count).unwrap_or(0);
        let mut parsed_records = Vec::with_capacity(capacity);

        let parse_options = if forensic {
            ParseOptions::FORENSIC
        } else {
            ParseOptions::DEFAULT
        };

        let mut hardlink_count = 0_usize;
        let mut max_name_count = 0_u16;

        for (frs, record_data) in raw.iter_records() {
            let mut record_buf = record_data.to_vec();
            let fixup_ok = apply_fixup(&mut record_buf);

            if forensic {
                let result = parse_record_forensic(&record_buf, frs, &parse_options, !fixup_ok);
                if let ParseResult::Base(parsed) = result {
                    if parsed.names.len() > 1 {
                        hardlink_count += 1;
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "name count per mft record fits in u16"
                        )]
                        {
                            max_name_count = max_name_count.max(parsed.names.len() as u16);
                        }
                    }
                    parsed_records.push(parsed);
                }
            } else {
                if !fixup_ok {
                    continue;
                }
                if let Some(parsed) = parse_record(&record_buf, frs) {
                    if parsed.names.len() > 1 {
                        hardlink_count += 1;
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "name count per mft record fits in u16"
                        )]
                        {
                            max_name_count = max_name_count.max(parsed.names.len() as u16);
                        }
                    }
                    parsed_records.push(parsed);
                }
            }
        }

        println!("Parsed {} records", parsed_records.len());
        println!("Records with multiple names (hardlinks): {hardlink_count}");
        println!("Max name_count: {max_name_count}");

        // Show sample hardlinks
        println!();
        println!("=== SAMPLE HARDLINKS (first 10) ===");
        let mut shown = 0_u32;
        for parsed in &parsed_records {
            if parsed.names.len() > 1 && shown < 10_u32 {
                println!(
                    "  FRS {}: name_count={}, size={}",
                    parsed.frs,
                    parsed.names.len(),
                    parsed.size
                );
                for (idx, name) in parsed.names.iter().enumerate() {
                    println!(
                        "    [{idx}] parent_frs={}, name={}",
                        name.parent_frs, name.name
                    );
                }
                shown += 1_u32;
            }
        }

        // Build MftIndex (this computes tree metrics normally)
        println!();
        println!("Building MftIndex...");
        let mut index = MftIndex::from_parsed_records(header.volume_letter, parsed_records);

        println!(
            "Index built: {} records, {} children entries",
            index.len(),
            index.children_count()
        );

        // Now recompute tree metrics with debug output
        // (compute_tree_metrics_debug will recompute and print detailed info)
        println!();
        index.compute_tree_metrics_debug();

        let elapsed = start_time.elapsed();
        println!();
        println!("⏱️  Completed in {}", format_duration(elapsed));
        return Ok(());
    }

    // Parse and export (output is guaranteed to be Some by upfront validation)
    let output = required_output_path(output_path)?;

    // Determine output format from extension
    let ext = output
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("parquet");

    let format_name = if ext == "csv" { "CSV" } else { "Parquet" };

    println!();
    println!("📤 EXPORTING TO {format_name}...");
    println!("  Building MftIndex with tree metrics...");

    // Build MftIndex (includes tree metrics computation)
    let build_start = Instant::now();
    let index = MftReader::load_raw_to_index_with_options(input, &data_load_options)
        .with_context(|| format!("Failed to build index from {}", input.display()))?;
    let build_time = build_start.elapsed();

    println!(
        "  ✅ Index built in {} ({} records)",
        format_duration(build_time),
        format_number_commas(index.len() as u64)
    );

    // Convert MftIndex to DataFrame (includes tree metrics + path!)
    println!("  Converting to DataFrame with paths...");
    let df_start = Instant::now();
    let mut df = index
        .to_dataframe()
        .with_context(|| "Failed to convert index to DataFrame")?;
    let df_time = df_start.elapsed();

    println!(
        "  ✅ DataFrame created in {} ({} columns)",
        format_duration(df_time),
        df.width()
    );

    let parsed_count = df.height();

    // Export to file
    println!("  Writing {format_name} file...");
    let export_start = Instant::now();
    match ext {
        "csv" => {
            use std::fs::File;

            use uffs_polars::{CsvWriter, SerWriter};

            let file = File::create(output)?;
            CsvWriter::new(file).finish(&mut df)?;
        }
        _ => {
            MftReader::save_parquet(&mut df, output)?;
        }
    }
    let export_time = export_start.elapsed();

    println!("  ✅ Export completed in {}", format_duration(export_time));

    // Get absolute path and file size after creation
    let output_abs = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let output_abs = clean_path_for_display(&output_abs);
    let output_size = std::fs::metadata(output).map_or(0, |meta| meta.len());

    println!();
    println!("📁 OUTPUT FILE");
    println!("  Path:                 {}", output_abs.display());
    println!("  Format:               {format_name}");
    println!("  File size:           {}", format_bytes(output_size));
    println!(
        "  Records exported:     {}",
        format_number_commas(parsed_count as u64)
    );
    println!("  Columns:              {} columns including:", df.width());
    println!("                        - Core: frs, parent_frs, name, size, allocated_size");
    println!("                        - Timestamps: si_created, si_modified, fn_created, etc.");
    println!("                        - Flags: is_directory, is_readonly, is_hidden, etc.");
    if forensic {
        println!(
            "                        - Forensic: is_deleted, is_corrupt, is_extension, base_frs"
        );
    }
    println!("                        - Path: full resolved path (e.g., C:\\Users\\file.txt)");

    let elapsed = start_time.elapsed();
    println!();
    println!("⏱️  Completed in {}", format_duration(elapsed));

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::required_output_path;

    #[test]
    fn test_required_output_path_accepts_validated_path() {
        let path = Path::new("out.parquet");
        let resolved = required_output_path(Some(path)).expect("validated path should pass");
        assert_eq!(resolved, path);
    }

    #[test]
    fn test_required_output_path_rejects_missing_output() {
        let err = required_output_path(None).expect_err("missing output should error");
        assert!(
            err.to_string()
                .contains("internal error: --output should have been validated before export")
        );
    }
}
