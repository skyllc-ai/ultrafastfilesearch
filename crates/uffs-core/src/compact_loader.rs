// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! MFT data loading, caching, and refresh logic for compact indices.
//!
//! Handles cold reads (file/live), IOCP detection, `.uffs` cache integration,
//! compact-cache integration, and USN-based incremental patching.

use std::path::PathBuf;
use std::time::Instant;

use uffs_mft::index::MftIndex;

use crate::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, INDEX_TTL_SECONDS, build_compact_index,
};
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

    // ── Load MftIndex (cache + USN replay, or cold) ────────────────
    //
    // Phase 5 (#94): the previous `try_compact_cache_hit` fast path
    // returned the on-disk compact cache directly without applying
    // USN deltas, so cold-boot WARM load served stale data for any
    // drive whose compact cache was older than the live filesystem
    // (5/7 drives at the v0.5.80 reference run).  We now always go
    // through the MftIndex path: on Windows this triggers
    // `read_index_cached` → `apply_usn_updates_to_fresh_index`, so
    // the rebuilt compact reflects the live USN journal state.  On
    // non-Windows / offline-file sources the MFT itself is the
    // source of truth and USN replay is a no-op.
    //
    // Cost: ~1.5 s per drive for the compact rebuild + trigram
    // step; at N=7 drives in parallel the cold-boot WARM total
    // moves from ~5.7 s to ~7–10 s (one-time per daemon start).
    // The Phase 4 re-promote path uses
    // [`load_drive_with_usn_refresh`] for the same correctness
    // guarantee.
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

/// Timing breakdown for [`load_drive_with_usn_refresh`].
///
/// Mirrors [`LoadTiming`] but with names that match the USN-refresh
/// flow (no "cache hit" arm — that path was removed in #94).  Field
/// names follow [`LoadTiming`]'s `_ms`-free convention.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefreshTiming {
    /// `MftIndex` load wall-clock in milliseconds (cache hit + USN
    /// replay).
    pub mft: u128,
    /// Compact rebuild from the refreshed `MftIndex` in
    /// milliseconds.
    pub compact: u128,
    /// Trigram CSR rebuild in milliseconds.
    pub trigram: u128,
    /// Total wall-clock in milliseconds, including the
    /// background-save submission.
    pub total: u128,
}

/// Phase 5 (#94) re-promote helper.
///
/// Loads the per-drive `MftIndex` from cache, applies USN journal
/// deltas, rebuilds the `DriveCompactIndex` from the refreshed MFT,
/// and submits a background compact-cache save so the next call is
/// faster.
///
/// Used by the daemon's `DiskBodyLoader::load` and (eventually) the
/// background USN refresh timer (#95).  On Windows the call goes
/// through `MftReader::read_index_cached` →
/// `apply_usn_updates_to_fresh_index` which is the canonical USN
/// replay path the cold-boot fall-through has been using all along.
/// On non-Windows the live-volume reader is unavailable and the
/// function returns an error so the caller can fall back to a bare
/// [`crate::compact_cache::load_compact_cache`].
///
/// # Errors
///
/// Returns an error if the live MFT cannot be opened, the cached
/// `MftIndex` cannot be loaded, or the USN journal apply step fails.
/// The compact-cache save is best-effort (warn-logged on failure)
/// and never propagates an error to the caller.
#[cfg(windows)]
pub fn load_drive_with_usn_refresh(
    drive_letter: char,
) -> anyhow::Result<(DriveCompactIndex, RefreshTiming)> {
    let total_start = Instant::now();
    let mft_start = Instant::now();
    let mft_index = load_mft_index_live(drive_letter, /* no_cache = */ false)?;
    let mft = mft_start.elapsed().as_millis();

    let (mut compact, compact_ms_inner, trigram_ms_inner) =
        build_compact_index(drive_letter, &mft_index);
    drop(mft_index);

    // Persist the USN-refreshed compact so the next promote on this
    // letter (or the next cold-boot) starts from a fresher snapshot.
    // Best-effort — warn on failure but don't fail the caller; the
    // returned in-memory body is still correct.
    if let Err(err) = crate::compact_cache::save_compact_cache_background(&compact) {
        tracing::warn!(
            drive = %drive_letter,
            error = %err,
            "Failed to start USN-refreshed compact cache save (best-effort)",
        );
    }

    let total = total_start.elapsed().as_millis();
    tracing::info!(
        target: "shard.transition",
        drive = %drive_letter,
        mft_ms = %mft,
        compact_ms = %compact_ms_inner,
        trigram_ms = %trigram_ms_inner,
        total_ms = %total,
        records = compact.records.len(),
        "♻️ USN-refreshed body ready (load_drive_with_usn_refresh)",
    );
    compact.source = IndexSource::MftFile(PathBuf::from(format!("{drive_letter}:")));
    Ok((compact, RefreshTiming {
        mft,
        compact: compact_ms_inner,
        trigram: trigram_ms_inner,
        total,
    }))
}

/// Non-Windows stub for [`load_drive_with_usn_refresh`].
///
/// USN journals are NTFS-only.  On macOS / Linux the daemon's only
/// cache source is the offline-file path (`MftSource::File`), which
/// does not need USN replay because the underlying `.iocp` /
/// `.uffs` snapshot is the source of truth.  This stub returns an
/// error so callers (notably `DiskBodyLoader::load`) fall back to
/// [`crate::compact_cache::load_compact_cache`] without a Windows
/// `cfg` gate at the call site.
///
/// # Errors
///
/// Always returns an `anyhow` error noting USN replay is unsupported
/// on this platform.  The caller is expected to handle this by
/// falling back to the bare compact-cache load.
#[cfg(not(windows))]
pub fn load_drive_with_usn_refresh(
    drive_letter: char,
) -> anyhow::Result<(DriveCompactIndex, RefreshTiming)> {
    anyhow::bail!(
        "USN refresh not supported on this platform (drive {drive_letter}); caller should fall back to load_compact_cache"
    )
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
///
/// Extracted from `load_drive` for readability; the workspace allows
/// `clippy::single_call_fn` so this remains a one-call helper rather
/// than being inlined.
#[cfg(windows)]
fn load_mft_index_live(drive_letter: char, no_cache: bool) -> anyhow::Result<MftIndex> {
    use anyhow::Context as _;

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

/// Apply USN changes in-place to the compact index.
///
/// Mutates records (`parent_idx`, names, flags) and the
/// `frs_to_compact` mapping then rebuilds the children CSR + path
/// lengths + trigram + extension index once at the end.  Typical
/// cost: <5ms for record mutations + ~100ms for CSR rebuild on a
/// 7M-record drive (the rebuild dominates).
///
/// **Platform note.**  The function itself is pure data manipulation
/// over `DriveCompactIndex` + the platform-agnostic
/// [`uffs_mft::usn::FileChange`] DTO and compiles + runs on all
/// targets.  Only the *journal source* that produces the
/// `&[FileChange]` slice (`uffs_mft::usn::read_usn_journal`) is
/// Windows-only.  This split is what makes the Phase 7 per-shard
/// patch path Mac-testable end-to-end via synthesised
/// [`uffs_mft::usn::FileChange`] arrays.
///
/// **Phase 8.** The `frs_to_compact` mapping is read from
/// [`DriveCompactIndex::frs_to_compact`] (no longer a separate
/// parameter) and maintained in lock-step with the records:
///
/// * **Create** \u2014 a new compact slot is appended at `records.len()` and
///   `frs_to_compact[new_frs]` is updated to point at it (extending the table
///   if the FRS exceeds the current `frs_to_compact.len()`).
/// * **Delete** \u2014 `frs_to_compact[deleted_frs] = u32::MAX` so subsequent
///   batches referencing the deleted FRS take the skip branch instead of
///   mis-applying to a tombstoned slot.
/// * **Rename** \u2014 the FRS keeps its compact slot; only `parent_idx` + name
///   move.  Mapping is unchanged.
///
/// **Empty-mapping fallback.** When `drive.frs_to_compact.is_empty()`
/// (v9 caches loaded before Phase 8 cache format v10) every change
/// looks up to `u32::MAX` and the function increments `skipped` for
/// the whole batch \u2014 the surgical patch silently degrades to a
/// no-op so the caller's full-reload fallback path runs.
#[expect(
    clippy::too_many_lines,
    reason = "Phase 8 surgical-patch loop: the create / delete / rename \
              branches each mutate `drive.frs_to_compact` in a \
              variant-specific way (delete tombstones the slot, create \
              extends + registers, rename leaves it intact); splitting \
              into per-variant helpers would scatter the FRS-mapping \
              invariant across functions and obscure the symmetric \
              treatment that's central to the surgical-patch correctness \
              contract."
)]
pub fn apply_usn_patch(
    drive: &mut DriveCompactIndex,
    changes: &[uffs_mft::usn::FileChange],
) -> PatchStats {
    let mut stats = PatchStats::default();

    for change in changes {
        let frs_usize = uffs_mft::frs_to_usize(change.frs);
        let compact_idx = drive
            .frs_to_compact
            .get(frs_usize)
            .copied()
            .unwrap_or(u32::MAX);

        if change.deleted {
            if compact_idx == u32::MAX {
                stats.skipped += 1;
            } else if let Some(rec) = drive.records.as_mut_slice().get_mut(compact_idx as usize) {
                rec.name_len = 0;
                // Clear parent so CSR rebuild excludes this record.
                rec.parent_idx = u32::MAX;
                // Phase 8: mark the FRS slot unmapped so a future
                // batch can't re-animate the tombstone via the
                // `compact_idx != u32::MAX` create branch below.
                if let Some(slot) = drive.frs_to_compact.get_mut(frs_usize) {
                    *slot = u32::MAX;
                }
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
                let parent_compact = drive
                    .frs_to_compact
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

                let new_compact_idx = uffs_mft::len_to_u32(drive.records.len());
                drive.records.as_mut_vec().push(new_rec);

                // Phase 8: register the new FRS → compact_idx mapping
                // so future batches that reference this FRS find the
                // correct slot.  Extend the table if needed (the FRS
                // may exceed the build-time max — e.g. NTFS reuses
                // freed FRS slots after deletes, and a long-running
                // daemon can outgrow the original `frs_to_idx`
                // capacity).  Sentinel-fill any intermediate gap so
                // skipped FRS values still report `u32::MAX`.
                if frs_usize >= drive.frs_to_compact.len() {
                    drive
                        .frs_to_compact
                        .resize(frs_usize.saturating_add(1), u32::MAX);
                }
                if let Some(slot) = drive.frs_to_compact.get_mut(frs_usize) {
                    *slot = new_compact_idx;
                }
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
                let new_parent_compact = drive
                    .frs_to_compact
                    .get(new_parent_frs)
                    .copied()
                    .unwrap_or(u32::MAX);

                // Update parent_idx — CSR rebuild picks this up.
                rec.parent_idx = new_parent_compact;
                // Rename keeps the FRS in the same compact slot;
                // mapping is unchanged.
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

#[cfg(test)]
#[path = "compact_loader_tests.rs"]
mod tests;
