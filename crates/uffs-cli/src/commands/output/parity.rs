// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Parity-compat 25-column CSV output, timestamp formatting, and legacy footer.

use std::io::Write;

use anyhow::Result;
use serde_json::Value;

use super::{CppFooterContext, ParityContext, parity_flags, vb, vi, vs, vu, vu32};

/// Parity-compat CSV header (25 columns, matching legacy baseline).
pub(super) const PARITY_HEADER: &[&str] = &[
    "Path",
    "Name",
    "Path Only",
    "Size",
    "Size on Disk",
    "Created",
    "Last Written",
    "Last Accessed",
    "Descendants",
    "Read-only",
    "Archive",
    "System",
    "Hidden",
    "Offline",
    "Not content indexed file",
    "No scrub file",
    "Integrity",
    "Pinned",
    "Unpinned",
    "Directory Flag",
    "Compressed",
    "Encrypted",
    "Sparse",
    "Reparse",
    "Attributes",
];

/// Boolean columns in parity order (matches `PARITY_HEADER[9..25]`).
const PARITY_BOOL_FLAGS: &[u32] = &[
    parity_flags::READONLY,
    parity_flags::ARCHIVE,
    parity_flags::SYSTEM,
    parity_flags::HIDDEN,
    parity_flags::OFFLINE,
    parity_flags::NOT_INDEXED,
    parity_flags::NO_SCRUB,
    parity_flags::INTEGRITY,
    parity_flags::PINNED,
    parity_flags::UNPINNED,
    parity_flags::DIRECTORY,
    parity_flags::COMPRESSED,
    parity_flags::ENCRYPTED,
    parity_flags::SPARSE,
    parity_flags::REPARSE,
];

/// Write parity-compat 25-column CSV output from `SearchRow` data.
pub(super) fn write_parity<W: Write>(
    writer: &mut W,
    rows: &[Value],
    separator: &str,
    quote: &str,
    ctx: &ParityContext<'_>,
) -> Result<()> {
    let mut header = String::with_capacity(512);
    for (idx, col) in PARITY_HEADER.iter().enumerate() {
        if idx > 0 {
            header.push_str(separator);
        }
        header.push_str(quote);
        header.push_str(col);
        header.push_str(quote);
    }
    header.push('\n');
    header.push('\n');
    writer.write_all(header.as_bytes())?;

    let mut buf = String::with_capacity(512);
    for row in rows {
        buf.clear();
        write_parity_row(&mut buf, row, separator, quote, ctx);
        buf.push('\n');
        writer.write_all(buf.as_bytes())?;
    }
    Ok(())
}

/// Write a single parity-compat CSV row.
fn write_parity_row(
    buf: &mut String,
    row: &Value,
    sep: &str,
    quote: &str,
    ctx: &ParityContext<'_>,
) {
    let is_dir = vb(row, "is_directory");
    let flags = vu32(row, "flags");
    let path = vs(row, "path");
    let name = vs(row, "name");

    // 0: Path (quoted, trailing \ for dirs)
    buf.push_str(quote);
    buf.push_str(&path);
    if is_dir && !path.ends_with('\\') {
        buf.push('\\');
    }
    buf.push_str(quote);

    // 1: Name (quoted, empty for dirs)
    buf.push_str(sep);
    buf.push_str(quote);
    if !is_dir {
        buf.push_str(&name);
    }
    buf.push_str(quote);

    // 2: PathOnly (quoted)
    buf.push_str(sep);
    buf.push_str(quote);
    if is_dir {
        buf.push_str(&path);
        if !path.ends_with('\\') {
            buf.push('\\');
        }
    } else if let Some(pos) = path.rfind('\\') {
        if let Some(slice) = path.get(..=pos) {
            buf.push_str(slice);
        }
    } else {
        buf.push_str(&path);
    }
    buf.push_str(quote);

    // 3: Size (treesize for dirs)
    buf.push_str(sep);
    push_u64(
        buf,
        if is_dir {
            vu(row, "treesize")
        } else {
            vu(row, "size")
        },
    );

    // 4: SizeOnDisk (tree_allocated for dirs)
    buf.push_str(sep);
    push_u64(
        buf,
        if is_dir {
            vu(row, "tree_allocated")
        } else {
            vu(row, "allocated")
        },
    );

    // 5-7: Created, Modified, Accessed
    for key in &["created", "modified", "accessed"] {
        buf.push_str(sep);
        append_datetime_tz(buf, vi(row, key), ctx.tz_offset_secs);
    }

    // 8: Descendants
    buf.push_str(sep);
    push_u64(buf, vu(row, "descendants"));

    // 9-23: Boolean flag columns (15 columns)
    for &flag in PARITY_BOOL_FLAGS {
        buf.push_str(sep);
        buf.push_str(if flags & flag != 0 { ctx.pos } else { ctx.neg });
    }

    // 24: ParityAttributes (masked to 15 bits)
    buf.push_str(sep);
    push_u64(buf, u64::from(flags & parity_flags::PARITY_MASK));
}

/// Append a `u64` value to a string buffer without allocation.
fn push_u64(buf: &mut String, value: u64) {
    use core::fmt::Write as _;
    let _ok = write!(buf, "{value}");
}

/// Format a raw FILETIME with timezone bias directly into `buf`.
///
/// Mirrors C++ `RtlTimeToTimeFields` — applies TZ bias in FILETIME ticks,
/// then decomposes.  No intermediate Unix conversion.
///
/// When `filetime` is `0` (the "unset / null" sentinel that
/// `filetime_to_calendar` returns `None` for), emits
/// `"0000-00-00 00:00:00"` — matches `uffs_format::append_datetime_native`
/// so the CLI's `write_parity` output stays byte-identical with the
/// daemon's `uffs_format::write_rows` for the `ParityCompat +
/// Created/Modified/Accessed` combination that
/// `RequestHandler::try_pack_csv_blob` now pre-formats (v0.5.64+).
fn append_datetime_tz(buf: &mut String, filetime: i64, tz_offset_secs: i32) {
    use core::fmt::Write as _;
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
        let _ok = write!(
            buf,
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        );
    } else {
        buf.push_str("0000-00-00 00:00:00");
    }
}

/// Append the legacy drive footer for baseline-compatible custom output.
///
/// Thin shim over [`uffs_format::write_legacy_drive_footer`] — the
/// canonical implementation lives in `uffs-format` so the daemon's
/// `RequestHandler::try_pack_csv_blob` fast path and this CLI slow
/// path emit byte-identical footer bytes.  Any change to the footer
/// shape (CRLF, fast-scan heuristic, drive-letter formatting) MUST
/// go through [`uffs_format::write_legacy_drive_footer`] (the public
/// entry point of the now-`pub(crate)` `uffs_format::footer` module)
/// so both sites pick it up.
///
/// # Errors
///
/// Propagates any I/O error the underlying writer returns.
pub(super) fn write_legacy_drive_footer<W: Write + ?Sized>(
    writer: &mut W,
    ctx: &CppFooterContext<'_>,
) -> Result<()> {
    let fmt_ctx = uffs_format::DriveFooterContext {
        output_targets: ctx.output_targets,
        pattern: ctx.pattern,
        row_count: ctx.row_count,
    };
    uffs_format::write_legacy_drive_footer(writer, &fmt_ctx).map_err(Into::into)
}
