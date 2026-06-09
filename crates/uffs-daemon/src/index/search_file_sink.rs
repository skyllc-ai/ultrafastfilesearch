// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Direct file output for `--out=<path>` search exports (OPT-4).
//!
//! Extracted from the parent `search.rs` to keep that file under the
//! workspace 800-LOC file-size policy.  The single caller in
//! `IndexManager::search` invokes `write_rows_to_file` directly (it was a
//! `Self`-less associated fn), so the move is a pure relocation with no
//! behavioural change.

use uffs_core::search::backend::DisplayRow;

/// Write `DisplayRow`s directly to a file, bypassing `SearchRow` and IPC.
///
/// Uses the same `OutputConfig::write_display_rows` that the CLI uses,
/// so all formatting options (separator, quotes, header, pos/neg,
/// columns, timestamps) produce identical output.
///
/// Atomic write: writes to a `.uffs.tmp` sibling file, then renames
/// to the target after a `BufWriter::flush`.  No `fsync` —
/// `--out=<path>` is reproducible search output, so the tmp+rename
/// dance protects against partial-file exposure during normal
/// writes but power-loss durability is intentionally not provided.
/// See the inline comment in the body and §Run 7 C / §Run 8 of
/// `docs/research/perf-phase2-measurement-plan.md` for the
/// measurement that motivated this trade-off.  Zero rows → no
/// file is created.
pub(super) fn write_rows_to_file(
    rows: &[DisplayRow],
    path: &str,
    output_config: &uffs_core::output::OutputConfig,
) -> Result<usize, std::io::Error> {
    use std::io::{BufWriter, Write as _};

    use rand::Rng as _;

    // Zero results → don't create the file at all.
    if rows.is_empty() {
        return Ok(0);
    }

    let target = std::path::Path::new(path);

    // Randomised temp name in the same directory (same-FS rename stays
    // atomic). The random suffix + exclusive `create_new` open refuse to
    // follow a symlink pre-planted at a guessed temp path. `file_name`
    // is an `Option`, not a `Result` — `unwrap_or_default` is not an
    // unwrap-lint violation.
    let mut suffix_bytes = [0_u8; 8];
    rand::rng().fill_bytes(&mut suffix_bytes);
    let suffix = u64::from_le_bytes(suffix_bytes);
    let file_name = target.file_name().unwrap_or_default();
    let tmp_name = format!("{}.{:016x}.uffs.tmp", file_name.to_string_lossy(), suffix);
    let tmp_path = target.with_file_name(tmp_name);

    // Write to temp file — target is untouched until rename.
    //
    // `--out=<path>` is a user-chosen export, NOT a secret: it must adopt
    // the directory's normal permissions, not the daemon owner's. We use
    // `create_new_file_exclusive` (exclusive create, no owner-only ACL)
    // rather than `create_new_secure_file` — the latter applies a Windows
    // owner-only ACL that historically shelled out to `icacls.exe`,
    // adding a per-query process-spawn (~tens of ms) to every result
    // write. The exclusive create still closes the symlink/TOCTOU window.
    let file = uffs_security::fs::create_new_file_exclusive(&tmp_path)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);

    let write_result = output_config
        .write_display_rows(rows, &mut writer)
        .map_err(std::io::Error::other);

    // On write error, clean up the temp file and propagate.
    if let Err(err) = write_result {
        drop(writer);
        let _cleanup: Result<(), std::io::Error> = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    // Flush the BufWriter and close the underlying file.
    //
    // We deliberately skip `sync_all()` here.  `--out=<path>` is
    // a user-requested export of search results; the data is
    // reproducible from the MFT index in ~100 ms, so paying a
    // 5-15 ms `fsync` per query for power-loss durability is not
    // worth it — a power cut would just leave a 0-byte file and
    // the user can simply re-run the query.  The atomic
    // tmp+rename below still prevents partial-file exposure
    // during normal writes.  See
    // `docs/research/perf-phase2-measurement-plan.md` §Run 7 C /
    // §Run 8 for the measurement that motivated dropping the
    // sync.
    writer.flush()?;
    writer
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    // The File temporary above is dropped at the semicolon,
    // closing the OS handle before the rename below.

    // Atomic rename: target appears only with complete data.
    std::fs::rename(&tmp_path, target)?;

    Ok(rows.len())
}
