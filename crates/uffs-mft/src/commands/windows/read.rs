//! Live MFT read/export command handlers.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{info, warn};
use uffs_mft::MftReader;

use crate::display::{format_bytes, format_duration, format_number_commas};
use crate::progress::spinner;

pub async fn cmd_read(
    drive: char,
    output: PathBuf,
    mode_str: &str,
    full: bool,
    unique: bool,
    forensic: bool,
) -> Result<()> {
    use std::time::Instant;

    use tracing::debug;
    use uffs_mft::MftReadMode;

    let start_time = Instant::now();
    let drive_upper = drive.to_ascii_uppercase();

    // Forensic mode is not yet supported for live reads
    // (requires significant I/O layer refactoring)
    if forensic {
        warn!("⚠️ Forensic mode (--forensic) is not yet supported for live reads.");
        warn!(
            "   Use 'uffs_mft save' to save the MFT, then 'uffs_mft load --forensic' to analyze."
        );
        warn!("   Proceeding with normal mode...");
    }

    // Parse read mode
    let mode: MftReadMode = mode_str.parse().map_err(|e: String| anyhow::anyhow!(e))?;

    info!(
        drive = %drive_upper,
        output = %output.display(),
        mode = %mode,
        full,
        unique,
        "📂 Starting MFT read operation{}",
        if unique { " (unique FRS mode)" } else { " (expanding hard links)" }
    );

    let pb = spinner("Opening volume...");

    debug!(drive = %drive_upper, "🔓 Opening volume handle");
    let open_start = Instant::now();

    let reader = MftReader::open(drive)
        .with_context(|| format!("Failed to open drive {}:", drive))?
        .with_mode(mode)
        .with_merge_extensions(full)
        .with_expand_links(!unique); // unique=true means don't expand
    // Note: forensic mode is not yet supported for live reads (see warning above)

    info!(
        drive = %drive_upper,
        elapsed_ms = open_start.elapsed().as_millis(),
        "✅ Volume opened successfully"
    );

    pb.set_message("Reading MFT records...");
    debug!("📖 Starting MFT record enumeration");
    let read_start = Instant::now();

    let mut df = reader.read_all().with_context(|| "Failed to read MFT")?;

    let record_count = df.height();
    let read_elapsed = read_start.elapsed();
    let records_per_sec = if read_elapsed.as_secs_f64() > 0.0 {
        record_count as f64 / read_elapsed.as_secs_f64()
    } else {
        0.0
    };

    info!(
        records = record_count,
        elapsed_ms = read_elapsed.as_millis(),
        records_per_sec = format!("{:.0}", records_per_sec),
        "✅ MFT read complete"
    );

    pb.set_message("Saving to Parquet...");
    debug!(output = %output.display(), "💾 Writing Parquet file");
    let save_start = Instant::now();

    MftReader::save_parquet(&mut df, &output).with_context(|| "Failed to save Parquet")?;

    // Get file size for logging
    let file_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

    info!(
        output = %output.display(),
        file_size_mb = format!("{:.2}", file_size_mb),
        elapsed_ms = save_start.elapsed().as_millis(),
        "✅ Parquet file saved"
    );

    let total_elapsed = start_time.elapsed();
    info!(
        drive = %drive_upper,
        records = record_count,
        total_elapsed_ms = total_elapsed.as_millis(),
        output_size_mb = format!("{:.2}", file_size_mb),
        "🎉 MFT export complete"
    );

    pb.finish_with_message(format!(
        "✅ Exported {} records to {} ({}) in {}",
        format_number_commas(record_count as u64),
        output.display(),
        format_bytes(file_size),
        format_duration(total_elapsed)
    ));

    Ok(())
}
