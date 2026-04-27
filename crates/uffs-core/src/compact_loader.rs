// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MFT data loading, caching, and refresh logic for compact indices.
//!
//! Handles cold reads (file/live), IOCP detection, `.uffs` cache integration,
//! compact-cache integration, and USN-based incremental patching.

use std::path::PathBuf;
use std::time::Instant;

use uffs_mft::index::MftIndex;

#[cfg(windows)]
use crate::compact::{ChildrenIndex, CompactRecord};
use crate::compact::{DriveCompactIndex, INDEX_TTL_SECONDS, build_compact_index};
#[cfg(windows)]
use crate::trigram::TrigramIndex;

/// What produced a given `DriveCompactIndex`.
#[derive(Clone)]
pub enum IndexSource {
    /// Raw/IOCP/compressed MFT file.
    MftFile(PathBuf),
}

/// Timing breakdown for the compact index build.
pub struct LoadTiming {
    /// Time to deserialize the compact cache (milliseconds, 0 if cache miss).
    pub cache: u128,
    /// Time to load/read the MFT (milliseconds, 0 if cache hit).
    pub mft: u128,
    /// Time to build compact records from `MftIndex` (milliseconds, 0 if cache
    /// hit).
    pub compact: u128,
    /// Time to build trigram index (milliseconds, 0 if cache hit).
    pub trigram: u128,
}

/// Where to read MFT data from.
#[derive(Debug, Clone)]
pub enum MftSource {
    /// Offline MFT file (`.uffs`, `.raw`, `.iocp` capture).
    /// Second field is an optional drive-letter override.
    File(PathBuf, Option<char>),
    /// Live Windows NTFS volume (e.g., `'C'`).
    #[cfg(windows)]
    Live(char),
}

impl MftSource {
    /// Returns the file path if this is a `File` source.
    #[must_use]
    #[cfg_attr(
        not(windows),
        expect(
            clippy::unnecessary_wraps,
            reason = "returns None for MftSource::Live on Windows"
        )
    )]
    pub fn file_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::File(path, _) => Some(path),
            #[cfg(windows)]
            Self::Live(_) => None,
        }
    }
}

/// Unified entry point: load MFT data from any source and build a compact
/// index.
///
/// Handles compact cache → MFT cache → cold read → save caches,
/// with `[CACHE_PROFILE]` profiling when `UFFS_CACHE_PROFILE=1`.
///
/// # Errors
///
/// Returns an error if the MFT data cannot be read or parsed.
pub fn load_drive(
    source: &MftSource,
    no_cache: bool,
) -> anyhow::Result<(DriveCompactIndex, LoadTiming)> {
    let drive_letter = match source {
        MftSource::File(path, drive_override) => drive_override.unwrap_or_else(|| {
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("X");
            stem.chars()
                .next()
                .filter(char::is_ascii_alphabetic)
                .map_or('X', |ch| ch.to_ascii_uppercase())
        }),
        #[cfg(windows)]
        MftSource::Live(ch) => *ch,
    };

    // ── Fast path: compact cache hit ───────────────────────────────
    if !no_cache && let Some(result) = try_compact_cache_hit(drive_letter, source) {
        return Ok(result);
    }

    // ── Load MftIndex (cache or cold) ──────────────────────────────
    let mft_start = Instant::now();
    let mft_index = match source {
        MftSource::File(path, _) => load_mft_index_from_file(path, drive_letter, no_cache)?,
        #[cfg(windows)]
        MftSource::Live(ch) => load_mft_index_live(*ch, no_cache)?,
    };
    let mft_elapsed = mft_start.elapsed().as_millis();

    // ── Build compact index ────────────────────────────────────────
    let (mut compact, compact_elapsed, tri_elapsed) = build_compact_index(drive_letter, &mft_index);

    // Log per-component heap footprint.
    compact.log_heap_report();

    // Free MftIndex (~1.6 GB for 7M records) now that compact is built.
    drop(mft_index);

    if let Some(path) = source.file_path() {
        compact.source = IndexSource::MftFile(path.to_path_buf());
    }

    // ── Save compact cache (background, best-effort) ────────────────
    if !no_cache {
        let t_compact_save = Instant::now();
        if let Err(err) = crate::compact_cache::save_compact_cache_background(&compact) {
            tracing::warn!(drive = %drive_letter, error = %err, "Failed to start compact cache save");
        }
        let compact_save_ms = t_compact_save.elapsed().as_millis();
        tracing::debug!(
            target: "cache_profile",
            compact_save_submit_ms = %compact_save_ms,
            "compact_save_submit (serialized, bg thread spawned)"
        );
    }

    Ok((compact, LoadTiming {
        cache: 0,
        mft: mft_elapsed,
        compact: compact_elapsed,
        trigram: tri_elapsed,
    }))
}

/// Attempt to load a compact index from cache (TTL-only, no mtime validation).
///
/// Returns `Some((index, timing))` on cache hit, `None` on miss.
#[expect(
    clippy::single_call_fn,
    reason = "extracted for readability from load_drive"
)]
fn try_compact_cache_hit(
    drive_letter: char,
    source: &MftSource,
) -> Option<(DriveCompactIndex, LoadTiming)> {
    let cache_start = Instant::now();
    let mut compact =
        crate::compact_cache::load_compact_cache(drive_letter, INDEX_TTL_SECONDS, 0, true)?;
    let cache_ms = cache_start.elapsed().as_millis();

    if let Some(path) = source.file_path() {
        compact.source = IndexSource::MftFile(path.to_path_buf());
    }
    tracing::info!(
        drive = %drive_letter,
        records = compact.records.len(),
        cache_ms,
        "📦 Cache hit — loaded compact cache"
    );
    Some((compact, LoadTiming {
        cache: cache_ms,
        mft: 0,
        compact: 0,
        trigram: 0,
    }))
}

/// Parse a `.uffs` MFT file into `MftIndex`, choosing the parser that
/// matches the file format.
///
/// IOCP captures must use `load_iocp_to_index` (unified `process_record`
/// path) which mirrors the Windows LIVE inline parser exactly.  The
/// generic `load_raw_to_index_with_options` dispatches IOCP to
/// `load_iocp_capture_to_index` (`MftRecordMerger` multi-pass path) which
/// produces different `total_stream_count` values and therefore
/// different tree metrics (descendants, treesize) — a known parity
/// divergence that we avoid by picking the right parser up front.
fn parse_mft_file_to_index(
    mft_path: &std::path::Path,
    drive_letter: char,
) -> anyhow::Result<MftIndex> {
    // `?` performs the `MftError → anyhow::Error` conversion via
    // `From<MftError> for anyhow::Error`, matching the original
    // inline call site's behaviour.
    let is_iocp = uffs_mft::is_iocp_capture(mft_path).unwrap_or(false);
    if is_iocp {
        tracing::info!(
            drive = %drive_letter,
            "📼 IOCP capture detected — using unified process_record parser for parity"
        );
        return Ok(uffs_mft::load_iocp_to_index(mft_path)?);
    }
    let options = uffs_mft::raw::LoadRawOptions {
        header_only: false,
        volume_letter: Some(drive_letter),
        forensic: false,
    };
    Ok(uffs_mft::MftReader::load_raw_to_index_direct(
        mft_path, &options,
    )?)
}

/// Kick off the post-parse background cache save and emit a matching
/// tracing line for success or failure.
fn spawn_mft_cache_save(index: &MftIndex, drive_letter: char) {
    match uffs_mft::cache::save_to_cache_background(index, drive_letter, 0, 0, 0) {
        Ok(()) => {
            tracing::info!(drive = %drive_letter, "💾 MFT cache save started (background)");
        }
        Err(err) => {
            tracing::warn!(
                drive = %drive_letter,
                error = %err,
                "Failed to start .uffs cache save"
            );
        }
    }
}

/// Load `MftIndex` from an offline file (cache → cold parse).
fn load_mft_index_from_file(
    mft_path: &std::path::Path,
    drive_letter: char,
    no_cache: bool,
) -> anyhow::Result<MftIndex> {
    let cached = if no_cache {
        None
    } else {
        uffs_mft::cache::load_cached_index(drive_letter, INDEX_TTL_SECONDS)
    };
    if let Some((cached_index, _header)) = cached {
        tracing::info!(
            drive = %drive_letter,
            records = cached_index.records.len(),
            "📦 Cache hit — loaded .uffs cache"
        );
        return Ok(cached_index);
    }

    tracing::info!(
        drive = %drive_letter,
        path = %mft_path.display(),
        "📖 Parsing MFT file (delegating to uffs-mft)"
    );
    let parsed = parse_mft_file_to_index(mft_path, drive_letter)?;
    spawn_mft_cache_save(&parsed, drive_letter);
    Ok(parsed)
}

/// Load `MftIndex` from a live Windows volume (cache → cold read via IOCP).
#[cfg(windows)]
#[expect(
    clippy::single_call_fn,
    reason = "extracted for readability from load_drive"
)]
fn load_mft_index_live(drive_letter: char, no_cache: bool) -> anyhow::Result<MftIndex> {
    use anyhow::Context;

    let read_index = async {
        let reader = uffs_mft::MftReader::open(drive_letter)
            .with_context(|| format!("Failed to open drive {drive_letter}:"))?;
        if no_cache {
            reader
                .read_all_index()
                .await
                .with_context(|| format!("Failed to read MFT fresh for drive {drive_letter}:"))
        } else {
            reader
                .read_index_cached(INDEX_TTL_SECONDS)
                .await
                .with_context(|| format!("Failed to read MFT for drive {drive_letter}:"))
        }
    };

    // Run the async MFT read synchronously.
    //
    // This function is called from multiple contexts:
    //   - Tokio worker threads (CLI `#[tokio::main]`)
    //   - Tokio blocking threads (daemon `JoinSet::spawn_blocking`)
    //   - No runtime at all (standalone tests)
    //
    // A dedicated current-thread runtime is always safe regardless of the
    // calling context.  The ~50µs overhead is negligible against seconds of
    // MFT I/O.  This avoids `block_in_place` (panics on blocking threads)
    // and `handle.block_on` (panics inside a runtime context).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(read_index)
}

/// Statistics from in-place USN patching.
#[derive(Debug, Clone, Default)]
pub struct PatchStats {
    /// Records marked as deleted (`name_len` zeroed).
    pub deleted: usize,
    /// New records appended.
    pub created: usize,
    /// Records with updated name/parent.
    pub renamed: usize,
    /// Changes skipped (FRS not in index, or no actionable change).
    pub skipped: usize,
}

/// Refresh a drive by reloading from its original source.
///
/// # Errors
///
/// Returns an error if the drive source cannot be reloaded.
pub fn refresh_drive(drive: &DriveCompactIndex) -> anyhow::Result<(DriveCompactIndex, LoadTiming)> {
    match &drive.source {
        IndexSource::MftFile(path) => {
            let source = if path.to_string_lossy().len() <= 2 {
                #[cfg(windows)]
                {
                    MftSource::Live(drive.letter)
                }
                #[cfg(not(windows))]
                {
                    anyhow::bail!("Cannot refresh live drive {}: on non-Windows", drive.letter);
                }
            } else {
                MftSource::File(path.clone(), Some(drive.letter))
            };
            load_drive(&source, false)
        }
    }
}

/// Convenience wrapper for loading an MFT file.
///
/// **Deprecated:** Use [`load_drive`] with [`MftSource::File`] instead.
///
/// # Errors
///
/// Returns an error if the MFT file cannot be read or parsed.
#[deprecated(note = "Use load_drive(MftSource::File(...)) instead")]
pub fn load_mft_file(
    mft_path: &std::path::Path,
    drive: Option<char>,
    no_cache: bool,
) -> anyhow::Result<(DriveCompactIndex, LoadTiming)> {
    load_drive(&MftSource::File(mft_path.to_path_buf(), drive), no_cache)
}

/// Load a live NTFS drive and build a compact index (Windows only).
///
/// **Deprecated:** Use [`load_drive`] with [`MftSource::Live`] instead.
///
/// # Errors
///
/// Returns an error if the drive cannot be read.
#[cfg(windows)]
#[deprecated(note = "Use load_drive(MftSource::Live(...)) instead")]
pub fn load_live_drive(
    drive_letter: char,
    no_cache: bool,
) -> anyhow::Result<(DriveCompactIndex, LoadTiming)> {
    load_drive(&MftSource::Live(drive_letter), no_cache)
}

/// Apply USN changes in-place to the compact index.
///
/// Mutates records (`parent_idx`, names, flags) then rebuilds the children CSR
/// once at the end.  Typical cost: <5ms for record mutations + ~100ms for CSR
/// rebuild on a 7M-record drive.
#[cfg(windows)]
pub fn apply_usn_patch(
    drive: &mut DriveCompactIndex,
    changes: &[uffs_mft::usn::FileChange],
    frs_to_compact: &[u32],
) -> PatchStats {
    let mut stats = PatchStats::default();

    for change in changes {
        let frs_usize = uffs_mft::frs_to_usize(change.frs);
        let compact_idx = frs_to_compact.get(frs_usize).copied().unwrap_or(u32::MAX);

        if change.deleted {
            if compact_idx == u32::MAX {
                stats.skipped += 1;
            } else if let Some(rec) = drive.records.as_mut_slice().get_mut(compact_idx as usize) {
                rec.name_len = 0;
                // Clear parent so CSR rebuild excludes this record.
                rec.parent_idx = u32::MAX;
                stats.deleted += 1;
            }
        } else if change.created {
            if compact_idx != u32::MAX {
                // Re-animate a previously deleted slot.
                if let Some(rec) = drive.records.as_mut_slice().get_mut(compact_idx as usize)
                    && rec.name_len == 0
                    && !change.filename.is_empty()
                {
                    let name_start = drive.names.len();
                    drive
                        .names
                        .as_mut_vec()
                        .extend_from_slice(change.filename.as_bytes());
                    rec.name_offset = uffs_mft::len_to_u32(name_start);
                    rec.name_len = uffs_mft::len_to_u16(change.filename.len());
                }
                stats.skipped += 1;
            } else if !change.filename.is_empty() {
                let name_start = drive.names.len();
                drive
                    .names
                    .as_mut_vec()
                    .extend_from_slice(change.filename.as_bytes());

                let parent_frs_usize = uffs_mft::frs_to_usize(change.parent_frs);
                let parent_compact = frs_to_compact
                    .get(parent_frs_usize)
                    .copied()
                    .unwrap_or(u32::MAX);

                let new_rec = CompactRecord {
                    size: 0,
                    allocated: 0,
                    treesize: 0,
                    tree_allocated: 0,
                    created: 0,
                    modified: 0,
                    accessed: 0,
                    name_offset: uffs_mft::len_to_u32(name_start),
                    flags: 0,
                    parent_idx: parent_compact,
                    descendants: 0,
                    name_len: uffs_mft::len_to_u16(change.filename.len()),
                    extension_id: 0,
                    // path_len is set to 0 here; the full-array
                    // `compute_path_lengths` call after the USN loop
                    // will populate the correct value for all records.
                    path_len: 0,
                    name_first_byte: change.filename.as_bytes().first().copied().unwrap_or(0),
                    _pad: [0; 1],
                };

                drive.records.as_mut_vec().push(new_rec);
                stats.created += 1;
            } else {
                stats.skipped += 1;
            }
        } else if change.renamed {
            if compact_idx == u32::MAX {
                stats.skipped += 1;
            } else if let Some(rec) = drive.records.as_mut_slice().get_mut(compact_idx as usize) {
                if !change.filename.is_empty() {
                    let name_start = drive.names.len();
                    drive
                        .names
                        .as_mut_vec()
                        .extend_from_slice(change.filename.as_bytes());
                    rec.name_offset = uffs_mft::len_to_u32(name_start);
                    rec.name_len = uffs_mft::len_to_u16(change.filename.len());
                }

                let new_parent_frs = uffs_mft::frs_to_usize(change.parent_frs);
                let new_parent_compact = frs_to_compact
                    .get(new_parent_frs)
                    .copied()
                    .unwrap_or(u32::MAX);

                // Update parent_idx — CSR rebuild picks this up.
                rec.parent_idx = new_parent_compact;
                stats.renamed += 1;
            }
        } else {
            stats.skipped += 1;
        }
    }

    // Rebuild derived structures from updated records + names.
    // Children CSR: ~100ms for 7M records. Trigram: ~500ms for 7M records.
    // Both are necessary so newly created/renamed files appear in tree
    // traversal AND trigram search.
    drive.children = ChildrenIndex::build(&drive.records);
    // Recompute path_len for all records (picks up creates + renames).
    crate::compact::compute_path_lengths(&mut drive.records, &drive.names, drive.letter);
    // Rebuild trigram index using CaseFold — no names_lower clone needed.
    drive.trigram = TrigramIndex::build(&drive.records, &drive.names, drive.fold);
    // Rebuild extension inverted index so --ext queries reflect USN changes.
    drive.ext_index = crate::compact::ExtensionIndex::build(&drive.records);

    stats
}
