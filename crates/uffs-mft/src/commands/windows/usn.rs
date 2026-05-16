// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! USN Journal command handlers.
//!
//! Extracted out of `incremental.rs` (2026-04-21) so both files stay
//! under the 800-LOC file-size policy.  These handlers only touch
//! `uffs_mft::usn::*` (journal queries + record decode) and share no
//! state with the index-save / cache-management verbs that live in
//! their sibling modules.
//!
//! These commands print human-readable journal records to stdout and use
//! `Debug` formatting for diagnostic enums; the lint exemptions below capture
//! those CLI-specific patterns.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "intentional user-facing CLI USN journal output: stdout for primary output, stderr for journal-unavailable diagnostics"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short identifiers used for printf-style indices in CLI output"
)]

use anyhow::Result;
use uffs_mft::bytes_to_mb_f64;

use crate::display::format_usn_reason;

/// Query USN Journal information for a drive.
#[cfg(windows)]
pub(crate) async fn cmd_usn_info(drive: uffs_mft::platform::DriveLetter) -> Result<()> {
    use uffs_mft::usn::query_usn_journal;

    println!("🔍 Querying USN Journal for {drive}:...");
    println!();

    match query_usn_journal(drive) {
        Ok(info) => {
            println!("=== USN Journal Info ===");
            println!("  Journal ID:       0x{:016X}", info.journal_id);
            println!("  First USN:        {}", info.first_usn);
            println!("  Next USN:         {}", info.next_usn);
            println!("  Lowest Valid USN: {}", info.lowest_valid_usn);
            println!("  Max USN:          {}", info.max_usn);
            println!(
                "  Max Size:         {:.1} MB",
                bytes_to_mb_f64(info.max_size)
            );
            println!(
                "  Alloc Delta:      {:.1} MB",
                bytes_to_mb_f64(info.allocation_delta)
            );
            println!();
            // Rough estimate: USN records average ~64 bytes, so the
            // record count is approximately the raw byte-distance
            // divided by 64.  Subtract through `.raw()` because
            // `Usn - Usn` has no clean type (the result is an i64
            // *count*, not another USN value).
            println!(
                "📊 Journal contains ~{} changes",
                (info.next_usn.raw() - info.first_usn.raw()) / 64
            );
        }
        Err(e) => {
            eprintln!("❌ Failed to query USN Journal: {e}");
            eprintln!();
            eprintln!("Note: USN Journal may not be enabled on this volume.");
            eprintln!(
                "Run as Administrator to enable: fsutil usn createjournal m=1000 a=100 {drive}:"
            );
        }
    }

    Ok(())
}

/// Read recent USN Journal changes for a drive.
#[cfg(windows)]
pub(crate) async fn cmd_usn_read(
    drive: uffs_mft::platform::DriveLetter,
    start_usn: Option<i64>,
    limit: usize,
) -> Result<()> {
    use uffs_mft::usn::{query_usn_journal, read_usn_journal};

    println!("🔍 Reading USN Journal for {drive}:...");
    println!();

    // First query the journal to get the ID
    let info = match query_usn_journal(drive) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("❌ Failed to query USN Journal: {e}");
            return Ok(());
        }
    };

    // CLI surface keeps the raw `Option<i64>` so existing `--start-usn`
    // invocations (e.g. `--start-usn 12345`) parse unchanged; the wrap
    // happens at this single call-boundary.
    let start = start_usn.map_or(info.first_usn, uffs_mft::usn::Usn::new);
    println!(
        "Reading from USN {} (journal ID: 0x{:016X})",
        start, info.journal_id
    );
    println!();

    match read_usn_journal(drive, info.journal_id, start) {
        Ok((records, next_usn)) => {
            println!(
                "=== USN Records ({} found, showing up to {}) ===",
                records.len(),
                limit
            );
            println!();
            println!(
                "{:<12} {:<12} {:<10} {:<40}",
                "FRS", "Parent", "Reason", "Filename"
            );
            println!("{}", "-".repeat(80));

            for record in records.iter().take(limit) {
                let reason_str = format_usn_reason(record.reason);
                println!(
                    "{:<12} {:<12} {:<10} {}",
                    record.frs, record.parent_frs, reason_str, record.filename
                );
            }

            if records.len() > limit {
                println!();
                println!("... and {} more records", records.len() - limit);
            }

            println!();
            println!("Next USN: {next_usn}");
        }
        Err(e) => {
            eprintln!("❌ Failed to read USN Journal: {e}");
        }
    }

    Ok(())
}
