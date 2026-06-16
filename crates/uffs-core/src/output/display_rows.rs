// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Native `DisplayRow` → text formatter with a sequential and a
//! rayon-parallel write path.
//!
//! Extracted from `output/config.rs` so the monolithic
//! [`OutputConfig`] module stays under the 800-LOC file-size policy.
//! The public entry point stays a method on [`OutputConfig`]
//! ([`OutputConfig::write_display_rows`]) and delegates to
//! [`write_display_rows`] here — callers see no API change.
//!
//! Hosts:
//! - [`write_display_rows`] — sequential + parallel write branches.
//! - [`write_display_row_columns`] — the 30-arm column→text dispatch.
//! - [`push_flag`] — boolean flag formatter.
//! - [`append_datetime_native`] — raw-FILETIME → `YYYY-MM-DD HH:MM:SS`.
//! - [`attr`] — NTFS attribute bit constants.

use core::fmt::Write as _;
use std::io::Write;

use rayon::prelude::*;

use super::{BASELINE_COLUMN_ORDER, OutputColumn, OutputConfig};
use crate::error::Result;
use crate::search::backend::DisplayRow;
use crate::search::derived::extension_from_name;

/// Row count above which [`write_display_rows`] parallelises row
/// formatting via rayon chunks.
///
/// Below this threshold the sequential path wins because per-row formatting
/// (~500 ns) is small relative to rayon's chunk dispatch + final write
/// overhead (~1 ms).  Above it, formatting parallelises cleanly across
/// workers while the final write stays sequential into a single
/// `BufWriter`.  Matches the thresholds used in
/// `search::sorting::PARALLEL_SORT_THRESHOLD` and
/// `search::query::numeric_top_n::RESOLVE_CHUNK_SIZE` so the same
/// hot-path crossover governs the whole query.
pub(crate) const PARALLEL_WRITE_THRESHOLD: usize = 16_384;

/// Chunk size used by [`write_display_rows`]'s parallel formatter.
///
/// Chosen so each chunk does ~2 ms of string-building work, well above
/// rayon's per-task dispatch floor.  Larger chunks would underutilise
/// workers; smaller chunks would waste time on scheduler overhead.
pub(crate) const PARALLEL_WRITE_CHUNK: usize = 4096;

/// Write `DisplayRow` results directly — **no `DataFrame` involved**.
///
/// Uses the same separator / quote / header / boolean formatting as
/// [`OutputConfig::write`] so output is identical to the `DataFrame`
/// path.
///
/// Above [`PARALLEL_WRITE_THRESHOLD`] the formatter runs in parallel:
/// rayon fans the rows out to worker-local `Vec<u8>` buffers (one per
/// chunk), then the main thread writes the pre-formatted chunks into
/// the underlying writer in order.  I/O stays sequential to keep the
/// `BufWriter` contract + avoid file-offset races; only the CPU-bound
/// string assembly parallelises.  Below the threshold the
/// single-buffer sequential path wins because the per-chunk
/// `Vec<u8>` allocations + final `collect::<Vec<Vec<u8>>>()`
/// overhead dominates the tiny format work.
///
/// Measured on a 168 K-row path export: write phase drops from ~24 ms
/// sequential to ~10 ms with 8 rayon workers, and the threshold
/// fallback keeps small queries untouched.
///
/// # Errors
///
/// Returns an error if the underlying writer fails.
pub(crate) fn write_display_rows<W: Write>(
    cfg: &OutputConfig,
    rows: &[DisplayRow],
    mut writer: W,
) -> Result<()> {
    let output_cols: &[OutputColumn] = cfg
        .columns
        .as_ref()
        .map_or(BASELINE_COLUMN_ORDER, |cols| cols.as_slice());

    // Header
    if cfg.header {
        let mut header = String::with_capacity(output_cols.len() * 24);
        for (idx, col) in output_cols.iter().enumerate() {
            if idx > 0 {
                header.push_str(&cfg.separator);
            }
            header.push_str(&cfg.quote);
            header.push_str(col.display_name());
            header.push_str(&cfg.quote);
        }
        header.push('\n');
        header.push('\n');
        writer.write_all(header.as_bytes())?;
    }

    // Data rows.
    if rows.len() >= PARALLEL_WRITE_THRESHOLD {
        let chunk_bufs: Vec<Vec<u8>> = rows
            .par_chunks(PARALLEL_WRITE_CHUNK)
            .map(|chunk| {
                // Pre-size the per-chunk buffer for a typical
                // ~128-byte formatted row to avoid repeated growth
                // during `extend_from_slice`.
                let mut chunk_buf: Vec<u8> = Vec::with_capacity(chunk.len() * 128);
                let mut scratch = String::with_capacity(output_cols.len() * 32);
                let mut itoa_buf = itoa::Buffer::new();
                for row in chunk {
                    scratch.clear();
                    write_display_row_columns(&mut scratch, &mut itoa_buf, output_cols, cfg, row);
                    scratch.push('\n');
                    chunk_buf.extend_from_slice(scratch.as_bytes());
                }
                chunk_buf
            })
            .collect();
        for chunk_buf in &chunk_bufs {
            writer.write_all(chunk_buf)?;
        }
    } else {
        let mut buf = String::with_capacity(output_cols.len() * 32);
        let mut itoa_buf = itoa::Buffer::new();
        for row in rows {
            buf.clear();
            write_display_row_columns(&mut buf, &mut itoa_buf, output_cols, cfg, row);
            buf.push('\n');
            writer.write_all(buf.as_bytes())?;
        }
    }

    Ok(())
}

/// NTFS attribute flag constants for bit-testing `DisplayRow::flags`.
pub(crate) mod attr {
    /// Read-only.
    pub(crate) const READONLY: u32 = 0x0001;
    /// Hidden.
    pub(crate) const HIDDEN: u32 = 0x0002;
    /// System.
    pub(crate) const SYSTEM: u32 = 0x0004;
    /// Directory.
    pub(crate) const DIRECTORY: u32 = 0x0010;
    /// Archive.
    pub(crate) const ARCHIVE: u32 = 0x0020;
    /// Temporary.
    pub(crate) const TEMPORARY: u32 = 0x0100;
    /// Sparse.
    pub(crate) const SPARSE: u32 = 0x0200;
    /// Reparse point.
    pub(crate) const REPARSE: u32 = 0x0400;
    /// Compressed.
    pub(crate) const COMPRESSED: u32 = 0x0800;
    /// Offline.
    pub(crate) const OFFLINE: u32 = 0x1000;
    /// Not content indexed.
    pub(crate) const NOT_INDEXED: u32 = 0x2000;
    /// Encrypted.
    pub(crate) const ENCRYPTED: u32 = 0x4000;
    /// Integrity stream.
    pub(crate) const INTEGRITY: u32 = 0x8000;
    /// Virtual.
    pub(crate) const VIRTUAL: u32 = 0x0001_0000;
    /// No scrub data.
    pub(crate) const NO_SCRUB: u32 = 0x0002_0000;
    /// Recall on open.
    pub(crate) const RECALL_ON_OPEN: u32 = 0x0004_0000;
    /// Pinned.
    pub(crate) const PINNED: u32 = 0x0008_0000;
    /// Unpinned.
    pub(crate) const UNPINNED: u32 = 0x0010_0000;
    /// Recall on data access.
    pub(crate) const RECALL_ON_DATA: u32 = 0x0040_0000;
    /// Parity-compat mask — must match `StandardInfo::parity_attributes()`.
    ///
    /// Includes the 15 attribute bits the legacy baseline tracks:
    /// `READONLY` | `HIDDEN` | `SYSTEM` | `DIRECTORY` | `ARCHIVE` | `SPARSE` |
    /// `REPARSE` | `COMPRESSED` | `OFFLINE` | `NOT_INDEXED` | `ENCRYPTED` |
    /// `INTEGRITY` | `NO_SCRUB` | `PINNED` | `UNPINNED`.
    ///
    /// Note: excludes `TEMPORARY` (0x100) and `VIRTUAL` (0x10000) which are
    /// NOT part of the parity contract.
    pub(crate) const PARITY_MASK: u32 = READONLY
        | HIDDEN
        | SYSTEM
        | DIRECTORY
        | ARCHIVE
        | SPARSE
        | REPARSE
        | COMPRESSED
        | OFFLINE
        | NOT_INDEXED
        | ENCRYPTED
        | INTEGRITY
        | NO_SCRUB
        | PINNED
        | UNPINNED;
}

/// Write one `DisplayRow` into `buf` using the configured columns.
///
/// Extracted as a standalone function for readability — the column match has
/// ~30 arms mirroring all `OutputColumn` variants.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive match over ~30 OutputColumn variants; each arm is 1–8 lines \
              of formatting — splitting would scatter the column→text dispatch table"
)]
pub(crate) fn write_display_row_columns(
    buf: &mut String,
    itoa_buf: &mut itoa::Buffer,
    output_cols: &[OutputColumn],
    cfg: &OutputConfig,
    row: &DisplayRow,
) {
    let flags = row.flags;
    // Parity-compat: directories get trailing `\`, empty name, self-path.
    let parity_dir = cfg.parity_compat && row.is_directory;

    for (idx, col) in output_cols.iter().enumerate() {
        if idx > 0 {
            buf.push_str(&cfg.separator);
        }
        match col {
            OutputColumn::Path => {
                buf.push_str(&cfg.quote);
                buf.push_str(&row.path);
                if parity_dir && !row.path.ends_with('\\') {
                    buf.push('\\');
                }
                buf.push_str(&cfg.quote);
            }
            OutputColumn::Name => {
                buf.push_str(&cfg.quote);
                if !parity_dir {
                    buf.push_str(row.name());
                }
                // parity dirs: empty name (just quotes)
                buf.push_str(&cfg.quote);
            }
            OutputColumn::PathOnly => {
                buf.push_str(&cfg.quote);
                if parity_dir {
                    // Legacy: PathOnly = full path with trailing `\`
                    buf.push_str(&row.path);
                    if !row.path.ends_with('\\') {
                        buf.push('\\');
                    }
                } else if let Some(pos) = row.path.rfind('\\') {
                    buf.push_str(row.path.get(..=pos).unwrap_or(&row.path));
                } else {
                    buf.push_str(&row.path);
                }
                buf.push_str(&cfg.quote);
            }
            OutputColumn::Size => {
                if parity_dir {
                    buf.push_str(itoa_buf.format(row.treesize));
                } else {
                    buf.push_str(itoa_buf.format(row.size));
                }
            }
            OutputColumn::SizeOnDisk => {
                if parity_dir {
                    buf.push_str(itoa_buf.format(row.tree_allocated));
                } else {
                    buf.push_str(itoa_buf.format(row.allocated));
                }
            }
            OutputColumn::Created => {
                append_datetime_native(buf, row.created, cfg.timezone_offset_secs);
            }
            OutputColumn::Modified => {
                append_datetime_native(buf, row.modified, cfg.timezone_offset_secs);
            }
            OutputColumn::Accessed => {
                append_datetime_native(buf, row.accessed, cfg.timezone_offset_secs);
            }
            OutputColumn::Descendants => {
                buf.push_str(itoa_buf.format(row.descendants));
            }
            OutputColumn::TreeSize => {
                buf.push_str(itoa_buf.format(row.treesize));
            }
            OutputColumn::TreeAllocated => {
                buf.push_str(itoa_buf.format(row.tree_allocated));
            }
            OutputColumn::Type => {
                buf.push_str(&cfg.quote);
                buf.push_str(crate::search::derived::semantic_type_for_row(row));
                buf.push_str(&cfg.quote);
            }
            OutputColumn::Attributes | OutputColumn::AttributeValue => {
                buf.push_str(itoa_buf.format(flags));
            }
            OutputColumn::ParityAttributes => {
                buf.push_str(itoa_buf.format(flags & attr::PARITY_MASK));
            }
            OutputColumn::Hidden => push_flag(buf, cfg, flags, attr::HIDDEN),
            OutputColumn::System => push_flag(buf, cfg, flags, attr::SYSTEM),
            OutputColumn::Archive => push_flag(buf, cfg, flags, attr::ARCHIVE),
            OutputColumn::ReadOnly => push_flag(buf, cfg, flags, attr::READONLY),
            OutputColumn::Compressed => push_flag(buf, cfg, flags, attr::COMPRESSED),
            OutputColumn::Encrypted => push_flag(buf, cfg, flags, attr::ENCRYPTED),
            OutputColumn::Sparse => push_flag(buf, cfg, flags, attr::SPARSE),
            OutputColumn::Reparse => push_flag(buf, cfg, flags, attr::REPARSE),
            OutputColumn::Offline => push_flag(buf, cfg, flags, attr::OFFLINE),
            OutputColumn::NotIndexed => push_flag(buf, cfg, flags, attr::NOT_INDEXED),
            OutputColumn::Temporary => push_flag(buf, cfg, flags, attr::TEMPORARY),
            OutputColumn::Virtual => push_flag(buf, cfg, flags, attr::VIRTUAL),
            OutputColumn::Pinned => push_flag(buf, cfg, flags, attr::PINNED),
            OutputColumn::Unpinned => push_flag(buf, cfg, flags, attr::UNPINNED),
            OutputColumn::DirectoryFlag => push_flag(buf, cfg, flags, attr::DIRECTORY),
            OutputColumn::Integrity => push_flag(buf, cfg, flags, attr::INTEGRITY),
            OutputColumn::NoScrub => push_flag(buf, cfg, flags, attr::NO_SCRUB),
            OutputColumn::RecallOnOpen => push_flag(buf, cfg, flags, attr::RECALL_ON_OPEN),
            OutputColumn::RecallOnDataAccess => push_flag(buf, cfg, flags, attr::RECALL_ON_DATA),
            OutputColumn::Bulkiness => {
                // Allocated-to-logical ratio × 100 (integer percentage).
                // 100 = perfectly packed. >100 = cluster slack / waste.
                // 0 for zero-byte files. Directories use tree metrics.
                let per_million = crate::search::derived::bulkiness_for_row(row);
                // Convert per-million → percentage (÷ 10_000).
                let pct = per_million / 10_000;
                buf.push_str(itoa_buf.format(pct));
            }
            OutputColumn::Drive => {
                buf.push(row.drive.as_char());
            }
            OutputColumn::Extension => {
                buf.push_str(&cfg.quote);
                // Dot-gated: dotfiles (`.bash_history`), dotless names
                // (`README`), and trailing-dot names (`foo.`) emit an
                // empty extension so the displayed value matches the
                // sort engine's key (`search::sorting::build_row_sort_key`)
                // and the indexer's `intern_extension` semantics.
                if let Some(ext) = extension_from_name(row.name()) {
                    buf.push_str(ext);
                }
                buf.push_str(&cfg.quote);
            }
            // Newly added columns that have no dedicated text formatter yet.
            OutputColumn::NameLength => {
                let len = uffs_mft::len_to_u16(row.name().chars().count());
                buf.push_str(itoa_buf.format(len));
            }
            OutputColumn::PathLength => {
                let len = uffs_mft::len_to_u16(row.path.chars().count());
                buf.push_str(itoa_buf.format(len));
            }
            // ── WI-4.4 forensic columns ────────────────────────────────
            // Rendered as 0/1 in their OWN column (like the attribute flags),
            // never inline with the name. The booleans are precomputed on the
            // hot path against the lossless name bytes and carried on the row.
            OutputColumn::Malformed => push_bool(buf, cfg, row.malformed),
            OutputColumn::MalformedPath => push_bool(buf, cfg, row.malformed_path),
            OutputColumn::NameHex => {
                buf.push_str(&cfg.quote);
                if let Some(hex) = row.name_hex.as_deref() {
                    buf.push_str(hex);
                }
                buf.push_str(&cfg.quote);
            }
        }
    }
}

/// Append a boolean flag test result.
pub(crate) fn push_flag(buf: &mut String, cfg: &OutputConfig, flags: u32, mask: u32) {
    if flags & mask != 0 {
        buf.push_str(&cfg.pos);
    } else {
        buf.push_str(&cfg.neg);
    }
}

/// Append a precomputed boolean as the configured pos/neg token (mirrors
/// [`push_flag`] but for a `bool` not backed by an attribute-flag mask).
pub(crate) fn push_bool(buf: &mut String, cfg: &OutputConfig, value: bool) {
    if value {
        buf.push_str(&cfg.pos);
    } else {
        buf.push_str(&cfg.neg);
    }
}

/// Append `YYYY-MM-DD HH:MM:SS` from a raw FILETIME (100-ns ticks since
/// 1601-01-01) with timezone offset.
///
/// v13+ of the compact index stores timestamps as **raw FILETIME** (matching
/// the C++ NTFS baseline), not Unix microseconds.  Callers in this file
/// pass `row.modified` / `row.created` / `row.accessed` which are FILETIME
/// values — previously this function mis-interpreted them as Unix
/// microseconds and produced year-6220 output for 2026-era timestamps (the
/// ~369-year + 10× unit offset between the two encodings).
///
/// Delegates to `uffs_time::filetime_with_tz_bias` + `filetime_to_calendar`
/// for the canonical Hinnant civil-calendar decomposition — same helpers
/// used by the parity-compat CSV writer in
/// `uffs_cli::commands::output::parity::append_datetime_tz`.
///
/// Regression-pinned by `append_datetime_native_*` in the `tests`
/// submodule at `display_rows_tests.rs`.
pub(crate) fn append_datetime_native(buf: &mut String, filetime: i64, tz_offset_secs: i32) {
    let local_ft = uffs_time::filetime_with_tz_bias(filetime, tz_offset_secs);
    if let Some(uffs_time::CalendarParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }) = uffs_time::filetime_to_calendar(local_ft)
    {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "String::write_fmt never fails"
        )]
        let _ = write!(
            buf,
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        );
    } else {
        // `filetime == 0` (unset / null) — surface as the zero sentinel.
        buf.push_str("0000-00-00 00:00:00");
    }
}

#[cfg(test)]
#[path = "display_rows_tests.rs"]
mod tests;
